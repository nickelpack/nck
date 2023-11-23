use std::{
    fs::{create_dir_all, set_permissions, write, Permissions},
    os::unix::{fs::symlink, prelude::*},
    path::{Path, PathBuf},
};

use nix::{
    mount::{mount, MsFlags},
    sys::personality::Persona,
    unistd::{sethostname, Gid, Uid},
};
use npk_util::io::{timeout_async, wait_for_file_async};
use tokio::net::UnixStream;

use crate::unix::SOCKET_TIMEOUT;

use super::{
    fs::{bind, MountType},
    zygote::SpawnRequest,
    NIX_NONE,
};

pub async fn main(
    req: SpawnRequest,
    sandbox_path: PathBuf,
    socket_path: PathBuf,
    rootfs_path: PathBuf,
) -> isize {
    #[tracing::instrument(name = "child_main", level = "trace", skip_all, fields(?sandbox_path), err(Debug))]
    async fn imp(
        _req: SpawnRequest,
        sandbox_path: PathBuf,
        socket_path: PathBuf,
        rootfs_path: PathBuf,
    ) -> nix::Result<()> {
        tracing::trace!(
            ?socket_path,
            "waiting for controller socket to appear on the filesystem"
        );

        wait_for_file_async(socket_path.as_path())
            .await
            .map_err(super::std_error_to_nix)?;

        tracing::trace!("connecting to controller");

        let _socket = timeout_async(SOCKET_TIMEOUT, UnixStream::connect(socket_path.as_path()))
            .await
            .map_err(super::std_error_to_nix)?;

        tracing::info!("connected to controller");

        tracing::trace!("becoming root");

        super::user::set_id(Uid::from_raw(0), Gid::from_raw(0), Vec::default())?;

        tracing::trace!("disabling ASLR");
        super::proc::change_personality(|p| p | Persona::ADDR_NO_RANDOMIZE)?;

        tracing::trace!("setting hostname to localhost");
        sethostname("localhost")?;

        tracing::trace!(?rootfs_path, "initializing rootfs");
        init_rootfs(rootfs_path.as_path())?;

        tracing::info!("sandbox initialized");

        super::fs::chroot(rootfs_path.as_path())?;

        Ok(())
    }

    if let Err(error) = imp(req, sandbox_path, socket_path, rootfs_path).await {
        tracing::error!(?error, "failed to start sandbox");
        -1
    } else {
        0
    }
}

#[tracing::instrument(level = "trace", err(Debug))]
fn init_rootfs(root: &Path) -> nix::Result<()> {
    tracing::trace!("creating rootfs dir");
    create_dir_all(root).map_err(super::std_error_to_nix)?;
    set_permissions(root, Permissions::from_mode(0o700)).map_err(super::std_error_to_nix)?;

    tracing::trace!("creating /tmp");
    let tmp = root.join("tmp");
    create_dir_all(tmp.as_path()).map_err(super::std_error_to_nix)?;
    mount(
        NIX_NONE,
        &tmp,
        Some(&MountType::TmpFs),
        MsFlags::empty(),
        NIX_NONE,
    )?;

    tracing::trace!("creating /etc");
    let etc = root.join("etc");
    create_dir_all(etc.as_path()).map_err(super::std_error_to_nix)?;

    tracing::trace!("creating /etc/group");
    let etc_group = etc.join("group");
    write(etc_group, "root:x:0:\nbuilder:!:1000:\nnogroup:x:65534:\n")
        .map_err(super::std_error_to_nix)?;

    tracing::trace!("creating /etc/passwd");
    let etc_passwd = etc.join("passwd");
    write(etc_passwd, "root:x:0:0:root:/build:/noshell\nbuilder:x:1000:1000:builder:/build:/noshell\nnobody:x:65534:65534:Nobody:/:/noshell\n")
        .map_err(super::std_error_to_nix)?;

    tracing::trace!("creating /etc/hosts");
    let etc_hosts = etc.join("hosts");
    std::fs::write(etc_hosts, "127.0.0.1 localhost\n::1 localhost\n")
        .map_err(super::std_error_to_nix)?;

    tracing::trace!("creating /etc/dev");
    let dev = root.join("dev");

    tracing::trace!("creating /etc/pts");
    let dev_pts = dev.join("pts");
    create_dir_all(&dev_pts).map_err(super::std_error_to_nix)?;

    if Path::new("/dev/pts/ptmx").exists() {
        tracing::trace!("creating /dev/pts");
        if let Err(error) = mount(
            NIX_NONE,
            &dev_pts,
            Some(&MountType::DevPts),
            MsFlags::empty(),
            Some("newinstance,mode=0620"),
        ) {
            tracing::debug!(?error, "failed to mount devpts, falling back to bind");
            bind("/dev/pts", &dev_pts)?;
            bind("/dev/ptmx", dev.join("ptmx"))?;
        } else {
            let ptmx = dev.join("ptmx");
            symlink("/dev/pts/ptmx", ptmx).map_err(super::std_error_to_nix)?;
            set_permissions(dev.join("dev/pts/ptmx"), Permissions::from_mode(0o666)).ok();
        }
    }

    tracing::trace!("creating /dev/shm");
    let dev_shm = dev.join("shm");
    create_dir_all(&dev_shm).map_err(super::std_error_to_nix)?;
    mount(
        NIX_NONE,
        &dev_shm,
        Some(&MountType::TmpFs),
        MsFlags::empty(),
        NIX_NONE,
    )?;

    tracing::trace!("creating /dev/sys");
    let sys = root.join("sys");
    create_dir_all(&sys).map_err(super::std_error_to_nix)?;
    // Likely to fail in rootlees
    if let Err(error) = mount(
        NIX_NONE,
        &sys,
        Some(&MountType::SysFs),
        MsFlags::empty(),
        NIX_NONE,
    ) {
        tracing::debug!(?error, "failed to mount /sys, falling back to a bind");
        bind("/sys", &sys)?;
    }

    tracing::trace!("creating /proc");
    let proc = root.join("proc");
    create_dir_all(&proc).map_err(super::std_error_to_nix)?;
    mount(
        NIX_NONE,
        &proc,
        Some(&MountType::Proc),
        MsFlags::empty(),
        NIX_NONE,
    )?;

    tracing::trace!("creating /dev/null");
    bind("/dev/null", dev.join("null"))?;
    tracing::trace!("creating /dev/zero");
    bind("/dev/zero", dev.join("zero"))?;
    tracing::trace!("creating /dev/full");
    bind("/dev/full", dev.join("full"))?;
    tracing::trace!("creating /dev/random");
    bind("/dev/random", dev.join("random"))?;
    tracing::trace!("creating /dev/urandom");
    bind("/dev/urandom", dev.join("urandom"))?;

    tracing::trace!("symlinks fds");
    symlink("/proc/self/fd", dev.join("fd")).map_err(super::std_error_to_nix)?;
    symlink("/proc/self/fd/0", dev.join("stdin")).map_err(super::std_error_to_nix)?;
    symlink("/proc/self/fd/1", dev.join("stdout")).map_err(super::std_error_to_nix)?;
    symlink("/proc/self/fd/2", dev.join("stderr")).map_err(super::std_error_to_nix)?;

    Ok(())
}
