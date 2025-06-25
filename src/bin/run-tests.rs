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
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{task::JoinHandle, time::Instant};
use wait_timeout::ChildExt;

#[derive(Debug)]
enum Error {
    IO(io::Error),
    SendPath(async_channel::SendError<PathBuf>),
    Join(tokio::task::JoinError),
    ColorEyre(color_eyre::Report),
    Other(String),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::IO(e)
    }
}

impl From<async_channel::SendError<PathBuf>> for Error {
    fn from(e: async_channel::SendError<PathBuf>) -> Self {
        Error::SendPath(e)
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(e: tokio::task::JoinError) -> Self {
        Error::Join(e)
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

#[tokio::main]
async fn main() -> Result<(), Error> {
    let concurrency: u16 = 4;

    let (paths_sender, paths_receiver): (
        async_channel::Sender<PathBuf>,
        async_channel::Receiver<PathBuf>,
    ) = async_channel::unbounded();
    let (done_sender, done_receiver): (async_channel::Sender<Done>, async_channel::Receiver<Done>) =
        async_channel::unbounded();
    let in_progress: Arc<Mutex<HashMap<PathBuf, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    let dones: Arc<Mutex<Vec<Done>>> = Arc::new(Mutex::new(Vec::new()));

    let stopping: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    let walker: JoinHandle<Result<(), Error>> = {
        let stopping: Arc<Mutex<bool>> = Arc::clone(&stopping);
        tokio::spawn(async move {
            let paths: Result<(), Error> = (async || {
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
                                paths_sender.send(version_root).await?;
                            }
                        }
                    }
                }
                Ok(())
            })()
            .await;
            paths_sender.close();
            paths
        })
    };

    let mut testers: Vec<JoinHandle<Result<(), Error>>> = Vec::new();
    for _i in 0..concurrency {
        let stopping: Arc<Mutex<bool>> = Arc::clone(&stopping);
        let paths_receiver: async_channel::Receiver<PathBuf> = paths_receiver.clone();
        let done_sender: async_channel::Sender<Done> = done_sender.clone();
        let in_progress: Arc<Mutex<HashMap<PathBuf, Instant>>> = in_progress.clone();
        let tester: JoinHandle<Result<(), Error>> = tokio::spawn(async move {
            loop {
                if *stopping.lock().expect("Could not lock \"stopping\"") {
                    return Ok(());
                }

                let version_root: PathBuf = match paths_receiver.recv_blocking() {
                    Ok(version_root) => version_root,
                    Err(async_channel::RecvError) => return Ok(()),
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

                // If we have a send error it's because we're done, no need to deal with it
                let _ = done_sender
                    .send(Done {
                        path: version_root,
                        time: start.elapsed(),
                        elm_result,
                        lamdera_result,
                    })
                    .await;
            }
        });
        testers.push(tester);
    }

    let consumer: JoinHandle<Result<(), Error>> = {
        let stopping = Arc::clone(&stopping);
        let dones = Arc::clone(&dones);
        tokio::spawn(async move {
            loop {
                if *stopping.lock().expect("Could not lock \"stopping\"") {
                    return Ok(());
                }

                let done = match done_receiver.recv_blocking() {
                    Ok(done) => done,
                    Err(async_channel::RecvError) => return Ok(()),
                };

                dones.lock().expect("Could not lock \"dones\"").push(done);
            }
        })
    };

    let tui: JoinHandle<Result<(), Error>> = tokio::spawn(async move {
        color_eyre::install()?;
        let res: Result<(), Error> = (|| {
            let mut terminal: ratatui::Terminal<_> = ratatui::init();

            loop {
                terminal.draw(|frame: &mut ratatui::Frame| {
                    let summary_table: widgets::Table<'_> = widgets::Table::new(
                        vec![
                            widgets::Row::new(vec![
                                "Pending".into(),
                                format!("{}", paths_receiver.len()),
                            ]),
                            widgets::Row::new(vec![
                                "In progress".into(),
                                format!(
                                    "{}",
                                    in_progress
                                        .lock()
                                        .expect("Could not lock \"in_progress\"")
                                        .len()
                                ),
                            ]),
                        ],
                        vec![layout::Constraint::Fill(1), layout::Constraint::Length(5)],
                    )
                    .block(
                        widgets::Block::default()
                            .title(" Summary ")
                            .border_style(style::Style::default().fg(style::Color::Blue))
                            .border_type(widgets::BorderType::Rounded)
                            .borders(widgets::Borders::ALL),
                    );

                    let in_progress_table: widgets::Table<'_> = widgets::Table::new(
                        in_progress
                            .lock()
                            .expect("Could not lock \"in_progress\"")
                            .iter()
                            .map(|(path, instant)| {
                                widgets::Row::new(vec![
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

                    let mut done_list: Vec<Done> = dones
                        .lock()
                        .expect("Could not lock \"dones\"")
                        .iter()
                        .map(|done| done.clone())
                        .collect::<Vec<_>>();
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
                                widgets::Row::new(vec![
                                    text::Line::raw(format!("{}", done.path.display())),
                                    view_done_result(done.elm_result),
                                    view_done_result(done.lamdera_result),
                                    text::Line::raw(format!("{}s", done.time.as_secs()))
                                        .right_aligned(),
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
                    .header(
                        widgets::Row::new(["Package", "  Elm  ", "Lamdera", "   Time   "]).yellow(),
                    )
                    .block(
                        widgets::Block::default()
                            .title(" Done ")
                            .border_style(style::Style::default().fg(style::Color::Blue))
                            .border_type(widgets::BorderType::Rounded)
                            .borders(widgets::Borders::ALL),
                    );

                    let layout: std::rc::Rc<[ratatui::prelude::Rect]> = layout::Layout::vertical([
                        layout::Constraint::Length(3),
                        layout::Constraint::Length(2 + concurrency),
                        layout::Constraint::Fill(1),
                    ])
                    .split(frame.area());

                    frame.render_widget(summary_table, layout[0]);
                    frame.render_widget(in_progress_table, layout[1]);
                    frame.render_widget(done_table, layout[2]);
                })?;

                if let Ok(available) = crossterm::event::poll(Duration::from_millis(1000 / 60)) {
                    if !available {
                        continue;
                    }
                }

                match crossterm::event::read()? {
                    Event::Key(_) => return Ok(()),
                    Event::FocusGained => {}
                    Event::FocusLost => {}
                    Event::Mouse(_mouse_event) => {}
                    Event::Paste(_) => {}
                    Event::Resize(_, _) => {}
                }
            }
        })();

        *stopping.lock().expect("Could not lock \"stopping\"") = true;

        ratatui::restore();

        res
    });

    walker.await??;
    for tester in testers {
        tester.await??;
    }
    consumer.await??;
    tui.await??;

    Ok(())
}

fn check_tests_for(path: &PathBuf) -> Result<(RunResult, RunResult), Error> {
    let elm_json = path.join("elm.json");

    let elm_json_content = fs::read_to_string(elm_json)?;

    let elm_test_version = if elm_json_content.contains("\"elm-explorations/test\": \"1") {
        "elm-test@0.19.1-revision9"
    } else {
        "elm-test"
    };

    let run_tests_with = |compiler| {
        fs::remove_dir_all(path.join("elm-stuff"))?;
        let timeout = Duration::from_secs(10);

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
                Ok(RunResult::TimedOut)
            }
        }
    };

    let elm_result = run_tests_with("elm")?;
    let lamdera_result = run_tests_with("lamdera")?;

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
