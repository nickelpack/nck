use nix::unistd::Pid;
use npk_util::io::{timeout_async, Buffer, TempDir, TempFile};
use tokio::net::{UnixListener, UnixStream};

use crate::{
    current::flavor::zygote::{
        read_from_socket_async, write_to_socket_async, Request, SpawnRequest, SpawnResponse,
    },
    unix::{SOCKET_TIMEOUT, ZYGOTE_HEADER_SIZE},
};

use super::proc::ChildProcess;

pub(crate) fn main<F, R>(
    cfg: super::Config,
    child: ChildProcess,
    f: impl FnOnce(Controller) -> F,
) -> nix::Result<R>
where
    F: std::future::Future<Output = R>,
{
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(main_async(cfg, &child, f))
}

#[tracing::instrument(name = "controller_main", level = "trace", skip_all, err(Debug))]
async fn main_async<F, R>(
    cfg: super::Config,
    _: &ChildProcess,
    f: impl FnOnce(Controller) -> F,
) -> nix::Result<R>
where
    F: std::future::Future<Output = R>,
{
    let zygote = {
        let socket_path = cfg.runtime_dir.join(super::zygote::SOCKET_NAME);
        if socket_path.exists() {
            tracing::debug!(?socket_path, "deleting existing socket");
            if let Err(error) = std::fs::remove_file(socket_path.as_path()) {
                tracing::warn!(
                    ?error,
                    ?socket_path,
                    "failed to delete existing socket, attempting to listen anyway"
                )
            }
        }

        let listener =
            UnixListener::bind(socket_path.as_path()).map_err(super::std_error_to_nix)?;

        // Make sure the socket file gets cleaned up
        let _socket_file = TempFile::from(socket_path.as_path());

        tracing::info!(?socket_path, "listening for zygote");
        timeout_async(SOCKET_TIMEOUT, listener.accept())
            .await
            .map_err(super::std_error_to_nix)?
    };

    tracing::info!("zygote connected");
    let controller = Controller {
        cfg,
        zygote: zygote.0,
        write_buffer: Buffer::with_capacity(ZYGOTE_HEADER_SIZE),
        read_buffer: Buffer::with_capacity(ZYGOTE_HEADER_SIZE),
    };

    Ok(f(controller).await)
}

pub struct Controller {
    cfg: super::Config,
    zygote: UnixStream,
    write_buffer: Buffer,
    read_buffer: Buffer,
}

impl Controller {
    #[tracing::instrument(level = "trace", skip_all, err(Debug))]
    pub async fn spawn_sandbox(&mut self) -> std::io::Result<Sandbox> {
        tracing::debug!("requesting new sandbox from zygote");

        write_to_socket_async(
            &mut self.write_buffer,
            &mut self.zygote,
            &Request::Spawn(SpawnRequest {
                name: "npk-sandbox-01".to_string(),
                root_uid: self.cfg.id_map.uid_min,
                root_gid: self.cfg.id_map.gid_min,
                user_uid: self.cfg.id_map.uid_min + 1,
                user_gid: self.cfg.id_map.gid_min + 1,
            }),
        )
        .await?;

        tracing::trace!("request sent");

        let response: SpawnResponse =
            read_from_socket_async(&mut self.read_buffer, &mut self.zygote).await?;

        tracing::trace!("response received");

        let socket = {
            let listener = UnixListener::bind(response.socket_path.as_path())?;

            tracing::debug!(
                ?response.socket_path,
                "waiting for sandbox to connect",
            );

            let _socket_path = TempFile::from(response.socket_path);

            timeout_async(SOCKET_TIMEOUT, listener.accept()).await?.0
        };

        tracing::info!("sandbox connected");
        Ok(Sandbox {
            pid: Pid::from_raw(response.pid).into(),
            socket,
            working_dir: TempDir::from(response.sandbox_path.as_path()),
        })
    }
}

#[derive(Debug)]
pub struct Sandbox {
    pid: ChildProcess,
    socket: UnixStream,
    working_dir: TempDir,
}
