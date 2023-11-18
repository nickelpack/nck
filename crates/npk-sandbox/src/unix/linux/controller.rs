use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};

use tokio::net::{UnixListener, UnixStream};

use super::{Main, Zygote};
use crate::{io_timeout, wait_for_file};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

enum PendingType {
    Main(super::PidContainer),
    Zygote,
}

pub struct ControllerSpawner {
    result: PendingType,
    socket_path: PathBuf,
}

pub enum ControllerType {
    Main(Main),
    Zygote(Zygote),
}

impl ControllerSpawner {
    pub fn new(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        let result =
            match super::proc::fork().with_context(|| "while starting the sandbox controller")? {
                nix::unistd::ForkResult::Parent { child } => {
                    PendingType::Main(super::PidContainer(Some(child)))
                }
                nix::unistd::ForkResult::Child => PendingType::Zygote,
            };
        Ok(Self {
            result,
            socket_path,
        })
    }

    pub async fn start(self) -> Result<ControllerType> {
        match self.result {
            PendingType::Main(pid) => {
                let listener = UnixListener::bind(&self.socket_path)
                    .with_context(|| "while binding to the socket in the main process")?;
                tracing::info!("listening for zygote at {:?}", &self.socket_path);

                let zygote = io_timeout(SOCKET_TIMEOUT, listener.accept())
                    .await
                    .with_context(|| "while waiting for the zygote to connect on the socket")?
                    .0;
                tracing::info!("zygote connected");

                let (rx, tx) = zygote.into_split();

                Ok(ControllerType::Main(
                    Main::new(pid, self.socket_path.clone(), rx, tx)
                        .await
                        .with_context(|| "while executing the main process worker thread")?,
                ))
            }

            PendingType::Zygote => {
                io_timeout(SOCKET_TIMEOUT, wait_for_file(self.socket_path.as_path()))
                    .await
                    .with_context(|| {
                        "while waiting for the main process socket to appear on the filesystem"
                    })?;

                tracing::info!("connecting to main process at {:?}", &self.socket_path);
                let main = io_timeout(
                    SOCKET_TIMEOUT,
                    UnixStream::connect(self.socket_path.as_path()),
                )
                .await
                .with_context(|| "while connecting to the main process socket")?;

                let (rx, tx) = main.into_split();
                Ok(ControllerType::Zygote(
                    Zygote::new(self.socket_path.clone(), rx, tx)
                        .await
                        .with_context(|| "while executing the zygote worker thread")?,
                ))
            }
        }
    }
}
