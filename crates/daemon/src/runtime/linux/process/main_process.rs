use std::path::PathBuf;

use nix::sched::CloneFlags;

use crate::runtime::linux::{
    channel::{self, AsyncChannel, PendingChannel},
    fork,
    user_ns::{LinuxIdMapping, UserNamespaceConfig},
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

#[derive(Debug)]
pub struct PendingController {
    _zygote: ChildProcess,
    channel: PendingChannel<ZygoteRequest, ZygoteResponse>,
}

impl PendingController {
    pub async fn into_controller(self) -> anyhow::Result<Controller> {
        Ok(Controller {
            _zygote: self._zygote,
            channel: self.channel.into_peer_async().await?,
        })
    }
}

#[derive(Debug)]
pub struct Controller {
    _zygote: ChildProcess,
    channel: AsyncChannel<ZygoteRequest, ZygoteResponse>,
}

impl Controller {
    pub async fn spawn_async(&self) -> anyhow::Result<Sandbox> {
        let mut user_namespace_config = UserNamespaceConfig::new()?;
        user_namespace_config
            .uid_mappings_mut()
            .push(LinuxIdMapping::new(0, 10000, 1));
        user_namespace_config
            .gid_mappings_mut()
            .push(LinuxIdMapping::new(0, 10000, 1));

        let (sandbox_peer, local_sandbox_peer) = channel::unix_pair()?;
        let sandbox_channel = local_sandbox_peer.into_peer_async().await?;

        self.channel
            .send(ZygoteRequest::Spawn {
                user_namespace_config,
                spec_path: PathBuf::new(),
                sandbox_peer,
            })
            .await?;

        match self.channel.recv().await? {
            ZygoteResponse::SpawnSuccess => Ok(Sandbox {
                channel: sandbox_channel,
            }),
            ZygoteResponse::SpawnFailure => Err(anyhow::anyhow!("failed to start a sandbox")),
        }
    }
}

#[derive(Debug)]
pub struct Sandbox {
    channel: AsyncChannel<SandboxRequest, SandboxResponse>,
}
