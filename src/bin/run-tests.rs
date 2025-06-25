use colored::*;
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use tokio::{sync::Mutex, task::JoinHandle, time::Instant};

#[derive(Debug)]
enum Error {
    IO(io::Error),
    SendPath(async_channel::SendError<PathBuf>),
    SendDone(async_channel::SendError<Done>),
    Join(tokio::task::JoinError),
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

impl From<async_channel::SendError<Done>> for Error {
    fn from(e: async_channel::SendError<Done>) -> Self {
        Error::SendDone(e)
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(e: tokio::task::JoinError) -> Self {
        Error::Join(e)
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

struct Done {
    path: PathBuf,
    time: Duration,
    elm_result: bool,
    lamdera_result: bool,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let (paths_sender, paths_receiver): (
        async_channel::Sender<PathBuf>,
        async_channel::Receiver<PathBuf>,
    ) = async_channel::unbounded();
    let (done_sender, done_receiver): (async_channel::Sender<Done>, async_channel::Receiver<Done>) =
        async_channel::unbounded();
    let in_progress: Mutex<HashMap<PathBuf, Instant>> = Mutex::new(HashMap::new());
    let mut dones: Vec<Done> = Vec::new();

    let walker: JoinHandle<Result<(), Error>> = tokio::spawn(async move {
        let paths: Result<(), Error> = (async || {
            let repos = Path::new("repos");
            for author_root in read_dir(&repos)? {
                for package_root in read_dir(author_root)? {
                    for version_root in read_dir(package_root)? {
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
    });

    let tester: JoinHandle<Result<(), Error>> = tokio::spawn(async move {
        loop {
            let version_root: PathBuf = match paths_receiver.recv_blocking() {
                Ok(version_root) => version_root,
                Err(async_channel::RecvError) => return Ok(()),
            };

            let start: Instant = Instant::now();
            in_progress.lock().await.insert(version_root.clone(), start);

            let res: Result<(bool, bool), Error> = check_tests_for(&version_root);

            in_progress.lock().await.remove(&version_root);

            let (elm_result, lamdera_result) = res?;

            done_sender
                .send(Done {
                    path: version_root,
                    time: start.elapsed(),
                    elm_result,
                    lamdera_result,
                })
                .await?;
        }
    });

    let consumer: JoinHandle<Result<(), Error>> = tokio::spawn(async move {
        loop {
            let done = match done_receiver.recv_blocking() {
                Ok(done) => done,
                Err(async_channel::RecvError) => return Ok(()),
            };

            dones.push(done);
        }
    });

    walker.await??;
    tester.await??;
    consumer.await??;

    Ok(())
}

fn check_tests_for(path: &PathBuf) -> Result<(bool, bool), Error> {
    println!("{}", path.display());

    let elm_json = path.join("elm.json");

    let elm_json_content = fs::read_to_string(elm_json)?;

    let elm_test_version = if elm_json_content.contains("\"elm-explorations/test\": \"1") {
        "elm-test@0.19.1-revision9"
    } else {
        "elm-test"
    };

    let elm_result: bool = Command::new("npx")
        .args(["--yes", elm_test_version, "--compiler", "elm"])
        .current_dir(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?
        .wait()?
        .success();

    let lamdera_result: bool = Command::new("npx")
        .args(["--yes", elm_test_version, "--compiler", "lamdera"])
        .current_dir(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?
        .wait()?
        .success();

    if lamdera_result != elm_result {
        // Re-run the tests to show the output
        let _ = Command::new("npx")
            .args(["--yes", elm_test_version, "--compiler", "lamdera"])
            .current_dir(&path)
            .spawn()?
            .wait()?
            .success();

        println!(
            "{} {}",
            "!!! Difference in test results between elm and lamdera in".red(),
            format!("{}", path.display()).blue()
        );
        return Err("There is a compiler bug!".into());
    }

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
