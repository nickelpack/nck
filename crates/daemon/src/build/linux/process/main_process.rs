use std::{
    collections::VecDeque,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    sync::Arc,
};

use nix::sched::CloneFlags;
use thiserror::Error;
use tokio::{io::unix::AsyncFd, sync::Mutex};

use crate::{
    build::linux::{
        fork,
        fs::bind,
        io::{AsyncMessageChannel as _, EmptyFds},
        user_ns::{LinuxIdMapping, UserNamespaceConfig, UserNamespaceError},
    },
    settings::{DaemonSettings, StoreSettings},
};

use super::{
    sandbox_process::SandboxResponse,
    zygote_process::{InitialRequest, SpawnError as ZygoteSpawnError},
    ChildProcess,
};

pub fn main_process(
    store_settings: StoreSettings,
    daemon_settings: DaemonSettings,
) -> anyhow::Result<PendingController> {
    let (parent, child) = UnixStream::pair()?;
    let cb = Box::new(move || match child.try_clone() {
        Ok(child) => super::zygote_process::zygote_process(store_settings.clone(), child),
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
    SpawnFailed(ZygoteSpawnError),
}

impl Controller {
    pub async fn spawn_async(&self) -> Result<Sandbox, SpawnError> {
        let s = self.0.clone().lock_owned().await;

        let mut user_namespace_config = UserNamespaceConfig::new()?;
        user_namespace_config
            .uid_mappings_mut()
            .push(LinuxIdMapping::new(
                0,
                s.daemon_settings.linux.sub_uid.min,
                1,
            ));
        user_namespace_config
            .gid_mappings_mut()
            .push(LinuxIdMapping::new(
                0,
                s.daemon_settings.linux.sub_gid.min,
                1,
            ));

        let (sandbox_peer, local_sandbox_peer) = UnixStream::pair()?;
        let sandbox_peer = Some(OwnedFd::from(sandbox_peer));
        local_sandbox_peer.set_nonblocking(true)?;
        let local_sandbox_peer = AsyncFd::new(local_sandbox_peer)?;

        s.socket
            .write_message(
                InitialRequest::Spawn {
                    user_namespace_config,
                },
                sandbox_peer.iter(),
            )
            .await?;

        let mut fds = VecDeque::with_capacity(1);
        match s.socket.read_message(&mut EmptyFds).await? {
            Ok(()) => {
                tracing::trace!("waiting for message");
                match local_sandbox_peer.read_message(&mut fds).await? {
                    SandboxResponse::EstablishStore(root) => {
                        let fd = fds.pop_front().unwrap();
                        tracing::trace!(?fd, ?root, "got message");
                        bind(
                            "/var/nck/store",
                            root.join("var/nck/store"),
                            None,
                            Some(fd.as_raw_fd()),
                        )
                        .unwrap();
                    }
                }
                Ok(Sandbox {
                    channel: local_sandbox_peer,
                })
            }
            Err(e) => Err(SpawnError::SpawnFailed(e)),
        }
    }
}

#[derive(Debug)]
pub struct Sandbox {
    channel: AsyncFd<UnixStream>,
}
