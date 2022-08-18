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

#[tokio::main]
async fn main() -> Result<(), Error> {
    println!("Getting packages list");
    let packages: Vec<Package> = reqwest::get("https://package.elm-lang.org/search.json")
        .await?
        .json()
        .await?;

    packages
        .into_par_iter()
        .map(|package| {
            let package_name = package.name;
            if Path::new(&format!("repos/{package_name}")).exists() {
                return Ok(());
            }

            let author: &str =
                if let [author, _name] = package_name.split("/").collect::<Vec<&str>>()[..] {
                    author
                } else {
                    return Err(
                        format!("Could not parse {} as author/package-name", package_name).into(),
                    );
                };

            let package_version: &String = &package.version;
            println!("Cloning {package_name}@{package_version}");

            fs::create_dir_all(format!("repos/{author}"))?;

            let url: String = format!("git@github.com:{package_name}.git");
            let is_ok: bool = Command::new("git")
                .args([
                    "clone",
                    "--quiet",
                    "--branch",
                    package_version,
                    "--depth",
                    "1",
                    &url,
                    &format!("repos/{package_name}"),
                ])
                .spawn()?
                .wait()?
                .success();
            if !is_ok {
                println!("!!! Error cloning {package_name}");
                return Ok(());
            }

            Ok(())
        })
        .collect::<Result<_, Error>>()?;

    Ok(())
}
