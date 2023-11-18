use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use async_lock::Mutex;
use remoc::codec::Bincode;
use serde::{Deserialize, Serialize};
use tokio::{
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    task::JoinHandle,
};

use super::{SandboxOptions, ZygoteFns};

#[derive(Debug)]
struct StaticState {
    socket_path: PathBuf,
    child: super::PidContainer,
}

#[derive(Debug)]
struct SharedState {}

#[derive(Debug, Serialize, Deserialize)]
pub struct MainFns {}

#[derive(Debug)]
pub struct Main {
    state: Arc<Mutex<StaticState>>,
    shared: SharedState,
    handle: JoinHandle<()>,
}

impl Main {
    pub(crate) async fn new(
        child: super::PidContainer,
        socket_path: PathBuf,
        rx: OwnedReadHalf,
        tx: OwnedWriteHalf,
    ) -> Result<Main> {
        let (conn, mut tx, mut rx) =
            remoc::Connect::io::<_, _, _, _, Bincode>(remoc::Cfg::balanced(), rx, tx)
                .await
                .with_context(|| "while establishing IPC with the zygote")?;
        let handle = tokio::spawn(async { conn.await.unwrap() });

        let state = StaticState { socket_path, child };

        let shared = SharedState {};

        let fns = MainFns {};

        tracing::trace!("hello from main");
        tx.send(fns).await;
        let zyg: ZygoteFns = loop {
            match rx.recv().await {
                Ok(Some(v)) => break v,
                Err(e) => Err(e).unwrap(),
                _ => {}
            }
        };
        tracing::info!("got fns");
        let r = zyg
            .spawn
            .try_call(SandboxOptions::default())
            .await
            .unwrap()
            .unwrap();
        tracing::info!("got result {:?}", r);

        Ok(Main {
            state: Arc::new(Mutex::new(state)),
            shared,
            handle,
        })
    }

    pub async fn join(self) {
        self.handle.await.unwrap()
    }
}
