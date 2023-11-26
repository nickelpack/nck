use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
};

use nix::{
    mount::MsFlags,
    sys::{personality::Persona, stat::Mode},
};
use npk_util::io::timeout_async;
use remoc::rtc;
use tokio::net::UnixStream;

use crate::unix::SOCKET_TIMEOUT;

use super::{
    proto::SandboxError,
    syscall::{MountType, Result, Syscall, SYS_NONE},
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

    let sandbox = SandboxProcess {
        rootfs_path,
        remote: None,
        _phantom: PhantomData,
    };
    let (server, client) = super::proto::connect::<
        super::proto::SandboxProcessServerSharedMut<SandboxProcess<SC>, _>,
        SandboxProcess<SC>,
        super::proto::ControllerProcessClient,
    >(socket, sandbox, 1)
    .await?;

    {
        let mut sandbox = server.write().await;
        sandbox.remote = Some(client);
    }

    match server.wait().await {
        Ok(_) => {
            tracing::info!("controller disconnected");
            Ok(())
        }
        Err(error) => {
            tracing::error!(?error, "RPC failed");
            Err(crate::current::flavor::syscall::SyscallError::OsError(
                nix::errno::Errno::EPIPE,
            ))
        }
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

#[derive(Debug)]
struct SandboxProcess<SC: Syscall> {
    rootfs_path: PathBuf,
    remote: Option<super::proto::ControllerProcessClient>,
    _phantom: PhantomData<SC>,
}

#[rtc::async_trait]
impl<SC: Syscall> super::proto::SandboxProcess for SandboxProcess<SC> {
    async fn isolate_network(&mut self) -> std::result::Result<(), SandboxError> {
        Ok(())
    }
    async fn isolate_filesystem(&mut self) -> std::result::Result<(), SandboxError> {
        SC::pivot(self.rootfs_path.as_path()).map_err(|_| SandboxError::IoError)?;
        Ok(())
    }
}
