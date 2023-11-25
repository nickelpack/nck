use std::{ops::Deref, sync::Arc};

use remoc::{codec, prelude::*};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{net::UnixStream, sync::RwLock, task::JoinHandle};

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum SandboxError {
    #[error("an RPC error occurred {:?}", _0)]
    RpcError(#[from] rtc::CallError),
    #[error("an I/O error occurred")]
    IoError,
}

impl From<std::io::Error> for SandboxError {
    fn from(_: std::io::Error) -> Self {
        Self::IoError
    }
}

impl From<nix::Error> for SandboxError {
    fn from(_: nix::Error) -> Self {
        Self::IoError
    }
}

#[rtc::remote]
pub trait SandboxProcess {
    async fn isolate_network(&mut self) -> Result<(), SandboxError>;
    async fn isolate_filesystem(&mut self) -> Result<(), SandboxError>;
}

#[rtc::remote]
pub trait ControllerProcess {}

#[tracing::instrument(skip_all, err(Debug))]
pub async fn connect<S, L, R>(
    socket: UnixStream,
    local_server: L,
    request_buffer: usize,
) -> nix::Result<(ServerWorker<L>, R)>
where
    S: ServerSharedMut<L, codec::Default> + Send + 'static,
    S::Client: RemoteSend + std::fmt::Debug,
    L: Send + Sync + 'static,
    R: RemoteSend,
{
    let (rx, tx) = socket.into_split();
    let (conn, mut tx, mut rx) = remoc::Connect::io_buffered::<_, _, _, _, codec::Default>(
        remoc::Cfg::balanced(),
        rx,
        tx,
        4096,
    )
    .await
    .map_err(|error| {
        tracing::error!(?error, "failed to establish RPC");
        nix::Error::UnknownErrno
    })?;

    let value = Arc::new(RwLock::new(local_server));
    let (server, local_client) = S::new(value.clone(), request_buffer);

    let conn = async move {
        match conn.await {
            Ok(_)
            | Err(remoc::chmux::ChMuxError::StreamClosed | remoc::chmux::ChMuxError::Reset) => {
                tracing::info!("remote closed the connection");
            }
            Err(error) => {
                tracing::error!(?error, "connection failed");
            }
        }
    };

    let send_client = async move {
        if let Err(error) = tx.send(local_client).await {
            tracing::error!(?error, "failed to send local client");
        } else {
            tracing::trace!("sent local client");
        }
    };

    let handle = tokio::spawn(async move {
        tokio::join! { conn, server.serve(true), send_client }
    });

    if let Some(client) = rx.recv().await.unwrap() {
        tracing::info!("connected to remote");
        Ok((ServerWorker { value, handle }, client))
    } else {
        handle.abort();
        tracing::info!("remote already disconnected");
        Err(nix::Error::EPIPE)
    }
}

#[derive(Debug)]
pub struct ServerWorker<L> {
    handle: JoinHandle<((), (), ())>,
    value: Arc<RwLock<L>>,
}

impl<L> ServerWorker<L> {
    pub async fn wait(self) -> Result<(), tokio::task::JoinError> {
        drop(self.value);
        self.handle.await.map(|_| ())
    }
}

impl<L> Deref for ServerWorker<L> {
    type Target = RwLock<L>;

    fn deref(&self) -> &Self::Target {
        self.value.deref()
    }
}
