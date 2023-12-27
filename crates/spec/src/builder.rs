use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    path::Path,
};

use nck_hashing::SupportedHash;
use url::Url;

use crate::{Action, InvalidSpec, Spec};

/// A builder that creates a [`Spec`].
#[derive(Debug)]
pub struct SpecBuilder {
    name: String,
    outputs: BTreeSet<String>,
    actions: Vec<Action>,
}

impl SpecBuilder {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            outputs: BTreeSet::new(),
            actions: Vec::new(),
        }
    }

    pub fn build(&self) -> Result<Spec, InvalidSpec> {
        Spec::new(
            self.name.clone(),
            self.outputs.iter().cloned().collect(),
            self.actions.clone(),
        )
    }

    pub fn add_output(&mut self, output: impl ToString) -> &mut Self {
        self.outputs.insert(output.to_string());
        self
    }

    pub fn push_action(&mut self, action: Action) -> &mut Self {
        self.actions.push(action);
        self
    }

    pub fn push_actions(&mut self, actions: impl Iterator<Item = Action>) -> &mut Self {
        for action in actions {
            self.actions.push(action);
        }
        self
    }

    pub fn fetch(&mut self, source: Option<Url>, integrity: SupportedHash) -> &mut Self {
        self.actions.push(Action::fetch(source, integrity));
        self
    }

    pub fn exec(&mut self, path: impl AsRef<Path>, args: Vec<OsString>) -> &mut Self {
        self.actions.push(Action::exec(path, args));
        self
    }

    pub fn set<V: AsRef<OsStr>>(&mut self, name: impl AsRef<OsStr>, value: Option<V>) -> &mut Self {
        self.actions.push(Action::set(name, value));
        self
    }

    pub fn unset(&mut self, name: impl AsRef<OsStr>) -> &mut Self {
        self.actions.push(Action::set(name, None::<&str>));
        self
    }

    pub fn work_dir(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.actions.push(Action::work_dir(path));
        self
    }
}
