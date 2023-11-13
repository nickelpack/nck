// Much of this comes from youki. Would have used that directly were it decoupled from OCI.

use std::{
    fs::create_dir_all,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use nix::{
    mount::MsFlags,
    sched::CloneFlags,
    sys::stat::Mode,
    unistd::{Gid, Pid, Uid},
};
use tokio::{
    net::{UnixListener, UnixStream},
    time::timeout,
};
use tokio_util::{compat::TokioAsyncReadCompatExt, sync::CancellationToken};

mod child;
mod fs;
mod parent;
mod proc;
mod user;

use crate::ipc::proto::{ChildMessage, ParentMessage};

use super::SandboxOptions;

#[derive(Debug, Clone)]
struct Shared {
    path: PathBuf,
    uid: Uid,
    gid: Gid,
    network_access: bool,

    root_path: PathBuf,
    socket_path: PathBuf,
}

impl From<SandboxOptions> for Shared {
    fn from(value: SandboxOptions) -> Self {
        Self {
            root_path: value.path.join("rootfs"),
            socket_path: value.path.join("parent.sock"),

            path: value.path,
            uid: value
                .uid
                .map(Uid::from_raw)
                .unwrap_or_else(|| nix::unistd::getuid()),
            gid: value
                .gid
                .map(Gid::from_raw)
                .unwrap_or_else(|| nix::unistd::getgid()),
            network_access: value.network_access,
        }
    }
}

fn child_main(shared: Shared) -> isize {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let end = Instant::now() + Duration::from_secs(1);
            let stream = loop {
                match UnixStream::connect(&shared.socket_path).await {
                    Ok(s) => {
                        break s;
                    }
                    Err(e) if end < Instant::now() => {
                        panic!(
                            "failed to connect to socket at {:?}: {:?}",
                            &shared.socket_path, e
                        );
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(20)).await
            };
            let ipc = crate::ipc::create(stream.compat());

            let mut client = SandboxChild { shared, ipc };
            client.start().await
        })
}

struct SandboxChild {
    shared: Shared,
    ipc: crate::ipc::IpcConnection<
        ChildMessage,
        ParentMessage,
        tokio_util::compat::Compat<UnixStream>,
    >,
}

impl SandboxChild {
    async fn start(&mut self) -> isize {
        let result: Result<()> = async {
            self.ipc.send(ParentMessage::Hello).await?;
            self.initial_setup()?;
            Ok(())
        }
        .await;

        match result {
            Ok(_) => {
                self.ipc.send(ParentMessage::Exit { code: 0 }).await.ok();
                0
            }
            Err(e) => {
                // TODO: Write this to a file if the IPC fails
                self.ipc
                    .send(ParentMessage::Fatal {
                        reason: format!("{:?}", e),
                    })
                    .await
                    .ok();
                1
            }
        }
    }

    fn initial_setup(&self) -> Result<()> {
        self.setup_root_fs()?;
        fs::make_root_private(&self.shared.root_path)?;
        fs::chroot(&self.shared.root_path)?;
        proc::disable_aslr()?;
        // user::set_id(self.shared.uid, self.shared.gid, &self.shared.gids)?;
        Ok(())
    }

    fn setup_root_fs(&self) -> Result<()> {
        let root = self.shared.root_path.as_path();
        create_dir_all(root)?;
        fs::chmod(root, Mode::from_bits_truncate(0o700))?;
        // fs::chown(root, Some(self.shared.uid), Some(self.shared.gid))?;

        let tmp = root.join("tmp");
        create_dir_all(&tmp)?;
        fs::mount(
            None,
            &tmp,
            Some(fs::MountType::TmpFs),
            MsFlags::empty(),
            None::<&str>,
        )?;

        let etc = root.join("etc");
        create_dir_all(&etc)?;
        // fs::chown(&etc, Some(self.shared.uid), Some(self.shared.gid))?;

        let etc_group = etc.join("group");
        std::fs::write(
            etc_group,
            format!(
                "root:x:0:\nnixbld:!:{}:\nnogroup:x:65534:\n",
                self.shared.uid.as_raw()
            ),
        )?;

        let etc_hosts = etc.join("hosts");
        std::fs::write(&etc_hosts, "127.0.0.1 localhost\n::1 localhost\n")?;

        let dev = root.join("dev");

        let dev_pts = dev.join("pts");
        create_dir_all(&dev_pts)?;

        if fs::mount(
            None,
            &dev_pts,
            Some(fs::MountType::DevPts),
            MsFlags::empty(),
            Some("newinstance,mode=0620"),
        )
        .is_ok()
        {
            fs::symlink(Path::new("/dev/pts/ptmx"), &dev.join("ptmx"))?;
            fs::chmod(&dev.join("pts/ptmx"), Mode::from_bits_truncate(0o666))?;
        } else {
            fs::bind(Path::new("/dev/pts"), &dev_pts)?;
            create_dir_all(&dev.join("dev/pts"))?;
            fs::bind(Path::new("/dev/pts"), &dev_pts)?;
        }

        let dev_shm = dev.join("shm");
        create_dir_all(&dev_shm)?;
        fs::mount(
            None,
            &dev_shm,
            Some(fs::MountType::TmpFs),
            MsFlags::empty(),
            None::<&str>,
        )?;

        let sys = root.join("sys");
        create_dir_all(&sys).unwrap();
        fs::mount(
            None,
            &sys,
            Some(fs::MountType::SysFs),
            MsFlags::empty(),
            None::<&str>,
        )?;

        let proc = root.join("proc");
        create_dir_all(&proc).unwrap();
        fs::mount(
            None,
            &proc,
            Some(fs::MountType::Proc),
            MsFlags::empty(),
            None::<&str>,
        )?;

        fs::bind(Path::new("/dev/null"), &root.join("dev/null"))?;
        fs::bind(Path::new("/dev/zero"), &root.join("dev/zero"))?;
        fs::bind(Path::new("/dev/full"), &root.join("dev/full"))?;
        fs::bind(Path::new("/dev/random"), &root.join("dev/random"))?;
        fs::bind(Path::new("/dev/urandom"), &root.join("dev/urandom"))?;
        fs::bind(Path::new("/dev/tty"), &root.join("dev/tty"))?;

        fs::symlink(Path::new("/proc/self/fd"), &root.join("dev/fd"))?;
        fs::symlink(Path::new("/proc/self/fd/0"), &root.join("dev/stdin"))?;
        fs::symlink(Path::new("/proc/self/fd/1"), &root.join("dev/stdout"))?;
        fs::symlink(Path::new("/proc/self/fd/2"), &root.join("dev/stderr"))?;

        Ok(())
    }
}
