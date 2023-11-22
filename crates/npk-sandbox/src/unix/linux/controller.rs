use std::path::PathBuf;

use anyhow::{Context, Result};
use futures::io::BufReader;
use nix::unistd::{Gid, Pid, Uid};
use npk_util::io::{timeout_async, Buffer, TempDir, TempFile};
use tokio::net::{UnixListener, UnixStream};

use crate::{
    current::flavor::zygote::{
        read_from_socket_async, write_to_socket_async, Request, SpawnRequest, SpawnResponse,
    },
    unix::{SOCKET_TIMEOUT, ZYGOTE_HEADER_SIZE},
};

use super::proc::ChildProcess;

pub(crate) fn main<F>(
    cfg: super::Config,
    child: ChildProcess,
    f: impl FnOnce(Controller) -> F,
) -> Result<()>
where
    F: std::future::Future<Output = Result<()>>,
{
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(main_async(cfg, &child, f))
}

async fn main_async<F>(
    cfg: super::Config,
    c: &ChildProcess,
    f: impl FnOnce(Controller) -> F,
) -> Result<()>
where
    F: std::future::Future<Output = Result<()>>,
{
    let zygote = {
        let socket_path = cfg.working_dir.join(super::zygote::SOCKET_NAME);
        let listener = UnixListener::bind(socket_path.as_path()).with_context(|| {
            format!(
                "when binding to the controller socket path {:?}",
                socket_path.as_path()
            )
        })?;

        // Make sure the socket file gets cleaned up
        let _socket_file = TempFile::from(socket_path.as_path());

        tracing::info!("listening for zygote at {:?}", socket_path);
        timeout_async(SOCKET_TIMEOUT, listener.accept())
            .await
            .with_context(|| "while accepting the zygote connection")?
    };

    let write_buffer = Buffer::with_capacity(ZYGOTE_HEADER_SIZE);

    tracing::info!("zygote connected");
    let controller = Controller {
        zygote: zygote.0,
        write_buffer,
        read_buffer: Buffer::with_capacity(ZYGOTE_HEADER_SIZE),
    };

    f(controller).await
}

pub struct Controller {
    zygote: UnixStream,
    write_buffer: Buffer,
    read_buffer: Buffer,
}

impl Controller {
    pub async fn spawn_sandbox(&mut self) -> Result<Sandbox> {
        tracing::info!("requesting new sandbox from zygote");

        write_to_socket_async(
            &mut self.write_buffer,
            &mut self.zygote,
            &Request::Spawn(SpawnRequest {
                root_uid: 0,
                root_gid: 0,
                user_uid: 0,
                user_gid: 0,
            }),
        )
        .await
        .with_context(|| "while sending request to zygote")?;

        tracing::trace!("request sent");

        let response: SpawnResponse =
            read_from_socket_async(&mut self.read_buffer, &mut self.zygote)
                .await
                .with_context(|| "while reading response from zygote")?;

        tracing::trace!("response received");

        let socket = {
            let listener =
                UnixListener::bind(response.socket_path.as_path()).with_context(|| {
                    format!(
                        "while binding {:?} for sandbox process",
                        response.socket_path.as_path()
                    )
                })?;

            tracing::debug!(
                "waiting for sandbox to connect to {:?}",
                response.socket_path
            );

            let _socket_path = TempFile::from(response.socket_path);

            timeout_async(SOCKET_TIMEOUT, listener.accept())
                .await
                .with_context(|| "while accepting sandbox process socket")?
                .0
        };

        Ok(Sandbox {
            pid: Pid::from_raw(response.pid).into(),
            socket,
            working_dir: response.sandbox_path.into(),
            rootfs_path: response.rootfs_path,
        })
    }
}

#[derive(Debug)]
pub struct Sandbox {
    pid: ChildProcess,
    socket: UnixStream,
    working_dir: TempDir,
    rootfs_path: PathBuf,
}
