use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use flume::{Receiver, Sender};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixListener, UnixStream,
    },
};

use crate::DisconnectedError;

use super::{proto, PidContainer};

pub(crate) async fn start(
    pid: super::PidContainer,
    working_dir: tempfile::TempDir,
    child_socket: UnixStream,
    listener: UnixListener,
) -> Result<Sandbox> {
    let (read, write) = child_socket.into_split();
    let (send_to_child, receive_from_parent) = flume::bounded(128);
    let (send_to_parent, receive_from_child) = flume::bounded(128);

    tokio::spawn(async {
        if let Err(e) = send_worker(write, receive_from_parent).await {
            tracing::error!("send worker task crashed: {:?}", e);
        }
    });
    tokio::spawn(async {
        if let Err(e) = receive_worker(read, send_to_parent).await {
            tracing::error!("receive worker thread crashed: {:?}", e);
        }
    });

    Ok(Sandbox {
        state: Arc::new(State {
            listener,
            pid,
            working_dir,
        }),
        send_to_child,
        receive_from_child,
    })
}

async fn send_worker(
    mut send: OwnedWriteHalf,
    outgoing: Receiver<proto::ChildMessage>,
) -> Result<()> {
    loop {
        let message = if let Ok(message) = outgoing.recv_async().await {
            message
        } else {
            break Ok(());
        };

        let buffer = bitcode::encode(&message)
            .with_context(|| "while encoding a message for the child process")?;
        let crc = proto::crc(&buffer).to_le_bytes();
        let len = (buffer.len() as u64).to_le_bytes();

        send.write_all(&len)
            .await
            .with_context(|| "while writing the length header to the child process")?;
        send.write_all(&crc)
            .await
            .with_context(|| "while writing the CRC to the child process")?;
        send.write_all(&buffer)
            .await
            .with_context(|| "while writing the data to the child process")?;
    }
}

async fn receive_worker(
    mut read: OwnedReadHalf,
    incoming: Sender<proto::ParentMessage>,
) -> Result<()> {
    let mut big_buffer = Vec::new();
    loop {
        let mut buffer = [0u8; 8];
        read.read_exact(&mut buffer)
            .await
            .with_context(|| "while reading the length header from the child process")?;
        let len = u64::from_le_bytes(buffer) as usize;
        read.read_exact(&mut buffer)
            .await
            .with_context(|| "while reading the CRC header from the child process")?;
        let crc = u64::from_le_bytes(buffer);

        big_buffer.resize(len, 0u8);

        read.read_exact(&mut big_buffer)
            .await
            .with_context(|| "while reading the data from the child process")?;

        if crc != proto::crc(&mut big_buffer) {
            return Err(anyhow::anyhow!("invalid data received from child"))?;
        }

        let message = bitcode::decode(&big_buffer)
            .with_context(|| "while decoding the messag from the child")?;

        if incoming.send_async(message).await.is_err() {
            break Ok(());
        }
    }
}

#[derive(Debug)]
struct State {
    pid: PidContainer,
    working_dir: TempDir,
    listener: UnixListener,
}

#[derive(Debug, Clone)]
pub struct Sandbox {
    state: Arc<State>,
    send_to_child: Sender<proto::ChildMessage>,
    receive_from_child: Receiver<proto::ParentMessage>,
}

impl Sandbox {
    pub async fn recv(&mut self) -> Result<proto::ParentMessage, DisconnectedError> {
        self.receive_from_child
            .recv_async()
            .await
            .map_err(|_| DisconnectedError)
    }

    pub async fn send(&mut self, message: proto::ChildMessage) -> Result<(), DisconnectedError> {
        self.send_to_child
            .send_async(message)
            .await
            .map_err(|_| DisconnectedError)
    }
}
