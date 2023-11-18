#[cfg(target_os = "linux")]
pub mod linux;

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
pub use linux as flavor;

use anyhow::Result;

#[derive(Debug, Default, Clone)]
pub struct SandboxBuilder {
    root: Option<PathBuf>,
    shared_filesystem: bool,
    shared_resources: bool,
    shared_ipc: bool,
    shared_networking: bool,
    shared_processes: bool,
    shared_users: bool,
    propagate_file_descriptors: bool,
    propagate_debugger: bool,
    propagate_signals: bool,
    hostname: Option<String>,
    uid: Option<u32>,
    gid: Option<u32>,
    suppl: Vec<u32>,
}

impl SandboxBuilder {
    pub async fn build(mut self) -> Result<flavor::Sandbox> {
        flavor::apply_assumptions(&mut self);
        flavor::start(self).await
    }

    pub fn apply_assumptions(&self) -> SandboxBuilder {
        let mut new = self.clone();
        flavor::apply_assumptions(&mut new);
        new
    }

    pub fn set_filesystem_isolated(&mut self, isolated: bool) -> &mut Self {
        self.shared_filesystem = !isolated;
        self
    }

    pub fn filesystem_isolated(&self) -> bool {
        !self.shared_filesystem
    }

    pub fn set_resources_isolated(&mut self, isolated: bool) -> &mut Self {
        self.shared_resources = !isolated;
        self
    }

    pub fn resources_isolated(&self) -> bool {
        !self.shared_resources
    }

    pub fn set_ipc_isolated(&mut self, isolated: bool) -> &mut Self {
        self.shared_ipc = !isolated;
        self
    }

    pub fn ipc_isolated(&self) -> bool {
        !self.shared_ipc
    }

    pub fn set_networking_isolated(&mut self, isolated: bool) -> &mut Self {
        self.shared_networking = !isolated;
        self
    }

    pub fn networking_isolated(&self) -> bool {
        !self.shared_networking
    }

    pub fn set_processes_isolated(&mut self, isolated: bool) -> &mut Self {
        self.shared_processes = !isolated;
        self
    }

    pub fn processes_isolated(&self) -> bool {
        !self.shared_processes
    }

    pub fn set_users_isolated(&mut self, isolated: bool) -> &mut Self {
        self.shared_users = !isolated;
        self
    }

    pub fn users_isolated(&self) -> bool {
        !self.shared_users
    }

    pub fn set_file_descriptors_propagated(&mut self, propagated: bool) -> &mut Self {
        self.propagate_file_descriptors = propagated;
        self
    }

    pub fn file_descriptors_propagated(&self) -> bool {
        self.propagate_file_descriptors
    }

    pub fn set_debugger_propagated(&mut self, propagated: bool) -> &mut Self {
        self.propagate_debugger = propagated;
        self
    }

    pub fn debugger_propagated(&self) -> bool {
        self.propagate_debugger
    }

    pub fn set_signals_propagated(&mut self, propagated: bool) -> &mut Self {
        self.propagate_signals = propagated;
        self
    }

    pub fn signals_propagated(&self) -> bool {
        self.propagate_signals
    }

    pub fn set_root<T: AsRef<Path>>(&mut self, path: Option<T>) -> &mut Self {
        self.root = path.map(|f| f.as_ref().to_path_buf());
        self
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_ref().map(|v| v.as_path())
    }

    pub fn set_hostname<T: ToString>(&mut self, hostname: Option<T>) -> &mut Self {
        self.hostname = hostname.map(|f| f.to_string());
        self
    }

    pub fn hostname(&self) -> Option<&str> {
        self.hostname.as_ref().map(|v| v.as_str())
    }

    pub fn set_uid(&mut self, uid: Option<u32>) -> &mut Self {
        self.uid = uid;
        self
    }

    pub fn uid(&self) -> Option<u32> {
        self.uid
    }

    pub fn set_gid(&mut self, gid: Option<u32>) -> &mut Self {
        self.gid = gid;
        self
    }

    pub fn gid(&self) -> Option<u32> {
        self.gid
    }

    pub fn change_supplementary_gids(&mut self, f: impl FnOnce(&mut Vec<u32>)) -> &mut Self {
        f(&mut self.suppl);
        self
    }

    pub fn supplementary_gids_mut(&mut self) -> &mut Vec<u32> {
        &mut self.suppl
    }

    pub fn supplementary_gids(&self) -> &[u32] {
        &self.suppl
    }
}
