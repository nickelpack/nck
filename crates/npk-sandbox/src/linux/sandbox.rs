use std::{
    collections::HashMap,
    fs::Permissions,
    io::ErrorKind,
    marker::PhantomData,
    os::unix::prelude::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use bytes::BytesMut;
use flume::Receiver;
use nix::{
    mount::MsFlags,
    sys::{personality::Persona, stat::Mode},
};
use npk_util::{io::Timeout, pool::PooledItem, transport::AsyncPeer};
use speedy::{Readable, Writable};
use tokio::{fs::OpenOptions, io::AsyncWriteExt, net::UnixStream, sync::Mutex, task::JoinHandle};
use tracing::Instrument;

use super::{
    proto::{PeerError, SerOsString},
    syscall::{MountType, NixSysCall, Result, Syscall, SyscallError, SYS_NONE},
    SOCKET_TIMEOUT,
};

#[tracing::instrument(level = "trace", skip_all, err(Debug), parent = None)]
pub async fn sandbox_main<SC: Syscall + 'static>(
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

    let socket = SOCKET_TIMEOUT
        .timeout_async(UnixStream::connect(socket_path.as_path()))
        .await?;

    let sandbox = SandboxProcess::<SC> {
        rootfs_path,
        remote: AsyncPeer::new(socket.into_split()),
        files: Arc::new(Mutex::new(HashMap::new())),
        _phantom: PhantomData,
    };
    match sandbox.run().in_current_span().await {
        Ok(_) => Ok(()),
        Err(SyscallError::IoError(e)) if e.kind() == ErrorKind::ConnectionAborted => Ok(()),
        other => other,
    }
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

#[derive(Debug, Readable, Writable)]
pub enum SandboxRequest {
    IsolateFilesystem,
    BeginFile(u32, SerOsString, u32),
    EndFile(u32),
    MkDir(SerOsString, u32),
    Link(SerOsString, SerOsString),
    Exec {
        path: SerOsString,
        args: Vec<SerOsString>,
        env: Vec<(SerOsString, SerOsString)>,
        dir: SerOsString,
    },
}

#[derive(Debug)]
struct SandboxProcess<SC: Syscall = NixSysCall> {
    rootfs_path: PathBuf,
    remote: AsyncPeer,
    files: Arc<Mutex<HashMap<u32, JoinHandle<std::io::Result<()>>>>>,
    _phantom: PhantomData<SC>,
}

impl<SC: Syscall + 'static> SandboxProcess<SC> {
    async fn run(self) -> Result<()> {
        loop {
            let (id, request) = self.remote.next::<SandboxRequest>().await?.into_inner();
            match request {
                SandboxRequest::IsolateFilesystem => {
                    let result = self.isolate_filesystem();
                    self.remote.respond_result(id, result).await.ok();
                }
                SandboxRequest::BeginFile(file_id, path, mode) => {
                    let stream = self.remote.read_stream(file_id).await;
                    let result = self.begin_file(file_id, stream, path.as_ref(), mode).await;
                    self.remote.respond_result(id, result).await.ok();
                }
                SandboxRequest::EndFile(file_id) => {
                    let result = self.end_file(file_id).await;
                    self.remote.respond_result(id, result).await.ok();
                }
                SandboxRequest::MkDir(path, mode) => {
                    let result = Self::mk_dir(path.as_ref(), mode).await;
                    self.remote.respond_result(id, result).await.ok();
                }
                SandboxRequest::Link(from, to) => {
                    let result = Self::link(from.as_ref(), to.as_ref()).await;
                    self.remote.respond_result(id, result).await.ok();
                }
                SandboxRequest::Exec {
                    path,
                    args,
                    env,
                    dir,
                } => {
                    let result = Self::exec(path.as_ref(), &args, env, dir.as_ref()).await;
                    self.remote.respond_result(id, result).await.ok();
                }
            }
        }
    }

    fn isolate_filesystem(&self) -> std::result::Result<(), PeerError> {
        SC::pivot(self.rootfs_path.as_path())?;
        Ok(())
    }

    async fn exec(
        path: &Path,
        args: &[SerOsString],
        env: Vec<(SerOsString, SerOsString)>,
        dir: &Path,
    ) -> std::result::Result<i32, PeerError> {
        let mut proc = tokio::process::Command::new(path)
            .args(args)
            .env_clear()
            .envs(env.into_iter())
            .current_dir(dir)
            .spawn()
            .inspect_err(|e| tracing::error!(?e, "err"))?;
        let exit = proc
            .wait()
            .await
            .inspect_err(|error| tracing::trace!(?error, "error"))?;
        Ok(exit.code().unwrap_or_default())
    }

    async fn mk_dir(path: &Path, mode: u32) -> std::result::Result<(), PeerError> {
        tokio::fs::create_dir_all(path).await?;
        tokio::fs::set_permissions(path, Permissions::from_mode(mode)).await?;
        Ok(())
    }

    async fn link(from: &Path, to: &Path) -> std::result::Result<(), PeerError> {
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::symlink(from, to).await?;
        Ok(())
    }

    async fn begin_file(
        &self,
        id: u32,
        receiver: Receiver<PooledItem<'static, BytesMut>>,
        path: &Path,
        mode: u32,
    ) -> std::result::Result<(), PeerError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(false)
            .truncate(true)
            .write(true)
            .open(path)
            .await?;

        let handle = tokio::spawn(
            async move {
                while let Ok(data) = receiver.recv_async().await {
                    file.write_all(&data).await?;
                }
                file.set_permissions(Permissions::from_mode(mode)).await?;
                Ok(())
            }
            .in_current_span(),
        );

        let mut files = self.files.lock().await;
        files.insert(id, handle);

        Ok(())
    }

    async fn end_file(&self, id: u32) -> std::result::Result<(), PeerError> {
        let mut files = self.files.lock().await;
        let file = files
            .remove(&id)
            .ok_or_else(|| PeerError::Other(format!("file {id:x} has not been started")))?;
        drop(files);

        file.await
            .map_err(|e| std::io::Error::new(ErrorKind::ConnectionAborted, e))
            .flatten()?;
        // tracing::trace!(id, "closed file");
        Ok(())
    }
}
