use std::path::PathBuf;

use nix::unistd::Pid;
use serde::{Deserialize, Serialize};

use crate::runtime::linux::channel::{ChannelError, PendingChannel};

// The direction here is from the caller's/remote's perspective.

#[derive(Debug, Serialize, Deserialize)]
pub enum OutboundZygoteMessage {
    Spawn { spec_path: PathBuf, socket: i32 },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum InboundZygoteMessage {}

pub fn zygote_process(
    peer: PendingChannel<InboundZygoteMessage, OutboundZygoteMessage>,
) -> anyhow::Result<()> {
    let peer = peer.into_peer()?;
    loop {
        let message = match peer.recv() {
            Err(ChannelError::BrokenChannel) => break,
            other => other?,
        };

        match message {
            OutboundZygoteMessage::Spawn { spec_path, socket } => {}
        }
    }
    Ok(())
}

fn spawn(spec_path: PathBuf, socket: i32) -> anyhow::Result<Pid> {
    Ok(Pid::from_raw(0))
}
