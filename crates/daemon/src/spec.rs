use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    ffi::OsString,
    fmt::Display,
    path::PathBuf,
    str::FromStr,
};

use nck_hashing::{StableHash, StableHasher, StableHasherExt, SupportedHash};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Default, Copy, Clone, Serialize, Deserialize)]
pub enum CompressionAlgorithm {
    #[default]
    #[serde(rename = "none")]
    None,
    #[serde(rename = "zstd")]
    Zstd,
}

#[derive(Debug, Error)]
#[error("unknown compression algorithm")]
pub struct UnknownCompressionAlgorithm;

impl FromStr for CompressionAlgorithm {
    type Err = UnknownCompressionAlgorithm;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" | "" => Ok(CompressionAlgorithm::None),
            "zstd" => Ok(CompressionAlgorithm::Zstd),
            _ => Err(UnknownCompressionAlgorithm),
        }
    }
}

impl Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompressionAlgorithm::Zstd => f.write_str("zstd"),
            CompressionAlgorithm::None => f.write_str("none"),
        }
    }
}

impl StableHash for CompressionAlgorithm {
    fn update<H: StableHasher>(&self, h: &mut H) {
        match self {
            CompressionAlgorithm::None => h.update_hash(0u8),
            CompressionAlgorithm::Zstd => h.update_hash(1u8),
        };
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum WireAction {
    #[serde(rename = "extract")]
    Extract {
        hash: String,
        destination: PathBuf,
        compression: CompressionAlgorithm,
    },
    #[serde(rename = "execute")]
    Execute {
        path: PathBuf,
        args: Vec<PathBuf>,
        environment: BTreeMap<PathBuf, PathBuf>,
    },
}

impl From<Action> for WireAction {
    fn from(value: Action) -> Self {
        match value {
            Action::Extract(hash, destination, compression) => WireAction::Extract {
                hash: hash.to_string(),
                destination,
                compression,
            },
            Action::Execute(path, args, environment) => WireAction::Execute {
                path,
                args: args.into_iter().map(|f| f.into()).collect(),
                environment: environment
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            },
        }
    }
}

#[derive(Debug, Error)]
enum ActionDeserializationError {
    #[error("duplicate environment variable: {:?}", _0)]
    DuplicateEnvironment(OsString),
    #[error("invalid hash: {:?}", _0)]
    InvalidHash(String),
}

impl TryFrom<WireAction> for Action {
    type Error = ActionDeserializationError;

    fn try_from(value: WireAction) -> Result<Self, Self::Error> {
        match value {
            WireAction::Extract {
                hash,
                destination,
                compression,
            } => {
                let hash = hash
                    .parse()
                    .map_err(|_| ActionDeserializationError::InvalidHash(hash))?;
                Ok(Action::Extract(hash, destination, compression))
            }
            WireAction::Execute {
                path,
                args,
                environment,
            } => {
                let mut env = BTreeMap::new();
                for (k, v) in environment {
                    let k = k.into_os_string();
                    let v = v.into_os_string();
                    if env.insert(k.clone(), v).is_some() {
                        return Err(ActionDeserializationError::DuplicateEnvironment(k));
                    }
                }
                Ok(Action::Execute(
                    path,
                    args.into_iter().map(|f| f.into_os_string()).collect(),
                    env,
                ))
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WireSpec {
    name: String,
    outputs: HashSet<String>,
    actions: Vec<WireAction>,
}

impl From<Spec> for WireSpec {
    fn from(value: Spec) -> Self {
        WireSpec {
            name: value.name.0,
            outputs: value.outputs.into_iter().map(|k| k.0).collect(),
            actions: value
                .actions
                .into_iter()
                .map(Into::<WireAction>::into)
                .collect(),
        }
    }
}

#[derive(Debug, Error)]
enum SpecDeserializationError {
    #[error("invalid package name: {:?}", _0)]
    PackageName(String),
    #[error("invalid output name: {:?}", _0)]
    OutputName(String),
    #[error("invalid action: {:?}", _0)]
    Action(ActionDeserializationError),
}

impl TryFrom<WireSpec> for Spec {
    type Error = SpecDeserializationError;

    fn try_from(value: WireSpec) -> Result<Self, Self::Error> {
        let name = value
            .name
            .parse()
            .map_err(|_| SpecDeserializationError::PackageName(value.name))?;

        let mut outputs = BTreeSet::new();
        for k in value.outputs {
            let k = k
                .parse()
                .map_err(|_| SpecDeserializationError::OutputName(k))?;
            outputs.insert(k);
        }

        let mut actions = Vec::new();
        for k in value.actions {
            let k = k.try_into().map_err(SpecDeserializationError::Action)?;
            actions.push(k);
        }

        Ok(Self {
            name,
            outputs,
            actions,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    Extract(SupportedHash, PathBuf, CompressionAlgorithm),
    Execute(PathBuf, Vec<OsString>, BTreeMap<OsString, OsString>),
}

impl StableHash for Action {
    fn update<H: StableHasher>(&self, h: &mut H) {
        match self {
            Action::Extract(a, b, c) => h
                .update_hash(1u8)
                .update_hash(a)
                .update_hash(b)
                .update_hash(c),
            Action::Execute(a, b, c) => h
                .update_hash(2u8)
                .update_hash(a)
                .update_hash(b)
                .update_hash(c),
        };
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// #[serde(into = "WireSpec", try_from = "WireSpec")]
pub struct Spec {
    name: PackageName,
    outputs: BTreeSet<OutputName>,
    actions: Vec<Action>,
}

impl Spec {
    pub fn new(name: PackageName) -> Spec {
        Spec {
            name,
            outputs: BTreeSet::new(),
            actions: Vec::new(),
        }
    }

    pub fn name(&self) -> &PackageName {
        &self.name
    }

    pub fn actions_iter(&self) -> impl ExactSizeIterator<Item = &Action> {
        self.actions.iter()
    }

    pub fn push_action(&mut self, action: Action) {
        self.actions.push(action)
    }

    pub fn push_actions<S: Into<Action>>(&mut self, arg: impl Iterator<Item = S>) {
        for item in arg {
            self.actions.push(item.into())
        }
    }

    pub fn add_output(&mut self, key: OutputName) {
        self.outputs.insert(key);
    }

    pub fn outputs_iter(&self) -> impl ExactSizeIterator<Item = &OutputName> {
        self.outputs.iter()
    }
}

impl StableHash for Spec {
    fn update<H: StableHasher>(&self, h: &mut H) {
        h.update_hash(&self.name)
            .update_iter(self.actions.iter())
            .update_iter(self.outputs.iter());
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

impl StableHash for OutputName {
    fn update<H: StableHasher>(&self, h: &mut H) {
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

impl StableHash for PackageName {
    fn update<H: StableHasher>(&self, h: &mut H) {
        h.update_hash(&self.0);
    }
}
