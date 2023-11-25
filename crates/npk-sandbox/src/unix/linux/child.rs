use std::{
    fs::{create_dir_all, set_permissions, write, Permissions},
    os::unix::{fs::symlink, prelude::PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use nix::{
    mount::{mount, MsFlags},
    sched::{unshare, CloneFlags},
    sys::personality::Persona,
    unistd::{fork, sethostname, ForkResult, Gid, Pid, Uid},
};
use npk_util::io::{timeout, timeout_async, wait_for_file, TempDir};
use remoc::rtc;
use signal_hook::{
    consts::{SIGCHLD, SIGHUP, SIGINT, SIGQUIT, SIGTERM},
    iterator::Signals,
};
use tokio::net::UnixStream;

use crate::{
    current::flavor::{fs::TempMount, proc::ChildProcess},
    unix::SOCKET_TIMEOUT,
};

use super::{
    fs::{bind, MountType},
    proto::SandboxError,
    zygote::SpawnRequest,
    NIX_NONE,
};

#[tracing::instrument(level = "trace", skip_all, fields(name = req.name()))]
pub fn main(req: SpawnRequest, sandbox_path: PathBuf, socket_path: PathBuf) -> isize {
    fn wait_for_controller(sandbox_path: &Path, socket_path: &Path) -> nix::Result<TempMount> {
        tracing::trace!("entering remaining namespaces");
        let flags = CloneFlags::CLONE_NEWPID
            | CloneFlags::CLONE_NEWUTS
            | CloneFlags::CLONE_NEWCGROUP
            | CloneFlags::CLONE_NEWIPC;
        unshare(flags)?;

        tracing::trace!(
            ?socket_path,
            "waiting for the controller socket to appear on the filesystem"
        );
        timeout(Duration::from_secs(5), || wait_for_file(socket_path))
            .map_err(super::std_error_to_nix)?;

        // The zugote is in charge of newuidmap/newgidmap, so if the controller socket has appeared on the filesystem
        // it means that the mapping has occurred and the result has been received by the controller.
        tracing::trace!("becoming root");
        super::user::set_id(Uid::from_raw(0), Gid::from_raw(0), Vec::default())?;

        tracing::trace!("creating rootfs directory");
        let temp = TempDir::new_in(sandbox_path).map_err(super::std_error_to_nix)?;

        tracing::trace!("mounting rootfs directory as tmpfs");
        temp.try_into()
    }

    let rootfs_dir = match wait_for_controller(sandbox_path.as_path(), socket_path.as_path()) {
        Err(error) => {
            tracing::error!(?error, "failed to initialize child process");
            return -1;
        }
        Ok(result) => result,
    };

    match unsafe { fork() } {
        Ok(ForkResult::Parent { child }) => supervisor(req.name(), child, rootfs_dir),
        Ok(ForkResult::Child) => sandbox(req.name(), socket_path, rootfs_dir.forget()),
        Err(error) => {
            tracing::error!(
                ?error,
                "failed to fork child process to supervisor and sandbox"
            );
            -1
        }
    }
}

#[tracing::instrument(level = "trace", skip_all)]
fn supervisor(name: &str, child: Pid, rootfs_dir: TempMount) -> isize {
    fn imp(name: &str, child: ChildProcess, rootfs_dir: TempMount) -> nix::Result<()> {
        if let Err(error) = prctl::set_name(format!("super-{}", name).as_str()) {
            let error = nix::Error::from_i32(error);
            tracing::warn!(?error, "failed to set supervisor process name");
        }

        tracing::trace!("waiting for a signal");
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGQUIT, SIGHUP, SIGCHLD])
            .map_err(super::std_error_to_nix)?;

        match signals.forever().next() {
            Some(SIGINT) => tracing::trace!("got SIGINT"),
            Some(SIGTERM) => tracing::trace!("got SIGTERM"),
            Some(SIGQUIT) => tracing::trace!("got SIGQUIT"),
            Some(SIGCHLD) => tracing::trace!("child process exited"),
            Some(other) => tracing::trace!(other, "got unknown signal"),
            None => {}
        }

        tracing::trace!("cleaning up filesystem");
        // Ensure that NLL doesn't drop these early
        drop(rootfs_dir);
        drop(child);
        Ok(())
    }

    let child_proc: ChildProcess = child.into();
    if let Err(error) = imp(name, child_proc, rootfs_dir) {
        tracing::error!(?error, "supervisor failed");
        -1
    } else {
        0
    }
}

#[tracing::instrument(level = "trace", skip_all)]
fn sandbox(name: &str, socket_path: PathBuf, rootfs_path: PathBuf) -> isize {
    async fn imp(name: &str, socket_path: PathBuf, rootfs_path: PathBuf) -> nix::Result<()> {
        if let Err(error) = prctl::set_name(name) {
            let error = nix::Error::from_i32(error);
            tracing::warn!(?error, "failed to set sandbox process name");
        }

        tracing::trace!(?rootfs_path, "initializing rootfs");
        init_rootfs(rootfs_path.as_path())?;

        tracing::trace!("disabling ASLR");
        super::proc::change_personality(|p| p | Persona::ADDR_NO_RANDOMIZE)?;

        tracing::trace!("setting hostname to localhost");
        sethostname("localhost")?;

        tracing::trace!("connecting to controller");

        let socket = timeout_async(SOCKET_TIMEOUT, UnixStream::connect(socket_path.as_path()))
            .await
            .map_err(super::std_error_to_nix)?;

        let sandbox = SandboxProcess {
            rootfs_path,
            remote: None,
        };
        let (server, client) = super::proto::connect::<
            super::proto::SandboxProcessServerSharedMut<SandboxProcess, _>,
            SandboxProcess,
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
                Err(nix::Error::EPIPE)
            }
        }
    }

    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map(|runtime| runtime.block_on(imp(name, socket_path, rootfs_path)))
    {
        Ok(Err(error)) => {
            tracing::error!(?error, "sandbox process failed");
            -1
        }
        Err(error) => {
            tracing::error!(?error, "failed to start tokio runtime");
            -1
        }
        _ => 0,
    }
}

#[tracing::instrument(level = "trace", skip_all, err(Debug))]
fn init_rootfs(root: &Path) -> nix::Result<()> {
    tracing::trace!(?root, "initializing rootfs directory");
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

#[derive(Debug)]
struct SandboxProcess {
    rootfs_path: PathBuf,
    remote: Option<super::proto::ControllerProcessClient>,
}

#[rtc::async_trait]
impl super::proto::SandboxProcess for SandboxProcess {
    async fn isolate_network(&mut self) -> Result<(), SandboxError> {
        Ok(())
    }
    async fn isolate_filesystem(&mut self) -> Result<(), SandboxError> {
        tracing::trace!(
            "GGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGgg"
        );
        super::fs::pivot(self.rootfs_path.as_path())?;
        Ok(())
    }
}
