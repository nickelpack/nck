use std::{
    fs::{copy, set_permissions, Permissions},
    os::{
        fd::OwnedFd,
        unix::{fs::symlink, net::UnixStream, prelude::PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
};

use nck_spec::{LinkFlags, Spec};
use nix::mount::MsFlags;
use serde::{Deserialize, Serialize};

use crate::{
    build::linux::fs::{bind, mount, overlay, pivot, MountType, SYS_NONE},
    settings::StoreSettings,
};

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxRequest {}

#[derive(Debug, Serialize, Deserialize)]
pub enum SandboxResponse {}

#[derive(Debug)]
struct Paths {
    sandbox: PathBuf,
    root: PathBuf,
    store: PathBuf,
    store_working: PathBuf,
    results: PathBuf,
}

#[tracing::instrument(level = "trace", skip_all)]
pub fn sandbox_process(
    config: StoreSettings,
    sandbox_path: PathBuf,
    sandbox_peer: OwnedFd,
    spec_path: PathBuf,
) -> anyhow::Result<()> {
    let sandbox_peer: UnixStream = sandbox_peer.into();

    let spec = std::fs::read_to_string(spec_path.as_path())?;
    let spec = toml::from_str::<Spec>(&spec)?;

    let paths = Paths {
        sandbox: sandbox_path.clone(),
        root: sandbox_path.join("rootfs"),
        store_working: sandbox_path.join("store_working"),
        results: sandbox_path.join("results"),
        store: config.path.clone(),
    };

    tracing::trace!("creating root filesystem");
    init_rootfs(&paths)?;

    tracing::trace!("entering sandbox");
    // TODO: Apply other namespaces, e.g. networking
    pivot(&paths.root)?;

    tracing::trace!("executing spec");
    for op in spec.iterate_execution() {
        match op {
            nck_spec::ExecutionAction::Exec {
                path,
                args,
                env,
                work_dir,
            } => {
                tracing::trace!(?path, "executing command");
                let status = Command::new(path)
                    .env_clear()
                    .args(args)
                    .envs(env)
                    .current_dir(work_dir)
                    .status()?;
                if !status.success() {
                    tracing::error!(?status, "process exited with failure status code");
                    break;
                }
            }
            nck_spec::ExecutionAction::Link { from, to, flags } => {
                tracing::trace!(?from, ?to, "creating symlink");
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                if flags.is_empty() {
                    symlink(from, to)?;
                } else {
                    copy(from, &to)?;
                    let mut mode = 0o444u32;
                    if flags.intersects(LinkFlags::EXECUTABLE) {
                        mode |= 0o111;
                    }
                    set_permissions(to, Permissions::from_mode(mode))?;
                }
            }
        }
    }

    Ok(())
}

#[tracing::instrument(level = "trace", skip_all)]
fn init_rootfs(paths: &Paths) -> anyhow::Result<()> {
    tracing::trace!(root = ?paths.root, "initializing rootfs directory");
    std::fs::create_dir(&paths.root)?;
    std::fs::set_permissions(&paths.root, Permissions::from_mode(0o700))?;

    // We need to bind the dir into the FS, per overlayfs requirements.
    tracing::trace!("creating store directories");
    let store_path = paths
        .root
        .join(paths.store.strip_prefix("/").unwrap_or(&paths.store));
    std::fs::create_dir(&paths.store_working)?;
    std::fs::create_dir(&paths.results)?;
    std::fs::create_dir_all(&store_path)?;

    tracing::trace!("creating store mount");
    overlay(
        &paths.store,
        &paths.store_working,
        &paths.results,
        &store_path,
        MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME,
    )?;

    tracing::trace!("creating /tmp");
    let tmp = paths.root.join("tmp");
    std::fs::create_dir_all(tmp.as_path())?;
    mount(
        SYS_NONE,
        &tmp,
        Some(&MountType::TmpFs),
        MsFlags::empty(),
        SYS_NONE,
    )?;

    tracing::trace!("creating /etc");
    let etc = paths.root.join("etc");
    std::fs::create_dir_all(etc.as_path())?;

    tracing::trace!("creating /etc/group");
    let etc_group = etc.join("group");
    std::fs::write(etc_group, "root:x:0:\nbuilder:!:1000:\nnogroup:x:65534:\n")?;

    tracing::trace!("creating /etc/passwd");
    let etc_passwd = etc.join("passwd");
    std::fs::write(etc_passwd, "root:x:0:0:root:/build:/noshell\nbuilder:x:1000:1000:builder:/build:/noshell\nnobody:x:65534:65534:Nobody:/:/noshell\n")?;

    tracing::trace!("creating /etc/hosts");
    let etc_hosts = etc.join("hosts");
    std::fs::write(etc_hosts, "127.0.0.1 localhost\n::1 localhost\n")?;

    tracing::trace!("creating /etc/dev");
    let dev = paths.root.join("dev");

    tracing::trace!("creating /etc/pts");
    let dev_pts = dev.join("pts");
    std::fs::create_dir_all(&dev_pts)?;

    if Path::new("/dev/pts/ptmx").exists() {
        tracing::trace!("creating /dev/pts");
        if let Err(error) = mount(
            SYS_NONE,
            &dev_pts,
            Some(&MountType::DevPts),
            MsFlags::empty(),
            Some("newinstance,mode=0620"),
        ) {
            tracing::debug!(?error, "failed to mount devpts, falling back to bind");
            bind("/dev/pts", &dev_pts, None)?;
            bind("/dev/ptmx", dev.join("ptmx"), None)?;
        } else {
            let ptmx = dev.join("ptmx");
            symlink("/dev/pts/ptmx", ptmx)?;
            std::fs::set_permissions(dev.join("pts/ptmx"), Permissions::from_mode(0o666))?;
        }
    }

    tracing::trace!("creating /dev/shm");
    let dev_shm = dev.join("shm");
    std::fs::create_dir_all(&dev_shm)?;
    mount(
        SYS_NONE,
        &dev_shm,
        Some(&MountType::TmpFs),
        MsFlags::empty(),
        SYS_NONE,
    )?;

    tracing::trace!("creating /dev/sys");
    let sys = paths.root.join("sys");
    std::fs::create_dir_all(&sys)?;
    // Likely to fail in rootlees
    if let Err(error) = mount(
        SYS_NONE,
        &sys,
        Some(&MountType::SysFs),
        MsFlags::empty(),
        SYS_NONE,
    ) {
        tracing::debug!(?error, "failed to mount /sys, falling back to a bind");
        bind("/sys", &sys, None)?;
    }

    tracing::trace!("creating /proc");
    let proc = paths.root.join("proc");
    std::fs::create_dir_all(&proc)?;
    mount(
        SYS_NONE,
        &proc,
        Some(&MountType::Proc),
        MsFlags::empty(),
        SYS_NONE,
    )?;

    tracing::trace!("creating /dev/null");
    bind("/dev/null", dev.join("null"), None)?;
    tracing::trace!("creating /dev/zero");
    bind("/dev/zero", dev.join("zero"), None)?;
    tracing::trace!("creating /dev/full");
    bind("/dev/full", dev.join("full"), None)?;
    tracing::trace!("creating /dev/random");
    bind("/dev/random", dev.join("random"), None)?;
    tracing::trace!("creating /dev/urandom");
    bind("/dev/urandom", dev.join("urandom"), None)?;

    tracing::trace!("symlinks fds");
    symlink("/proc/self/fd", dev.join("fd"))?;
    symlink("/proc/self/fd/0", dev.join("stdin"))?;
    symlink("/proc/self/fd/1", dev.join("stdout"))?;
    symlink("/proc/self/fd/2", dev.join("stderr"))?;

    Ok(())
}
