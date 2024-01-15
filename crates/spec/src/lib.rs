#![feature(never_type)]

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    os::unix::prelude::OsStrExt,
    path::{Path, PathBuf},
};

use nck_hashing::{StableHash, StableHashExt, StableHasherExt, SupportedHash, SupportedHasher};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    dependencies: Vec<Dependency>,
}

#[derive(Error, Debug, Clone, Eq, PartialEq)]
pub enum InvalidSpec {
    #[error("invalid package name '{}'", _0)]
    InvalidPackageName(String),
    #[error("invalid output name '{}'", _0)]
    InvalidOutputName(String),
    #[error("invalid hash '{}'", _0)]
    InvalidHash(String),
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
    pub fn iterate_execution(&self) -> ExecutionIterator<'_> {
        ExecutionIterator {
            spec: &self.actions,
            rest: false,
            env: BTreeMap::new(),
            work_dir: PathBuf::from("/"),
        }
    }

    pub fn paths<'a, 'b: 'a>(
        &'a self,
        store_directory: &'b impl AsRef<Path>,
        hasher: SupportedHasher,
    ) -> SpecPaths {
        let integrity = self.hash(hasher);
        SpecPaths {
            integrity,
            spec: self,
            store_directory: store_directory.as_ref(),
        }
    }

    pub fn builder(name: impl ToString) -> SpecBuilder {
        SpecBuilder::new(name.to_string())
    }

    fn new(
        name: String,
        outputs: Vec<String>,
        actions: Vec<Action>,
        dependencies: impl Iterator<Item = Dependency>,
    ) -> Result<Self, InvalidSpec> {
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
                Action::Link(_) => has_exec = false,
            }
        }

        if !has_exec {
            Err(InvalidSpec::DanglingConfiguration)
        } else {
            Ok(Spec {
                name,
                outputs,
                actions,
                dependencies: dependencies.collect(),
            })
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn outputs(&self) -> impl Iterator<Item = &str> + DoubleEndedIterator<Item = &str> {
        self.outputs.iter().map(|f| f.as_str())
    }

    pub fn actions(&self) -> impl Iterator<Item = &Action> + DoubleEndedIterator<Item = &Action> {
        self.actions.iter()
    }

    pub fn dependencies(
        &self,
    ) -> impl Iterator<Item = &Dependency> + DoubleEndedIterator<Item = &Dependency> {
        self.dependencies.iter()
    }
}

impl StableHash for Spec {
    fn update<H: nck_hashing::StableHasher>(&self, h: &mut H) {
        h.update_hash(&self.name).update_hash(&self.outputs);

        // The effects are what matters, not how (what order) they are described
        for action in self.iterate_execution() {
            h.update_hash(action);
        }
        h.update_hash(0u8);

        h.update_iter(self.dependencies.iter());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageDependency {
    name: String,
    output: String,
    integrity: SupportedHash,
}

impl PackageDependency {
    pub fn new(name: impl ToString, output: impl ToString, integrity: SupportedHash) -> Self {
        Self {
            name: name.to_string(),
            output: output.to_string(),
            integrity,
        }
    }

    pub fn path(&self, store_directory: impl AsRef<Path>) -> PathBuf {
        Self::format_path(
            store_directory,
            &self.name,
            self.integrity,
            Some(&self.output),
        )
    }

    pub fn spec_path(&self, store_directory: impl AsRef<Path>) -> PathBuf {
        Self::format_path(store_directory, &self.name, self.integrity, None::<&str>)
    }

    pub fn format_path<O: AsRef<str>>(
        store_directory: impl AsRef<Path>,
        name: impl AsRef<str>,
        integrity: SupportedHash,
        output: Option<O>,
    ) -> PathBuf {
        let name = name.as_ref();
        if let Some(output) = output {
            let output = output.as_ref();
            let f = format!("{name}-{integrity}-{output}");
            store_directory.as_ref().join(f)
        } else {
            let f = format!("{name}-{integrity}.spec");
            store_directory.as_ref().join(f)
        }
    }
}

impl StableHash for PackageDependency {
    fn update<H: nck_hashing::StableHasher>(&self, h: &mut H) {
        h.update_hash(&self.name)
            .update_hash(&self.output)
            .update_hash(self.integrity);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileDependency {
    integrity: SupportedHash,
}

impl FileDependency {
    pub fn new(integrity: SupportedHash) -> Self {
        Self { integrity }
    }

    pub fn path(&self, store_directory: impl AsRef<Path>) -> PathBuf {
        let integrity = &self.integrity;
        store_directory.as_ref().join(format!("files/{integrity}"))
    }
}

impl StableHash for FileDependency {
    fn update<H: nck_hashing::StableHasher>(&self, h: &mut H) {
        h.update_hash(self.integrity);
    }
}

/// A spec dependency.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dependency {
    Package(PackageDependency),
    File(FileDependency),
}

impl Dependency {
    pub fn package(name: impl ToString, output: impl ToString, integrity: SupportedHash) -> Self {
        Self::Package(PackageDependency::new(name, output, integrity))
    }

    pub fn file(integrity: SupportedHash) -> Self {
        Self::File(FileDependency::new(integrity))
    }

    pub fn path(&self, store_directory: impl AsRef<Path>) -> PathBuf {
        match self {
            Dependency::Package(p) => p.path(store_directory),
            Dependency::File(f) => f.path(store_directory),
        }
    }
}

impl StableHash for Dependency {
    fn update<H: nck_hashing::StableHasher>(&self, h: &mut H) {
        match self {
            Dependency::Package(p) => h.update_hash(1u8).update_hash(p),
            Dependency::File(f) => h.update_hash(2u8).update_hash(f),
        };
    }
}

#[derive(Debug)]
pub struct SpecPaths<'a, 'b> {
    integrity: SupportedHash,
    spec: &'a Spec,
    store_directory: &'b Path,
}

impl<'a, 'b> SpecPaths<'a, 'b> {
    pub fn spec(&self) -> PathBuf {
        PackageDependency::format_path(
            self.store_directory,
            &self.spec.name,
            self.integrity,
            None::<&str>,
        )
    }

    pub fn outputs<'c>(&'c self) -> SpecOutputPathsIterator<'a, 'b, 'c> {
        SpecOutputPathsIterator {
            paths: self,
            index: 0,
        }
    }
}

pub struct SpecOutputPathsIterator<'a, 'b, 'c> {
    paths: &'c SpecPaths<'a, 'b>,
    index: usize,
}

impl<'a, 'b, 'c> Iterator for SpecOutputPathsIterator<'a, 'b, 'c> {
    type Item = (String, PathBuf);

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.paths.spec.outputs.get(self.index)?;
        self.index += 1;
        Some((
            next.clone(),
            PackageDependency::format_path(
                self.paths.store_directory,
                &self.paths.spec.name,
                self.paths.integrity,
                Some(next),
            ),
        ))
    }
}

/// A parsed action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "action", tag = "action")]
pub enum Action {
    #[serde(rename = "exec")]
    /// Execute a binary.
    Exec(Exec),
    #[serde(rename = "set")]
    /// Set an environment variable.
    Set(Set),
    #[serde(rename = "work_dir")]
    /// Set the working directory for subsequent commands.
    WorkDir(WorkDir),
    /// Creates a link in the filesystem.
    #[serde(rename = "link")]
    Link(Link),
}

impl Action {
    pub fn exec(path: impl AsRef<Path>, args: Vec<OsString>) -> Self {
        Self::Exec(Exec::new(path, args))
    }

    pub fn set<V: AsRef<OsStr>>(name: impl AsRef<OsStr>, value: Option<V>) -> Self {
        Self::Set(Set::new(name, value))
    }

    pub fn work_dir(path: impl AsRef<Path>) -> Self {
        Self::WorkDir(WorkDir::new(path))
    }

    pub fn link(from: impl AsRef<Path>, to: impl AsRef<Path>, flags: LinkFlags) -> Self {
        Self::Link(Link::new(from, to, flags))
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

    pub fn args(&self) -> impl Iterator<Item = &OsStr> + DoubleEndedIterator<Item = &OsStr> {
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

bitflags::bitflags! {
    #[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
    pub struct LinkFlags: u16 {
        const EXECUTABLE = 0b01;
    }
}

/// Creates a symlink and any directories required to contain it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "ser::Link", try_from = "ser::Link")]
pub struct Link {
    /// The file to link from.
    from: PathBuf,
    /// The file to link to.
    to: PathBuf,
    /// The flags to apply to the link.
    flags: LinkFlags,
}

impl Link {
    pub fn new(from: impl AsRef<Path>, to: impl AsRef<Path>, flags: LinkFlags) -> Self {
        Self {
            from: from.as_ref().into(),
            to: to.as_ref().into(),
            flags,
        }
    }

    pub fn from(&self) -> &Path {
        &self.from
    }

    pub fn to(&self) -> &Path {
        &self.to
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use nck_hashing::SupportedHasher;
    use pretty_assertions::assert_eq;

    use crate::Spec;

    #[test]
    fn spec_paths() -> anyhow::Result<()> {
        let spec = Spec::builder("test-1.0.0")
            .add_output("outa")
            .add_output("outb")
            .link("/foo", "/test/foo", Some(super::LinkFlags::EXECUTABLE))
            .package(
                "foo-1.0",
                "dev",
                nck_hashing::SupportedHash::Blake3(*b"123456789012345678901234567890ab"),
            )
            .package(
                "bar-1.0",
                "out",
                nck_hashing::SupportedHash::Blake3(*b"123456789012345678901234567890ef"),
            )
            .file(nck_hashing::SupportedHash::Blake3(
                *b"123456789012345678901234567890cd",
            ))
            .exec("/test/foo", vec!["--help".into()])
            .build()?;

        let paths = spec.paths(&"/some/store", SupportedHasher::blake3());

        assert_eq!(
            PathBuf::from("/some/store/test-1.0.0-blake3-t7ujtbtj4sjaqkhffi5w5vpo2g3q5tem3geoygso5im37q5277ha.spec"),
            paths.spec()
        );

        let outputs: Vec<_> = paths.outputs().collect();
        assert_eq!(
            &[
                ("outa".to_string(), PathBuf::from("/some/store/test-1.0.0-blake3-t7ujtbtj4sjaqkhffi5w5vpo2g3q5tem3geoygso5im37q5277ha-outa")),
                ("outb".to_string(), PathBuf::from("/some/store/test-1.0.0-blake3-t7ujtbtj4sjaqkhffi5w5vpo2g3q5tem3geoygso5im37q5277ha-outb")),
            ],
            &outputs[..]
        );

        Ok(())
    }
}
