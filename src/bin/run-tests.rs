use colored::*;
use rayon::{
    iter::{IntoParallelIterator, ParallelBridge},
    prelude::ParallelIterator,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug)]
enum Error {
    IO(io::Error),
    Other(String),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::IO(e)
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

#[tokio::main]
async fn main() -> Result<(), Error> {
    println!("{}", "Starting tests".blue());
    let repos = Path::new("repos");
    let mut version_roots: Vec<PathBuf> = read_dir(&repos)?
        .into_iter()
        .map(|author_root| {
            Ok(read_dir(&author_root)?
                .into_iter()
                .map(|package_root| read_dir(&package_root))
                .collect::<Result<Vec<_>, Error>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>, Error>>()?
        .into_iter()
        .flatten()
        .collect();

    version_roots.sort();

    version_roots
        .into_par_iter()
        .map(|version_root| {
            let tests = version_root.join("tests");
            let elm_json = version_root.join("elm.json");

            if tests.exists() && elm_json.exists() {
                check_tests_for(version_root)
            } else {
                Ok(())
            }
        })
        .collect::<Result<Vec<_>, Error>>()?;

    Ok(())
}

fn check_tests_for(path: PathBuf) -> Result<(), Error> {
    println!("{}", path.display());

    let elm_json = path.join("elm.json");

    let elm_json_content = fs::read_to_string(elm_json)?;

    let elm_test_version = if elm_json_content.contains("\"elm-explorations/test\": \"1") {
        "elm-test@0.19.1-revision9"
    } else {
        "elm-test"
    };

    let is_vanilla_ok: bool = Command::new("npx")
        .args(["--yes", elm_test_version, "--compiler", "elm"])
        .current_dir(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?
        .wait()?
        .success();
    if !is_vanilla_ok {
        println!(
            "{} {}",
            "!!! Error running tests in".yellow(),
            format!("{}", path.display()).blue()
        );
        return Ok(());
    }

    let is_lamdera_ok: bool = Command::new("npx")
        .args(["--yes", elm_test_version, "--compiler", "lamdera"])
        .current_dir(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?
        .wait()?
        .success();
    if !is_lamdera_ok {
        // Re-run the tests to show the output
        let _ = Command::new("npx")
            .args(["--yes", elm_test_version, "--compiler", "lamdera"])
            .current_dir(&path)
            .spawn()?
            .wait()?
            .success();

        println!(
            "{} {}",
            "!!! Error running tests in".red(),
            format!("{}", path.display()).blue()
        );
        return Err("There is a compiler bug!".into());
    }

    return Ok(());
}

fn read_dir<T>(path: T) -> Result<Vec<PathBuf>, Error>
where
    T: AsRef<Path>,
{
    let mut entries: Vec<PathBuf> = fs::read_dir(path)?
        .into_iter()
        .par_bridge()
        .map(|res| res.map(|e| e.path()))
        .collect::<Result<Vec<PathBuf>, io::Error>>()?;

    entries.sort();

    return Ok(entries);
}
