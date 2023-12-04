use std::path::Path;

use nix::unistd::{Gid, Pid, Uid};
use speedy::{Readable, Writable};

use crate::current::proto::SerOsString;

// This protocol is intentionally extremely simple. Something like remoc would need an entire tokio runtime, which
// defeats the purpose of a zygote: a process that is as small as possible, which can be quickly forked/cloned.

#[derive(Debug, Clone, Readable, Writable)]
pub enum Request {
    Spawn(SpawnRequest),
}

impl From<SpawnRequest> for Request {
    fn from(value: SpawnRequest) -> Self {
        Self::Spawn(value)
    }
}

#[derive(Debug, Clone, Readable, Writable)]
pub struct SpawnRequest {
    name: String,
    root_uid: u32,
    root_gid: u32,
    user_uid: u32,
    user_gid: u32,
}

impl SpawnRequest {
    pub fn new(name: &str, root_uid: Uid, root_gid: Gid, user_uid: Uid, user_gid: Gid) -> Self {
        Self {
            name: name.to_string(),
            root_uid: root_uid.as_raw(),
            root_gid: root_gid.as_raw(),
            user_uid: user_uid.as_raw(),
            user_gid: user_gid.as_raw(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn root_uid(&self) -> Uid {
        Uid::from_raw(self.root_uid)
    }

    pub fn root_gid(&self) -> Gid {
        Gid::from_raw(self.root_gid)
    }

    pub fn user_uid(&self) -> Uid {
        Uid::from_raw(self.user_uid)
    }

    pub fn user_gid(&self) -> Gid {
        Gid::from_raw(self.user_gid)
    }
}

#[derive(Debug, Clone, Readable, Writable)]
pub struct SpawnResponse {
    pid: i32,
    sandbox_path: SerOsString,
    socket_path: SerOsString,
}

impl SpawnResponse {
    pub fn new(pid: Pid, sandbox_path: impl AsRef<Path>, socket_path: impl AsRef<Path>) -> Self {
        Self {
            pid: pid.as_raw(),
            sandbox_path: sandbox_path.as_ref().into(),
            socket_path: socket_path.as_ref().into(),
        }
    }

    pub fn pid(&self) -> Pid {
        Pid::from_raw(self.pid)
    }

    pub fn sandbox_path(&self) -> &Path {
        self.sandbox_path.as_ref()
    }

    pub fn socket_path(&self) -> &Path {
        self.socket_path.as_ref()
    }
}
