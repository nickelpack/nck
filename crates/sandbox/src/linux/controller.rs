use std::{
    ffi::{OsStr, OsString},
    io::{Error, ErrorKind},
    path::{Path, PathBuf},
};

mod id_allocator;

use nck_core::io::{TempDir, TempFile, Timeout};
use tokio::{
    io::{AsyncBufRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufStream},
    net::{UnixListener, UnixStream},
};
use tracing::{Instrument, Level};

use self::id_allocator::{IdAllocator, PooledId};

use super::{
    syscall::{ChildProcess, NixSysCall, Syscall},
    SOCKET_TIMEOUT,
};

#[tracing::instrument(name = "controller_main", level = "trace", skip_all, parent = None)]
pub async fn main<F, R>(
    cfg: crate::Settings,
    _child: ChildProcess,
    f: impl FnOnce(Controller) -> F,
) -> std::io::Result<R>
where
    F: std::future::Future<Output = R>,
{
    let socket_path = cfg.tmp_directory.join(super::zygote::SOCKET_NAME);
    let zygote = accept_socket::<NixSysCall>(SOCKET_TIMEOUT, socket_path).await?;

    tracing::info!("zygote connected");
    let controller = Controller::new(&cfg, zygote);

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

#[derive(Debug)]
pub struct Controller {
    socket: BufStream<UnixStream>,
    users: IdAllocator,
    groups: IdAllocator,
}

impl Controller {
    pub fn new(cfg: &crate::Settings, socket: UnixStream) -> Self {
        let users = IdAllocator::new(cfg.linux.uid_min, cfg.linux.uid_max);
        let groups = IdAllocator::new(cfg.linux.gid_min, cfg.linux.gid_max);
        Self {
            socket: BufStream::new(socket),
            users,
            groups,
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn begin_build(&mut self, formula_path: &Path) -> std::io::Result<Sandbox> {
        let uid = self.users.allocate().await;
        let gid = self.groups.allocate().await;

        tracing::trace!(?formula_path, ?uid, ?gid, "requesting build");

        self.socket.write_u16(0).await?;
        self.socket.write_u32(*uid).await?;
        self.socket.write_u32(*gid).await?;
        write_os_str(&mut self.socket, formula_path.as_os_str()).await?;
        self.socket.flush().await?;

        let status = self.socket.read_u16().await?;
        match status {
            0 => {
                let work_dir: PathBuf = read_os_str(&mut self.socket).await?.into();
                let work_dir = TempDir::from(work_dir);
                let socket_path: PathBuf = read_os_str(&mut self.socket).await?.into();

                let socket = accept_socket::<NixSysCall>(SOCKET_TIMEOUT, socket_path).await?;
                Ok(Sandbox {
                    _socket: socket,
                    _drop_working_dir: work_dir,
                    _drop_ids: (uid, gid),
                })
            }
            _ => Err(Error::from(ErrorKind::Other)),
        }
    }
}

async fn write_os_str(
    writer: &mut (impl AsyncWrite + Unpin),
    os_str: &OsStr,
) -> std::io::Result<()> {
    let bytes = os_str.as_encoded_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(Error::from(ErrorKind::InvalidInput));
    }
    writer.write_u16(bytes.len() as u16).await?;
    writer.write_all(bytes).await?;
    Ok(())
}

async fn read_os_str(reader: &mut (impl AsyncBufRead + Unpin)) -> std::io::Result<OsString> {
    let len = reader.read_u16().await? as usize;
    let mut buffer = vec![0u8; len];
    reader.read_exact(&mut buffer[..len]).await?;
    Ok(unsafe { OsString::from_encoded_bytes_unchecked(buffer) })
}

#[derive(Debug)]
pub struct Sandbox {
    _socket: UnixStream,
    _drop_working_dir: TempDir,
    _drop_ids: (PooledId, PooledId),
}
