#![feature(mpmc_channel)]

use crossterm::event::Event;
use ratatui::{
    layout,
    style::{self, Stylize},
    text, widgets,
};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpmc, Mutex},
    thread::{self, ScopedJoinHandle},
    time::{Duration, Instant},
};
use wait_timeout::ChildExt;

#[derive(Debug)]
enum Error {
    IO(io::Error),
    SendPath(mpmc::SendError<PathBuf>),
    ColorEyre(color_eyre::Report),
    Other(String),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::IO(e)
    }
}

impl From<mpmc::SendError<PathBuf>> for Error {
    fn from(e: mpmc::SendError<PathBuf>) -> Self {
        Error::SendPath(e)
    }
}

impl From<color_eyre::Report> for Error {
    fn from(e: color_eyre::Report) -> Self {
        Error::ColorEyre(e)
    }
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error::Other(e)
    }
}

impl From<&'static str> for Error {
    fn from(e: &'static str) -> Self {
        Error::Other(e.into())
    }
}

#[derive(Clone)]
struct Done {
    path: PathBuf,
    time: Duration,
    elm_result: RunResult,
    lamdera_result: RunResult,
}

#[derive(Clone, PartialEq)]
enum RunResult {
    Finished(bool),
    TimedOut,
}

const CONCURRENCY: u16 = 10;
const FPS: u64 = 20;

fn main() -> Result<(), Error> {
    let (paths_sender, paths_receiver): (mpmc::Sender<PathBuf>, mpmc::Receiver<PathBuf>) =
        mpmc::channel();
    let in_progress: Mutex<HashMap<PathBuf, Instant>> = Mutex::new(HashMap::new());
    let dones: Mutex<Vec<Done>> = Mutex::new(Vec::new());
    let stopping: Mutex<bool> = Mutex::new(false);

    thread::scope::<_, Result<(), Error>>(|scope| {
        let walker: ScopedJoinHandle<Result<(), Error>> = scope.spawn(|| {
            let paths: Result<(), Error> = walk_path(&stopping, &paths_sender);
            drop(paths_sender);
            paths
        });

        let mut testers: Vec<ScopedJoinHandle<Result<(), Error>>> = Vec::new();
        for _i in 0..CONCURRENCY {
            let tester: ScopedJoinHandle<Result<(), Error>> = scope.spawn(|| loop {
                if *stopping.lock().expect("Could not lock \"stopping\"") {
                    return Ok(());
                }

                let version_root: PathBuf = match paths_receiver.recv() {
                    Ok(version_root) => version_root,
                    Err(mpmc::RecvError) => return Ok(()),
                };

                let start: Instant = Instant::now();
                in_progress
                    .lock()
                    .expect("Could not lock \"in_progress\"")
                    .insert(version_root.clone(), start);

                let res: Result<(RunResult, RunResult), Error> = check_tests_for(&version_root);

                in_progress
                    .lock()
                    .expect("Could not lock \"in_progress\"")
                    .remove(&version_root);

                let (elm_result, lamdera_result) = res?;

                dones.lock().expect("Could not lock \"dones\"").push(Done {
                    path: version_root,
                    time: start.elapsed(),
                    elm_result,
                    lamdera_result,
                });
            });
            testers.push(tester);
        }

        let tui: ScopedJoinHandle<Result<(), Error>> = scope.spawn(|| {
            color_eyre::install()?;
            let res: Result<(), Error> =
                ui_thread(&stopping, &paths_receiver, &in_progress, &dones);

            ratatui::restore();

            res
        });

        let res: Result<Result<(), Error>, _> = tui.join();

        *stopping.lock().expect("Could not lock \"stopping\"") = true;

        // We can unwrap here because an Err means that the thread panic!ed
        res.unwrap()?;

        println!("Waiting for directory walker to exit.");
        // We can unwrap here because an Err means that the thread panic!ed
        walker.join().unwrap()?;

        println!("Waiting for testers to exit.");
        for tester in testers {
            // We can unwrap here because an Err means that the thread panic!ed
            tester.join().unwrap()?;
        }

        Ok(())
    })
}

fn walk_path(stopping: &Mutex<bool>, paths_sender: &mpmc::Sender<PathBuf>) -> Result<(), Error> {
    let repos = Path::new("repos");
    for author_root in read_dir(&repos)? {
        for package_root in read_dir(author_root)? {
            for version_root in read_dir(package_root)? {
                if *stopping.lock().expect("Could not lock \"stopping\"") {
                    return Ok(());
                }
                let tests: PathBuf = version_root.join("tests");
                let elm_json: PathBuf = version_root.join("elm.json");
                if tests.exists() && elm_json.exists() {
                    paths_sender.send(version_root)?;
                }
            }
        }
    }
    Ok(())
}

fn ui_thread(
    stopping: &Mutex<bool>,
    paths_receiver: &mpmc::Receiver<PathBuf>,
    in_progress: &Mutex<HashMap<PathBuf, Instant>>,
    dones: &Mutex<Vec<Done>>,
) -> Result<(), Error> {
    let mut terminal: ratatui::Terminal<_> = ratatui::init();

    let start: Instant = Instant::now();

    loop {
        if *stopping.lock().expect("Could not lock \"stopping\"") {
            return Ok(());
        }

        terminal.draw(|frame: &mut ratatui::Frame| {
            view(frame, paths_receiver, in_progress, dones, start.elapsed());
        })?;

        if let Ok(available) = crossterm::event::poll(Duration::from_millis(1000 / FPS)) {
            if !available {
                continue;
            }
        }

        match crossterm::event::read()? {
            Event::Key(key) => match key.code {
                crossterm::event::KeyCode::Char('q') => return Ok(()),
                _ => {}
            },
            Event::FocusGained => {}
            Event::FocusLost => {}
            Event::Mouse(_mouse_event) => {}
            Event::Paste(_) => {}
            Event::Resize(_, _) => {}
        }
    }
}

fn view(
    frame: &mut ratatui::Frame,
    paths_receiver: &mpmc::Receiver<PathBuf>,
    in_progress: &Mutex<HashMap<PathBuf, Instant>>,
    dones: &Mutex<Vec<Done>>,
    duration: Duration,
) {
    let in_progress: std::sync::MutexGuard<'_, HashMap<PathBuf, Instant>> =
        in_progress.lock().expect("Could not lock \"in_progress\"");
    let dones: std::sync::MutexGuard<'_, Vec<Done>> =
        dones.lock().expect("Could not lock \"dones\"");
    let layout: std::rc::Rc<[ratatui::prelude::Rect]> = layout::Layout::vertical([
        layout::Constraint::Length(6),
        layout::Constraint::Length(match in_progress.len() as u16 {
            0 => 0,
            l => l + 2,
        }),
        layout::Constraint::Fill(1),
    ])
    .split(frame.area());

    render_summary(
        frame,
        paths_receiver,
        &*in_progress,
        &*dones,
        duration,
        &layout[0],
    );

    let in_progress_table: widgets::Table<'_> = widgets::Table::new(
        in_progress
            .iter()
            .map(|(path, instant)| {
                widgets::Row::new([
                    format!("{}", path.display()),
                    format!("{:>9}s", instant.elapsed().as_secs()),
                ])
            })
            .collect::<Vec<_>>(),
        [layout::Constraint::Fill(1), layout::Constraint::Length(10)],
    )
    .block(
        widgets::Block::default()
            .title(" In progress ")
            .border_style(style::Style::default().fg(style::Color::Blue))
            .border_type(widgets::BorderType::Rounded)
            .borders(widgets::Borders::ALL),
    );

    let mut done_list: Vec<Done> = dones.iter().map(|done| done.clone()).collect::<Vec<_>>();
    done_list.reverse();
    done_list.sort_by_key(|done| {
        if done.elm_result != done.lamdera_result {
            0
        } else {
            1
        }
    });
    fn view_done_result<'a>(result: RunResult) -> ratatui::prelude::Line<'a> {
        match result {
            RunResult::Finished(true) => text::Line::raw("✅").centered(),
            RunResult::Finished(false) => text::Line::raw("❌").centered(),
            RunResult::TimedOut => text::Line::raw("⏰").centered(),
        }
    }
    let done_table: widgets::Table<'_> = widgets::Table::new(
        done_list
            .into_iter()
            .map(|done| {
                widgets::Row::new([
                    text::Line::raw(format!("{}", done.path.display())),
                    view_done_result(done.elm_result),
                    view_done_result(done.lamdera_result),
                    text::Line::raw(format!("{}s", done.time.as_secs())).right_aligned(),
                ])
            })
            .collect::<Vec<_>>(),
        [
            layout::Constraint::Fill(1),
            layout::Constraint::Length(7),
            layout::Constraint::Length(7),
            layout::Constraint::Length(10),
        ],
    )
    .header(widgets::Row::new(["Package", "  Elm  ", "Lamdera", "   Time   "]).yellow())
    .block(
        widgets::Block::default()
            .title(" Done ")
            .border_style(style::Style::default().fg(style::Color::Blue))
            .border_type(widgets::BorderType::Rounded)
            .borders(widgets::Borders::ALL),
    );

    frame.render_widget(in_progress_table, layout[1]);
    frame.render_widget(done_table, layout[2]);
}

fn render_summary(
    frame: &mut ratatui::Frame<'_>,
    paths_receiver: &mpmc::Receiver<PathBuf>,
    in_progress: &HashMap<PathBuf, Instant>,
    dones: &Vec<Done>,
    duration: Duration,
    area: &ratatui::prelude::Rect,
) {
    let total: usize = dones.len() + in_progress.len() + paths_receiver.len();

    let progress: f64 = if total == 0 {
        0.0
    } else {
        dones.len() as f64 / total as f64
    };

    let eta: u32 = (duration.as_secs_f64() * (1.0 / progress - 1.0)) as u32;

    let summary_block = widgets::Block::default()
        .title(" Summary ")
        .border_style(style::Style::default().fg(style::Color::Blue))
        .border_type(widgets::BorderType::Rounded)
        .borders(widgets::Borders::ALL);

    let summary_table: widgets::Table<'_> = widgets::Table::new(
        [
            widgets::Row::new([
                text::Line::raw("Pending"),
                text::Line::raw(format!("{}", paths_receiver.len())).right_aligned(),
            ]),
            widgets::Row::new([
                text::Line::raw("In progress"),
                text::Line::raw(format!("{}", in_progress.len())).right_aligned(),
            ]),
            widgets::Row::new([
                text::Line::raw("Expected time until end"),
                text::Line::raw(format!("{}m {:2}s", eta / 60, eta % 60)).right_aligned(),
            ]),
        ],
        [layout::Constraint::Fill(1), layout::Constraint::Length(10)],
    );

    let summary_gauge: widgets::Gauge<'_> = widgets::Gauge::default()
        .ratio(progress)
        .gauge_style(style::Color::Blue);

    let summary_sublayout = layout::Layout::vertical([
        layout::Constraint::Length(3), // Table
        layout::Constraint::Length(1), // Gauge
    ])
    .split(area.inner(layout::Margin {
        horizontal: 1,
        vertical: 1,
    }));
    frame.render_widget(summary_block, *area);
    frame.render_widget(summary_table, summary_sublayout[0]);
    frame.render_widget(summary_gauge, summary_sublayout[1]);
}

fn check_tests_for(path: &PathBuf) -> Result<(RunResult, RunResult), Error> {
    let elm_json: PathBuf = path.join("elm.json");

    let elm_json_content: String = fs::read_to_string(elm_json)?;

    let elm_test_version: &'static str =
        if elm_json_content.contains("\"elm-explorations/test\": \"1") {
            "elm-test@0.19.1-revision9"
        } else {
            "elm-test"
        };

    let run_tests_with = |compiler| {
        let elm_stuff: PathBuf = path.join("elm-stuff");
        if elm_stuff.exists() {
            fs::remove_dir_all(path.join("elm-stuff"))?;
        }

        let timeout: Duration = Duration::from_secs(120);

        let mut elm_child: std::process::Child = Command::new("npx")
            .args(["--yes", elm_test_version, "--compiler", compiler])
            .current_dir(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        match elm_child.wait_timeout(timeout)? {
            Some(status) => Ok::<RunResult, Error>(RunResult::Finished(status.success())),
            None => {
                elm_child.kill()?;
                elm_child.wait()?;
                Ok(RunResult::TimedOut)
            }
        }
    };

    let elm_result: RunResult = run_tests_with("elm")?;
    let lamdera_result: RunResult = run_tests_with("lamdera")?;

    return Ok((elm_result, lamdera_result));
}

fn read_dir<T>(path: T) -> Result<Vec<PathBuf>, Error>
where
    T: AsRef<Path>,
{
    let mut entries: Vec<PathBuf> = fs::read_dir(path)?
        .into_iter()
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<PathBuf>, io::Error>>()?;

    entries.sort();

    return Ok(entries);
}
