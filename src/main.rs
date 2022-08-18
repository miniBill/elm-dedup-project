use std::{collections::HashMap, fs, io, process::Command};

use rayon::prelude::{IntoParallelIterator, ParallelIterator};

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

#[tokio::main]
async fn main() -> Result<(), Error> {
    println!("Getting packages list");
    let packages: HashMap<String, Vec<String>> =
        reqwest::get("https://package.elm-lang.org/all-packages")
            .await?
            .json()
            .await?;

    packages
        .into_par_iter()
        .map(|(package, versions)| {
            if let [author, _name] = package.split("/").collect::<Vec<&str>>()[..] {
                let last_version: &String = &versions[versions.len() - 1];
                println!("Cloning {package}@{last_version} in {package}");

                fs::create_dir_all(format!("repos/{author}"))?;

                let url: String = format!("git@github.com:{package}.git");
                let is_ok: bool = Command::new("git")
                    .args([
                        "clone",
                        "-b",
                        last_version,
                        "--depth",
                        "1",
                        &url,
                        &format!("repos/{package}"),
                    ])
                    .spawn()?
                    .wait()?
                    .success();
                if !is_ok {
                    Err(format!("!!! Error cloning {package}").into())
                } else {
                    Ok(())
                }
            } else {
                Err(format!("Could not parse {} as author/package-name", package).into())
            }
        })
        .collect::<Result<_, Error>>()?;

    Ok(())
}
