use colored::*;
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use serde::Deserialize;
use std::{fs, io, path::Path, process::Command};

#[derive(Debug)]
enum Error {
    Reqwest(reqwest::Error),
    IO(io::Error),
    Other(String),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::IO(e)
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Reqwest(e)
    }
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error::Other(e)
    }
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
}

enum CloneStatus {
    Cloned,
    AlreadyPresent,
    Error,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    println!("{}", "Getting packages list".blue());
    let packages: Vec<Package> = reqwest::get("https://package.elm-lang.org/search.json")
        .await?
        .json()
        .await?;

    let result: Vec<CloneStatus> = packages
        .into_par_iter()
        .map(|package: Package| {
            let package_name: String = package.name;
            let package_version: String = package.version;

            if Path::new(&format!("repos/{package_name}/{package_version}")).exists() {
                return Ok(CloneStatus::AlreadyPresent);
            }

            println!(
                "{} {}@{}",
                "Cloning".green(),
                package_name.blue(),
                package_version.blue()
            );

            fs::create_dir_all(format!("repos/{package_name}"))?;

            // Use git URL to avoid username/password prompts
            let url: String = format!("git@github.com:{package_name}.git");
            let is_ok: bool = Command::new("git")
                .args([
                    "clone",
                    "--quiet",
                    "--branch",
                    &package_version,
                    "--depth",
                    "1",
                    &url,
                    &format!("repos/{package_name}/{package_version}"),
                ])
                .spawn()?
                .wait()?
                .success();
            if !is_ok {
                println!("{} {}", "!!! Error cloning ".red(), package_name.blue());

                return Ok(CloneStatus::Error);
            }

            Ok(CloneStatus::Cloned)
        })
        .collect::<Result<_, Error>>()?;

    let (present, cloned, error) = result
        .iter()
        .fold((0, 0, 0), |(present, cloned, error), r| match r {
            CloneStatus::Cloned => (present, cloned + 1, error),
            CloneStatus::AlreadyPresent => (present + 1, cloned, error),
            CloneStatus::Error => (present, cloned, error + 1),
        });
    println!(
        "{}",
        format!("Cloned {cloned}, errored {error}, already present {present}").green(),
    );

    Ok(())
}
