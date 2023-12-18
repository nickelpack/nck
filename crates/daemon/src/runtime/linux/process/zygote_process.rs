use std::path::PathBuf;

use anyhow::bail;
use nix::sched::CloneFlags;
use serde::{Deserialize, Serialize};

use crate::runtime::linux::{
    channel::{self, ChannelError, PendingChannel},
    fork,
    user_ns::UserNamespaceConfig,
};

use super::{
    sandbox_process::{SandboxRequest, SandboxResponse},
    supervisor_process::{SupervisorRequest, SupervisorResponse},
    ChildProcess,
};

// The direction here is from the caller's/remote's perspective.

#[derive(Debug, Serialize, Deserialize)]
pub enum ZygoteRequest {
    Spawn {
        user_namespace_config: UserNamespaceConfig,
        spec_path: PathBuf,
        sandbox_peer: PendingChannel<SandboxResponse, SandboxRequest>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ZygoteResponse {
    SpawnSuccess,
    SpawnFailure,
}

pub fn zygote_process(peer: PendingChannel<ZygoteResponse, ZygoteRequest>) -> anyhow::Result<()> {
    let peer = peer.into_peer()?;
    loop {
        let message = match peer.recv() {
            Err(ChannelError::BrokenChannel) => break,
            other => other?,
        };

        match message {
            ZygoteRequest::Spawn {
                user_namespace_config,
                spec_path,
                sandbox_peer,
            } => match spawn(user_namespace_config, spec_path, sandbox_peer) {
                Ok(_) => peer.send(ZygoteResponse::SpawnSuccess)?,
                Err(error) => {
                    tracing::error!(?error, "failed to spawn supervisor process");
                    peer.send(ZygoteResponse::SpawnSuccess)?;
                }
            },
        }
    }
    Ok(())
}

fn spawn(
    user_namespace_config: UserNamespaceConfig,
    spec_path: PathBuf,
    sandbox_peer: PendingChannel<SandboxResponse, SandboxRequest>,
) -> anyhow::Result<()> {
    let (supervisor_peer, local_supervisor_peer) = channel::unix_pair()?;

    let cb = {
        let supervisor_peer = supervisor_peer.clone();
        let sandbox_peer = sandbox_peer.clone();
        let spec_path = spec_path.clone();
        Box::new(move || {
            super::supervisor_process::supervisor_process(
                supervisor_peer.clone(),
                sandbox_peer.clone(),
                spec_path.clone(),
            )
        })
    };

    let pid: ChildProcess = fork::clone(
        cb,
        CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWUSER,
    )?
    .into();

    let supervisor_peer = local_supervisor_peer.into_peer()?;
    user_namespace_config.write_mappings(pid.inner())?;
    supervisor_peer.send(SupervisorRequest::UserMapped)?;

    if let SupervisorResponse::Failed = supervisor_peer.recv()? {
        bail!("supervisor process failed");
    }
    pid.forget();

    Ok(())
}
