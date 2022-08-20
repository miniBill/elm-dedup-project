use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use std::{
    ffi::OsString,
    fs::{self, ReadDir},
    io,
    process::Command,
    sync::atomic::{self, AtomicU32, Ordering},
};

#[derive(Debug)]
enum Error {
    IO(io::Error),
    Other(String),
    OsStringConversion(OsString),
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

impl From<OsString> for Error {
    fn from(e: OsString) -> Self {
        Error::OsStringConversion(e)
    }
}

#[tokio::main]
async fn main() -> () {
    println!("Getting repos list");
    let authors: ReadDir = fs::read_dir("repos").unwrap();

    let repos: Vec<String> = authors
        .into_iter()
        .flat_map(|author| {
            let author = author.unwrap().file_name().into_string().unwrap();
            fs::read_dir(format!("repos/{author}"))
                .unwrap()
                .into_iter()
                .map(|repo| {
                    let repo = repo.unwrap().file_name().into_string().unwrap();
                    format!("repos/{author}/{repo}")
                })
                .collect::<Vec<String>>()
        })
        .collect();

    println!("Got repos list");

    let home = std::env::home_dir()
        .unwrap()
        .into_os_string()
        .into_string()
        .unwrap();

    println!("Running elm-review");

    let total = repos.len();
    let done = AtomicU32::new(0);

    repos.into_par_iter().for_each(|path| {
        let output: String = String::from_utf8(
            Command::new("elm-review")
                .args([
                    "--config",
                    &format!("{home}/src/elm-review-simplify/preview"),
                ])
                .current_dir(&path)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        if output == "I found no errors!\n" {
            let count = done.fetch_add(1, Ordering::AcqRel);
            println!("{count:5}/{total}");
            return;
        }

        println!("\n\n==========================\n\n{path}\n\n{output}")
    })
}
