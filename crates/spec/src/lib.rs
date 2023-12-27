#![feature(never_type)]

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    os::unix::prelude::OsStrExt,
    path::{Path, PathBuf},
};

use nck_hashing::SupportedHash;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

mod builder;
pub use builder::*;

mod exec;
pub use exec::*;

mod ser;

/// A parsed spec file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "ser::Spec", try_from = "ser::Spec")]
pub struct Spec {
    name: String,
    outputs: Vec<String>,
    actions: Vec<Action>,
}

impl Spec {
    pub fn iterate_execution(&self) -> ExecutionIterator<'_> {
        ExecutionIterator {
            spec: &self.actions,
            rest: false,
            env: BTreeMap::new(),
            work_dir: PathBuf::from("/"),
        }
    }
}

#[derive(Error, Debug, Clone, Eq, PartialEq)]
pub enum InvalidSpec {
    #[error("invalid package name '{}'", _0)]
    InvalidPackageName(String),
    #[error("invalid output name '{}'", _0)]
    InvalidOutputName(String),
    /// An invalid environment variable name was provided.
    ///
    /// Environment variable names must:
    /// 1. Not contain `=`.
    /// 2. Start with an alphabetical character, or `_`.
    /// 3. Consist of only printable characters.
    #[error("invalid environment variable '{:?}'", _0)]
    InvalidEnvironmentVariableName(OsString),
    #[error("the final command in the spec is not an exec")]
    DanglingConfiguration,
    #[error("no outputs are declared")]
    NoOutputs,
}

pub fn validate_environment_variable_name(name: impl AsRef<OsStr>) -> Result<(), InvalidSpec> {
    let name = name.as_ref();
    for (i, b) in name.as_bytes().iter().enumerate() {
        match b {
            b'=' => {
                return Err(InvalidSpec::InvalidEnvironmentVariableName(
                    name.to_os_string(),
                ))
            }
            b'a'..=b'z' => {}
            b'A'..=b'Z' => {}
            b'_' => {}
            // Can technically be anything, but keep it printable to be reasonable.
            0x20..=0x7E if i != 0 => {}
            _ => {
                return Err(InvalidSpec::InvalidEnvironmentVariableName(
                    name.to_os_string(),
                ))
            }
        }
    }
    Ok(())
}

pub fn validate_output_name(name: impl AsRef<str>) -> Result<(), InvalidSpec> {
    let name = name.as_ref();

    if name.is_empty() {
        return Err(InvalidSpec::InvalidOutputName(name.to_string()));
    }

    if name.as_bytes().iter().any(|f| !f.is_ascii_lowercase()) {
        return Err(InvalidSpec::InvalidOutputName(name.to_string()));
    }

    Ok(())
}

pub fn validate_package_name(name: impl AsRef<str>) -> Result<(), InvalidSpec> {
    let name = name.as_ref();

    if name.is_empty() {
        return Err(InvalidSpec::InvalidPackageName(name.to_string()));
    }

    let b = name.as_bytes();
    if !b[0].is_ascii_lowercase() && !b[0].is_ascii_digit() {
        return Err(InvalidSpec::InvalidPackageName(name.to_string()));
    }

    for b in &b[1..] {
        match b {
            b'a'..=b'z' => {}
            b'A'..=b'Z' => {}
            b'0'..=b'9' => {}
            b'_' | b'-' | b'.' => {}
            _ => return Err(InvalidSpec::InvalidPackageName(name.to_string())),
        }
    }

    Ok(())
}

impl Spec {
    pub fn builder(name: impl ToString) -> SpecBuilder {
        SpecBuilder::new(name.to_string())
    }

    fn new(name: String, outputs: Vec<String>, actions: Vec<Action>) -> Result<Self, InvalidSpec> {
        let mut has_exec = false;

        validate_package_name(name.as_str())?;

        if outputs.is_empty() {
            return Err(InvalidSpec::NoOutputs);
        }

        for output in outputs.iter() {
            validate_output_name(output.as_str())?;
        }

        for action in actions.iter() {
            match action {
                Action::Exec(_) => has_exec = true,
                Action::Set(_) => has_exec = false,
                Action::WorkDir(_) => has_exec = false,
                Action::Fetch(_) => has_exec = false,
            }
        }

        if !has_exec {
            Err(InvalidSpec::DanglingConfiguration)
        } else {
            Ok(Spec {
                name,
                outputs,
                actions,
            })
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn outputs(
        &self,
    ) -> impl ExactSizeIterator<Item = &str> + DoubleEndedIterator<Item = &str> {
        self.outputs.iter().map(|f| f.as_str())
    }

    pub fn actions(
        &self,
    ) -> impl ExactSizeIterator<Item = &Action> + DoubleEndedIterator<Item = &Action> {
        self.actions.iter()
    }
}

/// A parsed action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "action")]
pub enum Action {
    #[serde(rename = "fetch")]
    /// Fetch a file.
    Fetch(Fetch),
    #[serde(rename = "exec")]
    /// Execute a binary.
    Exec(Exec),
    #[serde(rename = "set")]
    /// Set an environment variable.
    Set(Set),
    #[serde(rename = "work_dir")]
    /// Set the working directory for subsequent commands.
    WorkDir(WorkDir),
}

impl Action {
    pub fn fetch(source: Option<Url>, integrity: SupportedHash) -> Self {
        Self::Fetch(Fetch::new(source, integrity))
    }

    pub fn exec(path: impl AsRef<Path>, args: Vec<OsString>) -> Self {
        Self::Exec(Exec::new(path, args))
    }

    pub fn set<V: AsRef<OsStr>>(name: impl AsRef<OsStr>, value: Option<V>) -> Self {
        Self::Set(Set::new(name, value))
    }

    pub fn work_dir(path: impl AsRef<Path>) -> Self {
        Self::WorkDir(WorkDir::new(path))
    }
}

// Fetch a file from a URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "ser::Fetch", try_from = "ser::Fetch")]
pub struct Fetch {
    /// The URL when the archive can be fetched from.
    source: Option<Url>,
    /// The hash of the archive.
    integrity: SupportedHash,
}

impl Fetch {
    pub fn new(source: Option<Url>, integrity: SupportedHash) -> Self {
        Self { source, integrity }
    }

    pub fn source(&self) -> Option<&Url> {
        self.source.as_ref()
    }

    pub fn integrity(&self) -> SupportedHash {
        self.integrity
    }
}

/// Execute a binary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "ser::Exec", from = "ser::Exec")]
pub struct Exec {
    /// The path to the binary.
    path: PathBuf,
    /// The arguments to pass to the binary.
    args: Vec<OsString>,
}

impl Exec {
    pub fn new(path: impl AsRef<Path>, args: Vec<OsString>) -> Self {
        Self {
            path: path.as_ref().into(),
            args,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn args(
        &self,
    ) -> impl ExactSizeIterator<Item = &OsStr> + DoubleEndedIterator<Item = &OsStr> {
        self.args.iter().map(|f| f.as_os_str())
    }
}

#[derive(Error, Debug, Copy, Clone, Eq, PartialEq)]
#[error("invalid environment variable name")]
pub struct InvalidEnvironmentVariableName;

impl InvalidEnvironmentVariableName {
    /// Validate an environment variable name, returning an error result if it is invalid.
    pub fn validate(name: impl AsRef<[u8]>) -> Result<(), Self> {
        for (i, b) in name.as_ref().iter().enumerate() {
            match b {
                b'=' => return Err(Self),
                b'a'..=b'z' => {}
                b'A'..=b'Z' => {}
                b'_' => {}
                // Can technically be anything, but keep it printable to be reasonable.
                0x20..=0x7E if i != 0 => {}
                _ => return Err(Self),
            }
        }
        Ok(())
    }
}

/// Sets an environment variable value for subsequent commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "ser::Set", try_from = "ser::Set")]
pub struct Set {
    /// The name of the variable to set.
    name: OsString,
    /// The value of the variable.
    value: Option<OsString>,
}

impl Set {
    pub fn new<V: AsRef<OsStr>>(name: impl AsRef<OsStr>, value: Option<V>) -> Self {
        let name = name.as_ref();
        Self {
            name: name.to_os_string(),
            value: value.map(|f| f.as_ref().to_os_string()),
        }
    }

    pub fn name(&self) -> &OsStr {
        &self.name
    }

    pub fn value(&self) -> Option<&OsStr> {
        self.value.as_deref()
    }
}

/// Sets the current directory for subsequent commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "ser::WorkDir", try_from = "ser::WorkDir")]
pub struct WorkDir {
    /// The current directory to set.
    path: PathBuf,
}

impl WorkDir {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
