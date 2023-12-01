use std::{ffi::OsStr, path::Path};

use bitcode::{Decode, Encode};
use nix::unistd::{Gid, Pid, Uid};

// This protocol is intentionally extremely simple. Something like remoc would need an entire tokio runtime, which
// defeats the purpose of a zygote: a process that is as small as possible, which can be quickly forked/cloned.

#[derive(Debug, Clone, Encode, Decode)]
pub enum Request {
    Spawn(SpawnRequest),
}

impl From<SpawnRequest> for Request {
    fn from(value: SpawnRequest) -> Self {
        Self::Spawn(value)
    }
}

#[derive(Debug, Clone, Encode, Decode)]
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

#[derive(Debug, Clone, Encode, Decode)]
pub struct SpawnResponse {
    pid: i32,
    sandbox_path: Box<[u8]>,
    socket_path: Box<[u8]>,
}

impl SpawnResponse {
    pub fn new(pid: Pid, sandbox_path: impl AsRef<Path>, socket_path: impl AsRef<Path>) -> Self {
        Self {
            pid: pid.as_raw(),
            sandbox_path: sandbox_path.as_ref().as_os_str().as_encoded_bytes().into(),
            socket_path: socket_path.as_ref().as_os_str().as_encoded_bytes().into(),
        }
    }

    pub fn pid(&self) -> Pid {
        Pid::from_raw(self.pid)
    }

    pub fn sandbox_path(&self) -> &Path {
        Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&self.sandbox_path) })
    }

    pub fn socket_path(&self) -> &Path {
        Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&self.socket_path) })
    }
}
