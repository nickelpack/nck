use std::marker::PhantomData;

use nix::unistd::{Gid, Uid};
use npk_util::io::{timeout_async, Buffer, TempDir, TempFile};
use remoc::rtc;
use tokio::net::{UnixListener, UnixStream};

use crate::{
    current::flavor::zygote::{
        read_from_socket_async, write_to_socket_async, Request, SpawnRequest, SpawnResponse,
    },
    unix::{SOCKET_TIMEOUT, ZYGOTE_HEADER_SIZE},
};

use super::{
    proto::{SandboxProcess, ServerWorker},
    syscall::ChildProcess,
    syscall::{Result, Syscall},
};

#[tracing::instrument(name = "controller_main", level = "trace", skip_all)]
pub async fn main<SC: Syscall, F, R>(
    cfg: super::Config,
    _child: ChildProcess<SC>,
    f: impl FnOnce(Controller<SC>) -> F,
) -> Result<R>
where
    F: std::future::Future<Output = R>,
{
    let zygote = {
        let socket_path = cfg.runtime_dir.join(super::zygote::SOCKET_NAME);
        if socket_path.exists() {
            tracing::debug!(?socket_path, "deleting existing socket");
            if let Err(error) = SC::remove_file(socket_path.as_path()) {
                tracing::warn!(
                    ?error,
                    ?socket_path,
                    "failed to delete existing socket, attempting to listen anyway"
                )
            }
        }

        let listener = UnixListener::bind(socket_path.as_path())?;

        // Make sure the socket file gets cleaned up
        let _socket_file = TempFile::from(socket_path.as_path());

        tracing::info!(?socket_path, "listening for zygote");
        timeout_async(SOCKET_TIMEOUT, listener.accept()).await?
    };

    tracing::info!("zygote connected");
    let controller = Controller {
        cfg,
        zygote: zygote.0,
        write_buffer: Buffer::with_capacity(ZYGOTE_HEADER_SIZE),
        read_buffer: Buffer::with_capacity(ZYGOTE_HEADER_SIZE),
        bitcode_buffer: bitcode::Buffer::with_capacity(1024),
        _phantom: PhantomData,
    };

    Ok(f(controller).await)
}

pub struct Controller<SC: Syscall> {
    cfg: super::Config,
    zygote: UnixStream,
    write_buffer: Buffer,
    read_buffer: Buffer,
    bitcode_buffer: bitcode::Buffer,
    _phantom: PhantomData<SC>,
}

impl<SC: Syscall> Controller<SC> {
    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn spawn_sandbox(&mut self) -> std::io::Result<Sandbox<SC>> {
        tracing::trace!("requesting new sandbox from zygote");

        write_to_socket_async(
            &mut self.write_buffer,
            &mut self.bitcode_buffer,
            &mut self.zygote,
            &Request::Spawn(SpawnRequest::new(
                "npk-sandbox-01",
                Uid::from_raw(self.cfg.id_map.uid_min),
                Gid::from_raw(self.cfg.id_map.gid_min),
                Uid::from_raw(self.cfg.id_map.uid_min + 1),
                Gid::from_raw(self.cfg.id_map.gid_min + 1),
            )),
        )
        .await?;

        tracing::trace!("request sent");

        let response: SpawnResponse = read_from_socket_async(
            &mut self.read_buffer,
            &mut self.bitcode_buffer,
            &mut self.zygote,
        )
        .await?;

        tracing::trace!("response received");

        let socket = {
            let listener = UnixListener::bind(response.socket_path())?;

            tracing::debug!(
                socket_path = ?response.socket_path(),
                "waiting for sandbox to connect",
            );

            let _socket_path = TempFile::from(response.socket_path());

            timeout_async(SOCKET_TIMEOUT, listener.accept()).await?.0
        };

        let (server, client) = super::proto::connect::<
            super::proto::ControllerProcessServerSharedMut<ControllerProcess, _>,
            ControllerProcess,
            super::proto::SandboxProcessClient,
        >(socket, ControllerProcess { remote: None }, 1)
        .await?;

        {
            let mut sandbox = server.write().await;
            sandbox.remote = Some(client);
        }

        Ok(Sandbox {
            _drop_pid: response.pid().into(),
            _drop_working_dir: TempDir::from(response.sandbox_path()),
            server,
        })
    }
}

#[derive(Debug)]
pub struct Sandbox<SC: Syscall> {
    server: ServerWorker<ControllerProcess>,

    _drop_pid: ChildProcess<SC>,
    _drop_working_dir: TempDir,
}

#[derive(Debug)]
struct ControllerProcess {
    remote: Option<super::proto::SandboxProcessClient>,
}

#[rtc::async_trait]
impl super::proto::ControllerProcess for ControllerProcess {}

impl<SC: Syscall> Sandbox<SC> {
    pub async fn isolate_filesystem(&mut self) {
        let mut server = self.server.write().await;
        server
            .remote
            .as_mut()
            .unwrap()
            .isolate_filesystem()
            .await
            .unwrap()
    }
}
