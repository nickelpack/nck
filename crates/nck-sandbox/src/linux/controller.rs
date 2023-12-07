use std::{ffi::OsStr, ops::DerefMut, path::Path, sync::atomic::AtomicU32};

mod id_allocator;

use bytes::BytesMut;
use nck_util::{
    io::{TempDir, TempFile, Timeout},
    pool::Pooled,
    transport::AsyncPeer,
    BUFFER_POOL,
};
use nix::unistd::{Gid, Uid};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::{UnixListener, UnixStream},
    sync::oneshot,
};
use tracing::{Instrument, Level};

use self::id_allocator::{IdAllocator, PooledId};

use super::{
    proto::{PeerError, SerOsString},
    sandbox::SandboxRequest,
    syscall::{ChildProcess, NixSysCall, Result, Syscall},
    zygote::{Request, SpawnRequest, SpawnResponse},
    SOCKET_TIMEOUT,
};

#[tracing::instrument(name = "controller_main", level = "trace", skip_all, parent = None)]
pub async fn main<F, R>(
    cfg: super::Config,
    _child: ChildProcess,
    f: impl FnOnce(Controller) -> F,
) -> Result<R>
where
    F: std::future::Future<Output = R>,
{
    let socket_path = cfg.runtime_dir.join(super::zygote::SOCKET_NAME);
    let zygote = accept_socket::<NixSysCall>(SOCKET_TIMEOUT, socket_path).await?;

    tracing::info!("zygote connected");
    let controller = Controller::new(cfg, AsyncPeer::new(zygote.into_split()));

    let span = tracing::span!(Level::TRACE, "external_main");
    Ok(f(controller).instrument(span).await)
}

async fn accept_socket<SC: Syscall>(
    timeout: impl Timeout,
    socket_path: impl AsRef<Path>,
) -> std::io::Result<UnixStream> {
    let socket_path = socket_path.as_ref();
    if SC::exists(socket_path) {
        tracing::debug!(?socket_path, "deleting existing socket");
        if let Err(error) = SC::remove_file(socket_path) {
            tracing::warn!(
                ?error,
                ?socket_path,
                "failed to delete existing socket, attempting to listen anyway"
            )
        }
    }

    // Make sure the socket file gets cleaned up
    let _socket_file = TempFile::from(socket_path);
    let listener = UnixListener::bind(socket_path)?;

    tracing::info!(?socket_path, "listening");
    Ok(timeout.timeout_async(listener.accept()).await?.0)
}

#[derive(Debug, Clone)]
pub struct Controller {
    peer: AsyncPeer,
    users: IdAllocator,
    groups: IdAllocator,
}

impl Controller {
    pub fn new(cfg: super::Config, peer: AsyncPeer) -> Self {
        let users = IdAllocator::new(cfg.id_map.uid_min, cfg.id_map.uid_max);
        let groups = IdAllocator::new(cfg.id_map.uid_min, cfg.id_map.uid_max);
        Self {
            peer,
            users,
            groups,
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn new_sandbox(&mut self, name: &str) -> std::result::Result<Sandbox, PeerError> {
        let user_uid = self.users.allocate().await;
        let user_gid = self.groups.allocate().await;
        let root_uid = self.users.allocate().await;
        let root_gid = self.groups.allocate().await;

        tracing::trace!(
            name,
            ?user_uid,
            ?user_gid,
            ?root_uid,
            ?root_gid,
            "requesting new sandbox"
        );
        let response: SpawnResponse = self
            .peer
            .request_result::<SpawnResponse, PeerError, _>(&Request::Spawn(SpawnRequest::new(
                name,
                Uid::from_raw(*root_uid.get()),
                Gid::from_raw(*root_gid.get()),
                Uid::from_raw(*user_uid.get()),
                Gid::from_raw(*user_gid.get()),
            )))
            .await??;

        let socket = accept_socket::<NixSysCall>(SOCKET_TIMEOUT, response.socket_path()).await?;
        let peer = AsyncPeer::new(socket.into_split());
        Ok(Sandbox {
            peer,
            id: Default::default(),
            _drop_pid: response.pid().into(),
            _drop_working_dir: TempDir::from(response.sandbox_path()),
            _drop_ids: (user_uid, user_gid, root_uid, root_gid),
        })
    }
}

#[derive(Debug)]
pub struct Sandbox {
    peer: AsyncPeer,
    id: AtomicU32,
    _drop_pid: ChildProcess,
    _drop_working_dir: TempDir,
    _drop_ids: (PooledId, PooledId, PooledId, PooledId),
}

impl Sandbox {
    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn isolate_filesystem(&self) -> std::result::Result<(), PeerError> {
        self.peer
            .request_result::<(), PeerError, _>(&SandboxRequest::IsolateFilesystem)
            .await
            .map_err(|_| PeerError::IoError)
            .flatten()
            .inspect_err(|e| {
                tracing::error!(?e, "err");
            })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn create_dir(
        &self,
        path: impl AsRef<Path>,
        mode: u32,
    ) -> std::result::Result<(), PeerError> {
        self.peer
            .request_result::<(), PeerError, _>(&SandboxRequest::MkDir(path.as_ref().into(), mode))
            .await
            .map_err(|_| PeerError::IoError)
            .flatten()
            .inspect_err(|e| {
                tracing::error!(?e, "err");
            })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn symlink(
        &self,
        from: impl AsRef<Path>,
        to: impl AsRef<Path>,
    ) -> std::result::Result<(), PeerError> {
        self.peer
            .request_result::<(), PeerError, _>(&SandboxRequest::Link(
                from.as_ref().into(),
                to.as_ref().into(),
            ))
            .await
            .map_err(|_| PeerError::IoError)
            .flatten()
            .inspect_err(|e| {
                tracing::error!(?e, "err");
            })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn exec(
        &self,
        path: impl AsRef<Path>,
        args: impl AsRef<[&OsStr]>,
        env: impl AsRef<[(&OsStr, &OsStr)]>,
        dir: impl AsRef<Path>,
    ) -> std::result::Result<i32, PeerError> {
        self.peer
            .request_result::<i32, PeerError, _>(&SandboxRequest::Exec {
                path: path.as_ref().into(),
                args: args
                    .as_ref()
                    .iter()
                    .map(|f| Into::<SerOsString>::into(*f))
                    .collect(),
                env: env
                    .as_ref()
                    .iter()
                    .map(|(f, v)| (Into::<SerOsString>::into(*f), Into::<SerOsString>::into(*v)))
                    .collect(),
                dir: dir.as_ref().into(),
            })
            .await
            .inspect_err(|e| tracing::error!(?e, "err1"))
            .map_err(|_| PeerError::IoError)
            .flatten()
            .inspect_err(|e| {
                tracing::error!(?e, "err");
            })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn write(
        &self,
        path: impl AsRef<Path>,
        data: impl AsyncRead + Unpin + Send + 'static,
        mode: u32,
    ) -> std::result::Result<
        tokio::sync::oneshot::Receiver<std::result::Result<(), PeerError>>,
        PeerError,
    > {
        let id = self.id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let writer = self.peer.write_stream(id).await;
        self.peer
            .request_result::<(), PeerError, SandboxRequest>(&SandboxRequest::BeginFile(
                id,
                path.as_ref().into(),
                mode,
            ))
            .await??;

        async fn imp(
            id: u32,
            mut data: impl AsyncRead + Unpin,
            writer: flume::Sender<Pooled<'static, BytesMut>>,
            peer: AsyncPeer,
        ) -> std::result::Result<(), PeerError> {
            loop {
                let mut buffer = BUFFER_POOL.take();
                let len = data.read_buf(buffer.deref_mut()).await?;
                if len == 0 {
                    break;
                }
                buffer.resize(len, 0u8);
                if writer.send_async(buffer).await.is_err() {
                    break;
                }
            }
            drop(writer);
            peer.request_result::<(), PeerError, _>(&SandboxRequest::EndFile(id))
                .await??;
            Ok(())
        }

        let peer = self.peer.clone();
        let (send, recv) = oneshot::channel();
        tokio::spawn(async move {
            let result = imp(id, data, writer, peer).await;
            send.send(result).ok();
        });

        Ok(recv)
    }
}
