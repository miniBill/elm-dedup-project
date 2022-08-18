use std::{collections::HashMap, fs, io};

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

    let results: Vec<()> = packages
        .into_par_iter()
        .map(|(package, versions)| {
            if let [author, name] = package.split("/").collect::<Vec<&str>>()[..] {
                let last_version: &String = &versions[versions.len() - 1];
                println!("Cloning {}/{} {}", author, name, last_version);

                fs::create_dir_all(format!("repos/{author}"))?;
                Ok(())
            } else {
                Err(format!("Could not parse {} as author/package-name", package).into())
            }
        })
        .collect::<Result<_, Error>>()?;

    let repo_name: &str = "git/git";
    let url: String = format!("https://github.com/{repo_name}.git");

    // git clone -b '2.0' --depth 1 URL
    println!("git clone -b '2.0' --depth 1 {}", url);

    Ok(())
}
