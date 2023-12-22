use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    path::{Path, PathBuf},
    str::FromStr,
};

use nck_core::hashing::{DeterministicHash, DeterministicHasher};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug)]
pub struct SpecFile<'a> {
    source: &'a Path,
    dest: &'a Path,
    executable: bool,
}

impl<'a> SpecFile<'a> {
    pub fn source(&self) -> &'a Path {
        self.source
    }

    pub fn dest(&self) -> &'a Path {
        self.dest
    }

    pub fn executable(&self) -> bool {
        self.executable
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Spec {
    name: PackageName,
    entry: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    impure_env: BTreeSet<String>,
    outputs: BTreeMap<OutputName, PathBuf>,
    copy: BTreeMap<PathBuf, (PathBuf, bool)>,
}

impl Spec {
    pub fn new(name: PackageName, entry: impl AsRef<Path>) -> Spec {
        Spec {
            name,
            entry: entry.as_ref().to_path_buf(),
            args: Vec::new(),
            env: BTreeMap::new(),
            impure_env: BTreeSet::new(),
            outputs: BTreeMap::new(),
            copy: BTreeMap::new(),
        }
    }

    pub fn name(&self) -> &PackageName {
        &self.name
    }

    pub fn entry(&self) -> &Path {
        self.entry.as_path()
    }

    pub fn args_iter(&self) -> impl ExactSizeIterator<Item = &str> {
        self.args.iter().map(|v| v.as_str())
    }

    pub fn push_arg(&mut self, arg: impl AsRef<str>) {
        self.args.push(arg.as_ref().to_string())
    }

    pub fn push_args<S: AsRef<str>>(&mut self, arg: impl Iterator<Item = S>) {
        for item in arg {
            self.args.push(item.as_ref().to_string())
        }
    }

    pub fn env_iter(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.env.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn set_env(&mut self, key: impl AsRef<str>, value: impl AsRef<str>) {
        let key = key.as_ref();
        let value = value.as_ref();
        self.env.insert(key.to_string(), value.to_string());
    }

    pub fn update_env<R: AsRef<str>>(
        &mut self,
        key: impl AsRef<str>,
        value: impl FnOnce(&str) -> R,
    ) {
        let key = key.as_ref();

        self.env
            .entry(key.to_string())
            .and_modify(|k| {
                let r = value(k.as_str());
                *k = r.as_ref().to_string();
            })
            .or_default();
    }

    pub fn include_impure_env(&mut self, env: impl AsRef<str>) {
        self.impure_env.insert(env.as_ref().to_string());
    }

    pub fn impure_env_iter(&self) -> impl ExactSizeIterator<Item = &str> {
        self.impure_env.iter().map(|v| v.as_str())
    }

    pub fn add_output(&mut self, key: OutputName) {
        self.outputs.insert(key, PathBuf::new());
    }

    pub fn set_output(&mut self, key: OutputName, value: impl AsRef<Path>) {
        let value = value.as_ref();
        self.outputs.insert(key, value.to_path_buf());
    }

    pub fn outputs_iter(&self) -> impl ExactSizeIterator<Item = (&OutputName, &Path)> {
        self.outputs.iter().map(|(k, v)| (k, v.as_path()))
    }

    pub fn copy_file(
        &mut self,
        source: impl AsRef<Path>,
        dest: impl AsRef<Path>,
        executable: bool,
    ) {
        let source = source.as_ref();
        let dest = dest.as_ref();
        self.copy
            .insert(dest.to_path_buf(), (source.to_path_buf(), executable));
    }

    pub fn copy_iter(&self) -> impl ExactSizeIterator<Item = SpecFile<'_>> {
        self.copy.iter().map(|(k, v)| SpecFile {
            source: v.0.as_path(),
            dest: k.as_path(),
            executable: v.1,
        })
    }
}

impl DeterministicHash for Spec {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_hash(&self.entry)
            .update_hash(&self.args)
            .update_hash(&self.env)
            .update_hash(&self.impure_env)
            .update_hash(&self.copy)
            // This would cause a catch-22.
            .update_iter(self.outputs.keys());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputName(String);

impl Display for OutputName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Error)]
#[error("invalid output name")]
pub struct ParseOutputNameError;

impl FromStr for OutputName {
    type Err = ParseOutputNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseOutputNameError);
        }

        for v in s.chars() {
            match v {
                'a'..='z' => {}
                _ => return Err(ParseOutputNameError),
            }
        }
        Ok(OutputName(s.to_string()))
    }
}

impl DeterministicHash for OutputName {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_hash(&self.0);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageName(String);

impl Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Error)]
#[error("invalid package name")]
pub struct ParsePackageNameError;

impl FromStr for PackageName {
    type Err = ParsePackageNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParsePackageNameError);
        }

        let b = s.as_bytes();

        match b[0] {
            b'a'..=b'z' => {}
            b'0'..=b'9' => {}
            b'_' => {}
            _ => return Err(ParsePackageNameError),
        }

        for v in &b[1..] {
            match v {
                b'a'..=b'z' => {}
                b'0'..=b'9' => {}
                b'-' | b'_' | b'.' => {}
                _ => return Err(ParsePackageNameError),
            }
        }
        Ok(PackageName(s.to_string()))
    }
}

impl DeterministicHash for PackageName {
    fn update<H: DeterministicHasher>(&self, h: &mut H) {
        h.update_hash(&self.0);
    }
}
