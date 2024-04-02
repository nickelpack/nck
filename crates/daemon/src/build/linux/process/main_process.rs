use std::{
    os::{fd::OwnedFd, unix::net::UnixStream},
    sync::Arc,
};

use nix::{sched::CloneFlags, unistd::getgid};
use thiserror::Error;
use tokio::{io::unix::AsyncFd, sync::Mutex};

use crate::{
    build::linux::{
        fork,
        io::AsyncMessageChannel as _,
        proc::ChildProcess,
        user_ns::{LinuxIdMapping, UserNamespaceError},
    },
    settings::DaemonSettings,
};

use super::{zygote_process::InitialRequest, SandboxConfig};

pub fn main_process(daemon_settings: DaemonSettings) -> anyhow::Result<PendingController> {
    let (parent, child) = UnixStream::pair()?;
    let cb = Box::new(move || match child.try_clone() {
        Ok(child) => super::zygote_process::zygote_process(child),
        Err(e) => Err(anyhow::anyhow!("failed to clone child socket {:?}", e)),
    });
    tracing::debug!("starting zygote");
    let zygote: ChildProcess = fork::clone(cb, CloneFlags::empty())?.into();

    Ok(PendingController {
        daemon_settings,
        _zygote: zygote,
        socket: parent,
    })
}

/// A controller that has not been activated.
///
/// This primarily exists to avoid interacting with tokio prior to creating the zygote. Forking/cloning with threads
/// is UB.
#[derive(Debug)]
pub struct PendingController {
    _zygote: ChildProcess,
    socket: UnixStream,
    daemon_settings: DaemonSettings,
}

impl PendingController {
    pub async fn into_controller(self) -> anyhow::Result<Controller> {
        self.socket.set_nonblocking(true)?;
        Ok(Controller(Arc::new(Mutex::new(ControllerState {
            _zygote: self._zygote,
            socket: AsyncFd::new(self.socket)?,
            daemon_settings: self.daemon_settings,
        }))))
    }
}

#[derive(Debug)]
struct ControllerState {
    _zygote: ChildProcess,
    socket: AsyncFd<UnixStream>,
    daemon_settings: DaemonSettings,
}

#[derive(Debug)]
pub struct Controller(Arc<Mutex<ControllerState>>);

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error(transparent)]
    IO(#[from] std::io::Error),
    #[error(transparent)]
    UserNamespace(#[from] UserNamespaceError),
    #[error("spawn failed in child process")]
    SpawnFailed(super::zygote_process::SpawnError),
}

impl Controller {
    pub async fn spawn_async(&self, mut config: SandboxConfig) -> Result<Sandbox, SpawnError> {
        let s = self.0.clone().lock_owned().await;

        config
            .namespace
            .uid_mappings_mut()
            .push(LinuxIdMapping::new(
                0,
                s.daemon_settings.linux.sub_uid.min,
                1,
            ));
        config
            .namespace
            .gid_mappings_mut()
            .push(LinuxIdMapping::new(0, getgid().as_raw(), 1));

        let (sandbox_peer, local_sandbox_peer) = UnixStream::pair()?;
        let sandbox_peer = Some(OwnedFd::from(sandbox_peer));
        local_sandbox_peer.set_nonblocking(true)?;
        let local_sandbox_peer = AsyncFd::new(local_sandbox_peer)?;

        s.socket
            .write_message(InitialRequest::Spawn { config }, sandbox_peer.iter())
            .await?;

        Ok(Sandbox {
            channel: local_sandbox_peer,
        })
    }
}

#[derive(Debug)]
pub struct Sandbox {
    channel: AsyncFd<UnixStream>,
}
