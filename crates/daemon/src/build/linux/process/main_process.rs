use std::{path::Path, sync::Arc};

use nix::sched::CloneFlags;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::build::linux::{
    channel::{self, AsyncChannel, ChannelError, PendingChannel, PendingChannelError},
    fork,
    user_ns::{LinuxIdMapping, UserNamespaceConfig, UserNamespaceError},
};

use super::{
    sandbox_process::{SandboxRequest, SandboxResponse},
    zygote_process::{ZygoteRequest, ZygoteResponse},
    ChildProcess,
};

pub fn main_process() -> anyhow::Result<PendingController> {
    let (parent, child) = channel::unix_pair()?;
    let cb = Box::new(move || super::zygote_process::zygote_process(child.clone()));
    let zygote: ChildProcess = fork::clone(cb, CloneFlags::empty())?.into();

    Ok(PendingController {
        _zygote: zygote,
        channel: parent,
    })
}

/// A controller that has not been activated.
///
/// This primarily exists to avoid interacting with tokio prior to creating the zygote. This is to keep the address
/// space relatively small so that zygote clones are fast.
#[derive(Debug)]
pub struct PendingController {
    _zygote: ChildProcess,
    channel: PendingChannel<ZygoteRequest, ZygoteResponse>,
}

impl PendingController {
    pub async fn into_controller(self) -> anyhow::Result<Controller> {
        Ok(Controller(Arc::new(Mutex::new(ControllerState {
            _zygote: self._zygote,
            channel: self.channel.into_peer_async().await?,
        }))))
    }
}

#[derive(Debug)]
struct ControllerState {
    _zygote: ChildProcess,
    channel: AsyncChannel<ZygoteRequest, ZygoteResponse>,
}

#[derive(Debug)]
pub struct Controller(Arc<Mutex<ControllerState>>);

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error(transparent)]
    IO(#[from] std::io::Error),
    #[error(transparent)]
    UserNamespace(#[from] UserNamespaceError),
    #[error(transparent)]
    PendingChannel(#[from] PendingChannelError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error("spawn failed in child process")]
    SpawnFailed,
}

impl Controller {
    pub async fn spawn_async(&self, path: impl AsRef<Path>) -> Result<Sandbox, SpawnError> {
        let s = self.0.clone().lock_owned().await;

        let mut user_namespace_config = UserNamespaceConfig::new()?;
        user_namespace_config
            .uid_mappings_mut()
            .push(LinuxIdMapping::new(0, 10000, 1));
        user_namespace_config
            .gid_mappings_mut()
            .push(LinuxIdMapping::new(0, 10000, 1));

        let (sandbox_peer, local_sandbox_peer) = channel::unix_pair()?;
        let sandbox_channel = local_sandbox_peer.into_peer_async().await?;

        s.channel
            .send(ZygoteRequest::Spawn {
                user_namespace_config,
                spec_path: path.as_ref().to_path_buf(),
                sandbox_peer,
            })
            .await?;

        match s.channel.recv().await? {
            ZygoteResponse::SpawnSuccess => Ok(Sandbox {
                channel: sandbox_channel,
            }),
            ZygoteResponse::SpawnFailure => Err(SpawnError::SpawnFailed),
        }
    }
}

#[derive(Debug)]
pub struct Sandbox {
    channel: AsyncChannel<SandboxRequest, SandboxResponse>,
}
