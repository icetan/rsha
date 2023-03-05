use std::fmt;
use std::process::{Command, Stdio};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use thiserror::Error as ThisError;
use sha2::{Sha256, Digest};
use strict_yaml_rust::{StrictYaml as Yaml};

use crate::error::{Error, Result};

pub trait FromYaml: Sized {
    fn from_yaml(yaml: &Yaml) -> Result<Self>;
}

type Sha = String;

#[derive(Debug)]
pub enum ReifySuccess {
    ExecSuccess(Sha),
    Noop,
}

#[derive(ThisError, Debug)]
pub enum ReifyFail {
    #[error("exit {0}")]
    ExecFail(i32),
    #[error("missing required files")]
    MissingRequiredFiles,
    #[error("dry run, things have changed")]
    DryFail,
}

pub type ReifyResult = core::result::Result<ReifySuccess, ReifyFail>;

#[derive(Debug)]
pub struct Entry {
    name: String,
    cmd: String,
    required_files: Vec<String>,
    files: Vec<String>,
    sha: Option<String>,
}

fn canonicalize(p: &String) -> Option<PathBuf> {
    Path::new(p).canonicalize().ok()
}

fn str_vec(y: &Yaml) -> Vec<String> {
    match y {
        Yaml::Array(x) => x.iter()
            .filter_map(Yaml::as_str)
            .map(String::from)
            .collect::<Vec<_>>(),
        Yaml::String(x) => vec![x.into()],
        _ => vec![],
    }
}

impl Entry {
    fn calc_sha(&self) -> Result<Sha> {
        let mut hasher = Sha256::new();
        let mut buffer = [0; 1024];
        let mut all_files = self.files.iter()
            .chain(self.required_files.iter())
            .filter_map(canonicalize)
            .collect::<Vec<_>>();
        all_files.sort();
        for file in all_files {
            let input = File::open(&file)?;
            let mut reader = BufReader::new(input);

            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 { break }
                hasher.update(&buffer[..count]);
            }
        }
        hasher.update(&self.cmd);
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn exec(&self) -> Result<i32> {
        let mut child = Command::new("/bin/bash")
            .arg("-c")
            .arg(vec!["set -xe", &self.cmd].join("\n"))
            .env("files", self.files.join("\n"))
            .env("required_files", self.required_files.join("\n"))
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        match child.wait()?.code() {
            Some(code) => Ok(code),
            None => Err(Error::Unknown)
        }
    }

    fn check_then<F>(&self, exec: F) -> Result<ReifyResult>
    where F: FnOnce() -> Result<ReifyResult> {
        if let Some(old_sha) = self.sha.as_ref() {
            // Check if existing sha matches newly calculated one
            let new_sha = self.calc_sha()?;
            if &new_sha != old_sha {
                // If shas don't match execute entry and re-calculate sha
                exec()
            } else {
                // Sha hasn't changed
                Ok(Ok(ReifySuccess::Noop))
            }
        } else {
            // No sha to compare, execute entry and calculate sha
            exec()
        }
    }

    pub fn reify(&self) -> Result<ReifyResult> {
        let exec = || self.exec()
            .and_then(|code| {
                if code == 0 {
                    self.calc_sha()
                        .and_then(|sha| Ok(Ok(ReifySuccess::ExecSuccess(sha))))
                } else {
                    Ok(Err(ReifyFail::ExecFail(code)))
                }
            });

        let len = self.required_files.iter().filter_map(canonicalize).collect::<Vec<_>>().len();
        if  self.required_files.len() == len {
            self.check_then(exec)
        } else {
            Ok(Err(ReifyFail::MissingRequiredFiles))
        }
    }

    pub fn dry_run(&self) -> Result<ReifyResult> {
        self.check_then(|| Ok(Err(ReifyFail::DryFail)))
    }

    pub fn dump(&self, w: &mut dyn fmt::Write, new_sha: Option<Sha>) -> Result<()> {
        writeln!(w ,"-")?;

        if self.name != "" {
            writeln!(w ,"  name: {}", self.name)?;
        }

        writeln!(w ,"  cmd: |")?;
        for line in self.cmd.lines() {
            writeln!(w ,"    {}", line)?;
        }

        if ! self.required_files.is_empty() {
            writeln!(w ,"  required_files:")?;
            for file in self.required_files.iter() {
                writeln!(w ,"  - {file}")?;
            }
        }

        if ! self.files.is_empty() {
            writeln!(w ,"  files:")?;
            for file in self.files.iter() {
                writeln!(w ,"  - {file}")?;
            }
        }

        if let Some(sha) = new_sha.or_else(|| self.sha.clone()) {
            writeln!(w ,"  sha: {}", sha)?;
        }
        Ok(())
    }

    pub fn name(&self) -> &String {
        &self.name
    }
}

impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // TODO: Display something more useful
        write!(f, "{:?}", self)
    }
}

impl FromYaml for Entry {
    fn from_yaml(yaml: &Yaml) -> Result<Self> {
        Ok(Self {
            name: yaml["name"].as_str().map(String::from).ok_or(Error::MissingName)?,
            cmd: yaml["cmd"].as_str().map(String::from).ok_or(Error::MissingCmd)?,
            sha: yaml["sha"].as_str().map(String::from),
            files: str_vec(&yaml["files"]),
            required_files: str_vec(&yaml["required_files"]),
        })
    }
}
