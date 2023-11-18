use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use async_lock::Mutex;
use remoc::{codec::Bincode, prelude::*};
use serde::{Deserialize, Serialize};
use tokio::{
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    task::JoinHandle,
};

use super::{Main, MainFns};

#[derive(Debug)]
struct StaticState {
    socket_path: PathBuf,
}

#[derive(Debug)]
struct SharedState {}

impl SharedState {
    pub async fn spawn(&self, opts: SandboxOptions) -> Result<PathBuf, String> {
        println!("{:?}", opts);
        Ok(PathBuf::from("some result"))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ZygoteFns {
    pub spawn: rfn::RFn<(SandboxOptions,), std::result::Result<PathBuf, String>>,
}

#[derive(Debug)]
pub struct Zygote {
    state: Option<Arc<Mutex<StaticState>>>,
    handle: JoinHandle<()>,
}

impl Zygote {
    pub(crate) async fn new(
        socket_path: PathBuf,
        rx: OwnedReadHalf,
        tx: OwnedWriteHalf,
    ) -> Result<Zygote> {
        let (conn, mut tx, mut rx) =
            remoc::Connect::io::<_, _, _, _, Bincode>(remoc::Cfg::balanced(), rx, tx)
                .await
                .with_context(|| "while establishing IPC with the parent")?;
        let handle = tokio::spawn(async { conn.await.unwrap() });

        let state = StaticState { socket_path };

        let shared = Arc::new(Box::new(SharedState {}));
        let next = shared.clone();
        let mut spawn = |opts: SandboxOptions| async move {
            println!("{:?}", opts);
            Ok::<_, String>(PathBuf::from("some result"))
        };
        let spawn = rfn::RFn::new_1(spawn);

        let result = Zygote {
            state: Some(Arc::new(Mutex::new(state))),
            handle,
        };

        let fns = ZygoteFns { spawn };

        tracing::trace!("hello from zygote");
        let main: MainFns = rx.recv().await.unwrap().unwrap();
        tx.send(fns).await.unwrap();

        Ok(result)
    }

    pub async fn join(self) {
        self.handle.await.unwrap()
    }
}

bitflags::bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
    pub struct SharingFlags: u8 {
        const FILESYSTEM = 0b0000_0001;
        const RESOURCES = 0b0000_0010;
        const IPC = 0b0000_0100;
        const NETWORKING = 0b0000_1000;
        const PROCESSES = 0b0001_0000;
        const USERS = 0b0010_0000;
    }
}

bitflags::bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
    pub struct CopyFlags: u8 {
        const FILE_DESCRIPTORS = 0b0000_0001;
        const DEBUGGER = 0b0000_0010;
        const SIGNALS = 0b0000_0100;
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SandboxOptions {
    root: Option<PathBuf>,
    share: SharingFlags,
    copy: CopyFlags,
    hostname: Option<String>,
    uid: Option<u32>,
    gid: Option<u32>,
    suppl: Vec<u32>,
}
