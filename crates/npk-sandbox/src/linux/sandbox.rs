use std::{
    collections::HashMap,
    ffi::OsStr,
    io::ErrorKind,
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::Arc,
};

use bitcode::{Decode, Encode};
use nix::{
    mount::MsFlags,
    sys::{personality::Persona, stat::Mode},
};
use npk_util::io::timeout_async;
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
    net::UnixStream,
    sync::{Mutex, RwLock},
    task::{JoinError, JoinHandle},
};

use super::{
    proto::{OverlapPeer, PeerError},
    syscall::{MountType, NixSysCall, Result, Syscall, SYS_NONE},
    SOCKET_TIMEOUT,
};

#[tracing::instrument(level = "trace", skip_all)]
pub async fn main<SC: Syscall + 'static>(
    name: &str,
    socket_path: PathBuf,
    rootfs_path: PathBuf,
) -> Result<()> {
    if let Err(error) = prctl::set_name(name) {
        let error = nix::Error::from_i32(error);
        tracing::warn!(?error, "failed to set sandbox process name");
    }

    tracing::trace!(?rootfs_path, "initializing rootfs");
    init_rootfs::<SC>(rootfs_path.as_path())?;

    tracing::trace!("disabling ASLR");
    SC::change_personality(|p| p | Persona::ADDR_NO_RANDOMIZE)?;

    tracing::trace!("setting hostname to localhost");
    SC::set_hostname("localhost")?;

    tracing::trace!("connecting to controller");

    let socket = timeout_async(SOCKET_TIMEOUT, UnixStream::connect(socket_path.as_path())).await?;

    let sandbox = SandboxProcess::<SC> {
        rootfs_path,
        remote: OverlapPeer::new(socket),
        files: Arc::new(RwLock::new(HashMap::new())),
        _phantom: PhantomData,
    };
    sandbox.run().await
}

#[tracing::instrument(level = "trace", skip_all)]
fn init_rootfs<SC: Syscall>(root: &Path) -> Result<()> {
    tracing::trace!(?root, "initializing rootfs directory");
    SC::chmod(root, Mode::from_bits_truncate(0o700))?;

    tracing::trace!("creating /tmp");
    let tmp = root.join("tmp");
    SC::create_dir_all(tmp.as_path())?;
    SC::mount(
        SYS_NONE,
        &tmp,
        Some(&MountType::TmpFs),
        MsFlags::empty(),
        SYS_NONE,
    )?;

    tracing::trace!("creating /etc");
    let etc = root.join("etc");
    SC::create_dir_all(etc.as_path())?;

    tracing::trace!("creating /etc/group");
    let etc_group = etc.join("group");
    SC::overwrite(etc_group, "root:x:0:\nbuilder:!:1000:\nnogroup:x:65534:\n")?;

    tracing::trace!("creating /etc/passwd");
    let etc_passwd = etc.join("passwd");
    SC::overwrite(etc_passwd, "root:x:0:0:root:/build:/noshell\nbuilder:x:1000:1000:builder:/build:/noshell\nnobody:x:65534:65534:Nobody:/:/noshell\n")?;

    tracing::trace!("creating /etc/hosts");
    let etc_hosts = etc.join("hosts");
    SC::overwrite(etc_hosts, "127.0.0.1 localhost\n::1 localhost\n")?;

    tracing::trace!("creating /etc/dev");
    let dev = root.join("dev");

    tracing::trace!("creating /etc/pts");
    let dev_pts = dev.join("pts");
    SC::create_dir_all(&dev_pts)?;

    if Path::new("/dev/pts/ptmx").exists() {
        tracing::trace!("creating /dev/pts");
        if let Err(error) = SC::mount(
            SYS_NONE,
            &dev_pts,
            Some(&MountType::DevPts),
            MsFlags::empty(),
            Some("newinstance,mode=0620"),
        ) {
            tracing::debug!(?error, "failed to mount devpts, falling back to bind");
            SC::bind("/dev/pts", &dev_pts)?;
            SC::bind("/dev/ptmx", dev.join("ptmx"))?;
        } else {
            let ptmx = dev.join("ptmx");
            SC::symlink("/dev/pts/ptmx", ptmx)?;
            SC::chmod(dev.join("pts/ptmx"), Mode::from_bits_truncate(0o666))?;
        }
    }

    tracing::trace!("creating /dev/shm");
    let dev_shm = dev.join("shm");
    SC::create_dir_all(&dev_shm)?;
    SC::mount(
        SYS_NONE,
        &dev_shm,
        Some(&MountType::TmpFs),
        MsFlags::empty(),
        SYS_NONE,
    )?;

    tracing::trace!("creating /dev/sys");
    let sys = root.join("sys");
    SC::create_dir_all(&sys)?;
    // Likely to fail in rootlees
    if let Err(error) = SC::mount(
        SYS_NONE,
        &sys,
        Some(&MountType::SysFs),
        MsFlags::empty(),
        SYS_NONE,
    ) {
        tracing::debug!(?error, "failed to mount /sys, falling back to a bind");
        SC::bind("/sys", &sys)?;
    }

    tracing::trace!("creating /proc");
    let proc = root.join("proc");
    SC::create_dir_all(&proc)?;
    SC::mount(
        SYS_NONE,
        &proc,
        Some(&MountType::Proc),
        MsFlags::empty(),
        SYS_NONE,
    )?;

    tracing::trace!("creating /dev/null");
    SC::bind("/dev/null", dev.join("null"))?;
    tracing::trace!("creating /dev/zero");
    SC::bind("/dev/zero", dev.join("zero"))?;
    tracing::trace!("creating /dev/full");
    SC::bind("/dev/full", dev.join("full"))?;
    tracing::trace!("creating /dev/random");
    SC::bind("/dev/random", dev.join("random"))?;
    tracing::trace!("creating /dev/urandom");
    SC::bind("/dev/urandom", dev.join("urandom"))?;

    tracing::trace!("symlinks fds");
    SC::symlink("/proc/self/fd", dev.join("fd"))?;
    SC::symlink("/proc/self/fd/0", dev.join("stdin"))?;
    SC::symlink("/proc/self/fd/1", dev.join("stdout"))?;
    SC::symlink("/proc/self/fd/2", dev.join("stderr"))?;

    Ok(())
}

#[derive(Debug, Encode, Decode)]
pub enum SandboxRequest {
    IsolateFilesystem,
    BeginFile(u64, Box<[u8]>),
    WriteFile(u64, Box<[u8]>),
    EndFile(u64),
}

#[derive(Debug)]
struct PendingFile {
    send: flume::Sender<Box<[u8]>>,
    handle: JoinHandle<std::io::Result<()>>,
}

#[derive(Debug)]
struct SandboxProcess<SC: Syscall = NixSysCall> {
    rootfs_path: PathBuf,
    remote: OverlapPeer,
    files: Arc<RwLock<HashMap<u64, PendingFile>>>,
    _phantom: PhantomData<SC>,
}

impl<SC: Syscall> SandboxProcess<SC> {
    async fn run(self) -> Result<()> {
        loop {
            let (id, request) = self.remote.next().await?;
            match request {
                SandboxRequest::IsolateFilesystem => {
                    let result = self.isolate_filesystem();
                    self.remote.respond_result(id, result).await?;
                }
                SandboxRequest::BeginFile(file_id, path) => {
                    let path = Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&path) });
                    let result = self.begin_file(file_id, path).await;
                    self.remote.respond_result(id, result).await?;
                }
                SandboxRequest::WriteFile(file_id, data) => {
                    let result = self.write_file(file_id, data).await;
                    self.remote.respond_result(id, result).await?;
                }
                SandboxRequest::EndFile(file_id) => {
                    let result = self.end_file(file_id).await;
                    self.remote.respond_result(id, result).await?;
                }
            }
        }
    }

    fn isolate_filesystem(&self) -> std::result::Result<(), PeerError> {
        SC::pivot(self.rootfs_path.as_path())?;
        Ok(())
    }

    async fn begin_file(&self, id: u64, path: &Path) -> std::result::Result<(), PeerError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(false)
            .truncate(true)
            .write(true)
            .open(path)
            .await?;

        let (send, receive) = flume::bounded::<Box<[u8]>>(16);
        let handle = tokio::spawn(async move {
            while let Ok(data) = receive.recv_async().await {
                file.write_all(&data).await?;
                tracing::trace!(id, "wrote {} bytes", data.len());
            }
            Ok(())
        });

        let mut files = self.files.write().await;
        files.insert(id, PendingFile { send, handle });
        tracing::trace!(id, ?path, "opened");

        Ok(())
    }

    async fn write_file(&self, id: u64, data: Box<[u8]>) -> std::result::Result<(), PeerError> {
        let files = self.files.read().await;
        let file = files
            .get(&id)
            .ok_or_else(|| PeerError::Other(format!("file {id:x} has not been started")))?;
        tracing::trace!(id, "pending write of {} bytes", data.len());
        if file
            .send
            .send_async(data)
            .await
            .map_err(|_| std::io::Error::from(ErrorKind::ConnectionAborted))
            .is_err()
        {
            drop(files);
            return self.end_file(id).await;
        }
        Ok(())
    }

    async fn end_file(&self, id: u64) -> std::result::Result<(), PeerError> {
        let mut files = self.files.write().await;
        let file = files
            .remove(&id)
            .ok_or_else(|| PeerError::Other(format!("file {id:x} has not been started")))?;
        drop(file.send);
        file.handle
            .await
            .map_err(|e| std::io::Error::new(ErrorKind::ConnectionAborted, e))
            .flatten()?;
        tracing::trace!(id, "closed");
        Ok(())
    }
}
