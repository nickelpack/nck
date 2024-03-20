use std::{
    collections::VecDeque,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::PathBuf,
};

use anyhow::{anyhow, bail};
use nix::sched::CloneFlags;
use serde::{Deserialize, Serialize};

use crate::{
    build::linux::{
        fork,
        io::{ChannelError, EmptyFds, MessageChannel},
        user_ns::UserNamespaceConfig,
    },
    settings::StoreSettings,
};

use super::{
    supervisor_process::{SupervisorRequest, SupervisorResponse},
    ChildProcess,
};

// The direction here is from the caller's/remote's perspective.

#[derive(Debug, Serialize, Deserialize)]
pub enum ZygoteRequest {
    Spawn {
        user_namespace_config: UserNamespaceConfig,
        spec_path: PathBuf,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ZygoteResponse {
    SpawnSuccess,
    SpawnFailure,
}

pub fn zygote_process(config: StoreSettings, peer: UnixStream) -> anyhow::Result<()> {
    tracing::info!("zygote started");
    let mut fds = VecDeque::new();
    loop {
        let message = match peer.read_message(&mut fds) {
            Err(e) if e.is_closed_channel() => break,
            other => other?,
        };

        let response = match message {
            ZygoteRequest::Spawn {
                user_namespace_config,
                spec_path,
            } => match spawn(
                config.clone(),
                user_namespace_config,
                spec_path,
                fds.pop_front(),
            ) {
                Ok(_) => ZygoteResponse::SpawnSuccess,
                Err(error) => {
                    tracing::error!(?error, "failed to spawn supervisor process");
                    ZygoteResponse::SpawnFailure
                }
            },
        };

        fds.clear();
        peer.write_message(response, EmptyFds)?;
    }
    Ok(())
}

fn spawn(
    config: StoreSettings,
    user_namespace_config: UserNamespaceConfig,
    spec_path: PathBuf,
    mut sandbox_peer: Option<OwnedFd>,
) -> anyhow::Result<()> {
    let sandbox_peer = sandbox_peer.take().ok_or_else(|| anyhow!("missing fd"))?;
    let (supervisor_peer, local_supervisor_peer) = UnixStream::pair()?;

    let cb = {
        let spec_path = spec_path.clone();
        let config = config.clone();
        Box::new(move || {
            super::supervisor_process::supervisor_process(
                config.clone(),
                &supervisor_peer,
                sandbox_peer.try_clone()?,
                spec_path.clone(),
            )
        })
    };

    let pid: ChildProcess = fork::clone(
        cb,
        CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWUSER,
    )?
    .into();

    if let Err(error) = user_namespace_config.write_mappings(pid.inner()) {
        if let Err(error) = local_supervisor_peer.write_message(SupervisorRequest::Exit, EmptyFds) {
            tracing::warn!(?error, "failed to inform supervisor to exit");
        }
        return Err(error.into());
    }

    local_supervisor_peer.write_message(SupervisorRequest::UserMapped, EmptyFds)?;

    if let SupervisorResponse::Failed = local_supervisor_peer.read_message(&mut EmptyFds)? {
        bail!("supervisor process failed");
    }
    pid.forget();
    tracing::info!("started supervisor process");

    Ok(())
}
