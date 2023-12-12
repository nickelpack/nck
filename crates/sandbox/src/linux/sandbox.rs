use std::path::{Path, PathBuf};

use nck_core::io::Timeout;
use nix::{
    mount::MsFlags,
    sys::{personality::Persona, stat::Mode},
};
use tokio::net::UnixStream;

use super::{
    syscall::{MountType, Result, Syscall, SYS_NONE},
    SOCKET_TIMEOUT,
};

pub struct SandboxArgs {
    pub formula_path: PathBuf,
    pub socket_path: PathBuf,
    pub rootfs_path: PathBuf,
    pub store_directory: PathBuf,
}

#[tracing::instrument(level = "trace", skip_all, err(Debug), parent = None)]
pub async fn sandbox_main<SC: Syscall + 'static>(args: SandboxArgs) -> Result<()> {
    if let Err(error) = prctl::set_name(&args.formula_path.to_string_lossy()) {
        let error = nix::Error::from_i32(error);
        tracing::warn!(?error, "failed to set sandbox process name");
    }

    tracing::trace!(?args.rootfs_path, "initializing rootfs");
    init_rootfs::<SC>(args.rootfs_path.as_path(), args.store_directory.as_path())?;

    tracing::trace!("disabling ASLR");
    SC::change_personality(|p| p | Persona::ADDR_NO_RANDOMIZE)?;

    tracing::trace!("setting hostname to localhost");
    SC::set_hostname("localhost")?;

    tracing::trace!("connecting to controller");

    let socket = SOCKET_TIMEOUT
        .timeout_async(UnixStream::connect(args.socket_path.as_path()))
        .await?;

    tracing::trace!("connected to controller");

    todo!();

    // let sandbox = SandboxProcess::<SC> {
    //     rootfs_path,
    //     socket,
    //     _phantom: PhantomData,
    // };

    // match sandbox.run().in_current_span().await {
    //     Ok(_) => Ok(()),
    //     Err(SyscallError::IoError(e)) if e.kind() == ErrorKind::ConnectionAborted => Ok(()),
    //     other => other,
    // }
}

#[tracing::instrument(level = "trace", skip_all)]
fn init_rootfs<SC: Syscall>(root: &Path, store_directory: &Path) -> Result<()> {
    tracing::trace!(?root, "initializing rootfs directory");
    SC::chmod(root, Mode::from_bits_truncate(0o700))?;

    tracing::trace!("creating store directory");
    SC::bind(
        store_directory,
        root.join(store_directory.strip_prefix("/").unwrap_or(store_directory)),
        Some(MsFlags::MS_RDONLY),
    )?;

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
            SC::bind("/dev/pts", &dev_pts, None)?;
            SC::bind("/dev/ptmx", dev.join("ptmx"), None)?;
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
        SC::bind("/sys", &sys, None)?;
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
    SC::bind("/dev/null", dev.join("null"), None)?;
    tracing::trace!("creating /dev/zero");
    SC::bind("/dev/zero", dev.join("zero"), None)?;
    tracing::trace!("creating /dev/full");
    SC::bind("/dev/full", dev.join("full"), None)?;
    tracing::trace!("creating /dev/random");
    SC::bind("/dev/random", dev.join("random"), None)?;
    tracing::trace!("creating /dev/urandom");
    SC::bind("/dev/urandom", dev.join("urandom"), None)?;

    tracing::trace!("symlinks fds");
    SC::symlink("/proc/self/fd", dev.join("fd"))?;
    SC::symlink("/proc/self/fd/0", dev.join("stdin"))?;
    SC::symlink("/proc/self/fd/1", dev.join("stdout"))?;
    SC::symlink("/proc/self/fd/2", dev.join("stderr"))?;

    Ok(())
}
