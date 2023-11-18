use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use flume::{Receiver, Sender};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixStream,
    },
    time::timeout,
};

use super::{super::SandboxBuilder, proto};

pub async fn start(options: SandboxBuilder, working_path: PathBuf, socket_path: PathBuf) -> isize {
    async fn bootstrap(
        socket_path: PathBuf,
    ) -> Result<(Sender<proto::ParentMessage>, Receiver<proto::ChildMessage>)> {
        let socket_path = socket_path.as_path();

        let socket = timeout(Duration::from_secs(2), UnixStream::connect(socket_path))
            .await
            .with_context(|| format!("while connecting to the parent via {:?}", socket_path))?
            .with_context(|| format!("while connecting to the parent via {:?}", socket_path))?;
        println!("got");

        let (read, write) = socket.into_split();
        let (send_to_child, receive_from_parent) = flume::bounded(1024);
        let (send_to_parent, receive_from_child) = flume::bounded(1024);

        tokio::spawn(async {
            if let Err(e) = send_worker(write, receive_from_child).await {
                tracing::error!("send worker task crashed: {:?}", e);
            }
        });
        tokio::spawn(async {
            if let Err(e) = receive_worker(read, send_to_child).await {
                tracing::error!("receive worker thread crashed: {:?}", e);
            }
        });

        Ok((send_to_parent, receive_from_parent))
    }

    println!("got");
    let (send, receive) = match bootstrap(socket_path).await {
        Ok(r) => r,
        Err(e) => {
            println!("got");
            tracing::error!("failed to bootstrap child: {:?}", e);
            return -1;
        }
    };

    loop {
        let j = tokio::join!(
            send.send_async(proto::ParentMessage::Hello),
            receive.recv_async()
        );

        j.0.unwrap();
        tracing::trace!("{:?}", j.1.unwrap());
    }

    0
}

async fn send_worker(
    mut send: OwnedWriteHalf,
    outgoing: Receiver<proto::ParentMessage>,
) -> Result<()> {
    loop {
        let message = if let Ok(message) = outgoing.recv_async().await {
            message
        } else {
            break Ok(());
        };

        let buffer = bitcode::encode(&message)
            .with_context(|| "while encoding a message for the parent process")?;
        let crc = proto::crc(&buffer).to_le_bytes();
        let len = (buffer.len() as u64).to_le_bytes();

        send.write_all(&len)
            .await
            .with_context(|| "while writing the length header to the parent process")?;
        send.write_all(&crc)
            .await
            .with_context(|| "while writing the CRC to the parent process")?;
        send.write_all(&buffer)
            .await
            .with_context(|| "while writing the data to the parent process")?;
    }
}

async fn receive_worker(
    mut read: OwnedReadHalf,
    incoming: Sender<proto::ChildMessage>,
) -> Result<()> {
    let mut big_buffer = Vec::new();
    loop {
        let mut buffer = [0u8; 8];
        read.read_exact(&mut buffer)
            .await
            .with_context(|| "while reading the length header from the parent process")?;
        let len = u64::from_le_bytes(buffer) as usize;
        read.read_exact(&mut buffer)
            .await
            .with_context(|| "while reading the CRC header from the parent process")?;
        let crc = u64::from_le_bytes(buffer);

        big_buffer.resize(len, 0u8);

        read.read_exact(&mut big_buffer)
            .await
            .with_context(|| "while reading the data from the parent process")?;

        if crc != proto::crc(&mut big_buffer) {
            return Err(anyhow::anyhow!("invalid data received from parent"))?;
        }

        let message = bitcode::decode(&big_buffer)
            .with_context(|| "while decoding the message from the parent")?;

        if incoming.send_async(message).await.is_err() {
            break Ok(());
        }
    }
}
