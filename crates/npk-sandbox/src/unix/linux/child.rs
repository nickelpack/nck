use std::{
    fs::{create_dir_all, set_permissions, write, Permissions},
    os::unix::{fs::symlink, prelude::*},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use nix::{
    mount::MsFlags,
    sched::CloneFlags,
    unistd::{Gid, Uid},
};
use npk_util::io::{timeout_async, wait_for_file_async};
use tokio::net::UnixStream;

use crate::unix::SOCKET_TIMEOUT;

use super::{
    fs::{bind, mount, MountType},
    zygote::SpawnRequest,
};

pub async fn main(
    req: SpawnRequest,
    sandbox_path: PathBuf,
    socket_path: PathBuf,
    rootfs_path: PathBuf,
) -> isize {
    async fn imp(
        req: SpawnRequest,
        sandbox_path: PathBuf,
        socket_path: PathBuf,
        rootfs_path: PathBuf,
    ) -> Result<()> {
        wait_for_file_async(socket_path.as_path())
            .await
            .with_context(|| {
                "while waiting for the controller socket to appear in the filesystem"
            })?;

        let socket = timeout_async(SOCKET_TIMEOUT, UnixStream::connect(socket_path.as_path()))
            .await
            .with_context(|| "while connecting to controller process")?;

        super::user::set_id(Uid::from_raw(0), Gid::from_raw(0), Vec::default())
            .with_context(|| "while updating the user and group ids")?;

        super::proc::disable_aslr().with_context(|| "while disabling ASLR")?;
        super::proc::set_hostname("localhost").with_context(|| "while setting the hostname")?;

        init_rootfs(rootfs_path.as_path()).with_context(|| "while initializing sandbox")?;

        tracing::debug!("sandbox initialized");

        Ok(())
    }
    if let Err(e) = imp(req, sandbox_path, socket_path, rootfs_path).await {
        eprintln!("sandbox failed: {:?}", e);
        -1
    } else {
        0
    }
}

fn init_rootfs(root: &Path) -> Result<()> {
    create_dir_all(root).with_context(|| "while creating the directory for the root fs")?;
    set_permissions(root, Permissions::from_mode(0o700))
        .with_context(|| "while changing the root permissions")?;

    let tmp = root.join("tmp");
    create_dir_all(tmp.as_path()).with_context(|| "while creating /tmp")?;
    mount(
        None::<&str>,
        &tmp,
        Some(MountType::TmpFs),
        MsFlags::empty(),
        None::<&str>,
    )
    .with_context(|| "while mounting a tmp FS at /tmp")?;

    let etc = root.join("etc");
    create_dir_all(etc.as_path()).with_context(|| "while creating /etc")?;

    let etc_group = etc.join("group");
    write(etc_group, "root:x:0:\nbuilder:!:1000:\nnogroup:x:65534:\n")
        .with_context(|| "while creating /etc/group")?;

    let etc_passwd = etc.join("passwd");
    write(etc_passwd, "root:x:0:0:root:/build:/noshell\nbuilder:x:1000:1000:builder:/build:/noshell\nnobody:x:65534:65534:Nobody:/:/noshell\n")
        .with_context(|| "while creating /etc/group")?;

    let etc_hosts = etc.join("hosts");
    std::fs::write(etc_hosts, "127.0.0.1 localhost\n::1 localhost\n")
        .with_context(|| "while creating /etc/hosts")?;

    let dev = root.join("dev");

    let dev_pts = dev.join("pts");
    create_dir_all(&dev_pts).with_context(|| "while creating /dev/pts")?;

    if Path::new("/dev/pts/ptmx").exists() {
        if mount(
            None::<&str>,
            &dev_pts,
            Some(MountType::DevPts),
            MsFlags::empty(),
            Some("newinstance,mode=0620"),
        )
        .is_ok()
        {
            let ptmx = dev.join("ptmx");
            symlink("/dev/pts/ptmx", ptmx).with_context(|| "while creating /dev/ptmx")?;
            set_permissions(dev.join("dev/pts/ptmx"), Permissions::from_mode(0o666)).ok();
        } else {
            bind(Path::new("/dev/pts"), &dev_pts)
                .with_context(|| "while binding /dev/pts to the host /dev/pts")?;
            bind(Path::new("/dev/ptmx"), dev.join("ptmx"))
                .with_context(|| "while binding /dev/pts to the host /dev/pts")?;
        }
    }

    let dev_shm = dev.join("shm");
    create_dir_all(&dev_shm).with_context(|| "while creating /dev/shm")?;
    mount(
        None::<&str>,
        &dev_shm,
        Some(MountType::TmpFs),
        MsFlags::empty(),
        None::<&str>,
    )
    .with_context(|| "while mounting /dev/shm")?;

    let sys = root.join("sys");
    create_dir_all(&sys).with_context(|| "while creating /sys")?;
    // Likely to fail in rootlees
    if let Err(err) = mount(
        None::<&str>,
        &sys,
        Some(MountType::SysFs),
        MsFlags::empty(),
        None::<&str>,
    ) {
        tracing::debug!("failed to mount /sys, falling back to a bind: {}", err);
        bind("/sys", &sys).with_context(|| "while binding /sys")?;
    }

    let proc = root.join("proc");
    create_dir_all(&proc).with_context(|| "while creating /proc")?;
    mount(
        None::<&str>,
        &proc,
        Some(MountType::Proc),
        MsFlags::empty(),
        None::<&str>,
    )
    .with_context(|| "while mounting /proc")?;

    bind(Path::new("/dev/null"), root.join("dev/null"))
        .with_context(|| "while binding /dev/null")?;
    bind(Path::new("/dev/zero"), root.join("dev/zero"))
        .with_context(|| "while binding /dev/zero")?;
    bind(Path::new("/dev/full"), root.join("dev/full"))
        .with_context(|| "while binding /dev/full")?;
    bind(Path::new("/dev/random"), root.join("dev/random"))
        .with_context(|| "while binding /dev/random")?;
    bind(Path::new("/dev/urandom"), root.join("dev/urandom"))
        .with_context(|| "while binding /dev/urandom")?;

    symlink("/proc/self/fd", root.join("dev/fd"))?;
    symlink("/proc/self/fd/0", root.join("dev/stdin"))?;
    symlink("/proc/self/fd/1", root.join("dev/stdout"))?;
    symlink("/proc/self/fd/2", root.join("dev/stderr"))?;

    Ok(())
}
