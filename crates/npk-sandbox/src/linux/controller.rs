use std::{marker::PhantomData, path::Path, sync::atomic::AtomicU64};

use nix::unistd::{Gid, Uid};
use npk_util::io::{timeout_async, TempDir, TempFile};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::UnixListener,
};

use super::{
    proto::{OverlapPeer, PeerAsync, PeerError},
    sandbox::SandboxRequest,
    syscall::{ChildProcess, Result, Syscall},
    zygote::{Request, SpawnRequest, SpawnResponse},
    SOCKET_TIMEOUT,
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
        zygote: PeerAsync::new(zygote.0),
        _phantom: PhantomData,
    };

    Ok(f(controller).await)
}

pub struct Controller<SC: Syscall> {
    cfg: super::Config,
    zygote: PeerAsync,
    _phantom: PhantomData<SC>,
}

impl<SC: Syscall> Controller<SC> {
    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn spawn_sandbox(&mut self) -> std::io::Result<Sandbox<SC>> {
        tracing::trace!("requesting new sandbox from zygote");

        self.zygote
            .write(&Request::Spawn(SpawnRequest::new(
                "npk-sandbox-01",
                Uid::from_raw(self.cfg.id_map.uid_min),
                Gid::from_raw(self.cfg.id_map.gid_min),
                Uid::from_raw(self.cfg.id_map.uid_min + 1),
                Gid::from_raw(self.cfg.id_map.gid_min + 1),
            )))
            .await?;

        tracing::trace!("request sent");

        let response: SpawnResponse = self.zygote.read().await?;

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

        let peer = OverlapPeer::new(socket);
        Ok(Sandbox {
            peer,
            id: Default::default(),
            _drop_pid: response.pid().into(),
            _drop_working_dir: TempDir::from(response.sandbox_path()),
        })
    }
}

#[derive(Debug)]
pub struct Sandbox<SC: Syscall> {
    peer: OverlapPeer,
    id: AtomicU64,
    _drop_pid: ChildProcess<SC>,
    _drop_working_dir: TempDir,
}

impl<SC: Syscall> Sandbox<SC> {
    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn isolate_filesystem(&self) -> std::result::Result<(), PeerError> {
        self.peer
            .request_result(&SandboxRequest::IsolateFilesystem)
            .await
            .map_err(|_| PeerError::IoError)
            .flatten()
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn write(
        &self,
        path: impl AsRef<Path>,
        data: &mut (impl AsyncRead + Unpin),
    ) -> std::result::Result<(), PeerError> {
        let path: Box<[u8]> = path.as_ref().as_os_str().as_encoded_bytes().into();
        let id = self.id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.peer
            .request_result::<(), _>(&SandboxRequest::BeginFile(id, path))
            .await??;
        tracing::trace!("starting");
        let result: std::result::Result<(), PeerError> = async move {
            // TODO: So many allocations
            loop {
                let mut buffer = vec![0u8; 4096];
                buffer.resize(4096, 0u8);
                let len = data.read(&mut buffer).await?;
                if len == 0 {
                    break;
                }
                buffer.resize(len, 0u8);
                let b = buffer.into_boxed_slice();
                self.peer
                    .request_result::<(), _>(&SandboxRequest::WriteFile(id, b))
                    .await??;
            }
            Ok(())
        }
        .await;

        let end_result = self
            .peer
            .request_result::<(), _>(&SandboxRequest::EndFile(id))
            .await;

        tracing::trace!("done");
        if result.is_err() {
            result
        } else {
            end_result?
        }
    }
}
