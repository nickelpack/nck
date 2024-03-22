use std::{
    fs::{OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{symlink, PermissionsExt as _},
    path::{Path, PathBuf},
};

use nix::{
    mount::MsFlags,
    unistd::{Gid, Uid},
};

use super::fs::{bind, mount, MountType, SYS_NONE};

#[derive(Debug)]
struct RootFs {
    root: PathBuf,
    etc: PathBuf,
    dev: PathBuf,
    tmp: PathBuf,
    sys: PathBuf,
    proc: PathBuf,
}

impl RootFs {
    fn new(root: PathBuf) -> Self {
        Self {
            etc: root.join("etc"),
            tmp: root.join("tmp"),
            dev: root.join("dev"),
            sys: root.join("sys"),
            proc: root.join("proc"),
            root,
        }
    }

    pub fn add_group<G: AsRef<str>>(
        &self,
        name: impl AsRef<str>,
        gid: Gid,
        user_list: impl IntoIterator<Item = G>,
    ) -> anyhow::Result<()> {
        let etc_group = self.etc.join("group");
        let name = name.as_ref();
        let mut w = OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .open(etc_group)?;
        write!(w, "{name}:!:{gid}:")?;
        let mut first = true;
        for user in user_list {
            if !first {
                w.write_all(b",")?;
            }
            first = false;
            let g = user.as_ref();
            w.write_all(g.as_bytes())?;
        }
        w.write_all(b"\n")?;
        Ok(())
    }

    pub fn add_user<H: AsRef<str>>(
        &self,
        name: impl AsRef<str>,
        uid: Uid,
        gid: Gid,
        home: Option<H>,
    ) -> anyhow::Result<()> {
        let etc_passwd = self.etc.join("passwd");
        let name = name.as_ref();
        let home = home.as_ref().map(|a| a.as_ref()).unwrap_or("/");
        let mut w = OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .open(etc_passwd)?;
        write!(w, "{name}:x:{uid}:{gid}:{home}:/sbin/nologin\n")?;
        Ok(())
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub fn install(&self) -> anyhow::Result<()> {
        tracing::trace!(root = ?self.root, "initializing rootfs directory");
        std::fs::create_dir(&self.root)?;
        std::fs::set_permissions(&self.root, Permissions::from_mode(0o774))?;

        tracing::trace!("creating /etc");
        std::fs::create_dir_all(self.etc.as_path())?;

        self.add_group::<&str>("root", Gid::from_raw(0), []);
        self.add_group::<&str>("nogroup", Gid::from_raw(65534), []);

        self.add_user::<&str>("root", Uid::from_raw(0), Gid::from_raw(0), None);
        self.add_user::<&str>("nobody", Uid::from_raw(65534), Gid::from_raw(65534), None);

        tracing::trace!("creating /etc/hosts");
        let etc_hosts = self.etc.join("hosts");
        std::fs::write(etc_hosts, "127.0.0.1 localhost\n::1 localhost\n")?;

        tracing::trace!("creating /dev/pts");
        let dev_pts = self.dev.join("pts");
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
                bind("/dev/pts", &dev_pts, None, None)?;
                bind("/dev/ptmx", self.dev.join("ptmx"), None, None)?;
            } else {
                let ptmx = self.dev.join("ptmx");
                symlink("/dev/pts/ptmx", ptmx)?;
                std::fs::set_permissions(self.dev.join("pts/ptmx"), Permissions::from_mode(0o620))?;
            }
        }

        tracing::trace!("creating /dev/shm");
        let dev_shm = self.dev.join("shm");
        std::fs::create_dir_all(&dev_shm)?;
        mount(
            SYS_NONE,
            &dev_shm,
            Some(&MountType::TmpFs),
            MsFlags::empty(),
            SYS_NONE,
        )?;

        tracing::trace!("creating /dev/sys");
        std::fs::create_dir_all(&self.sys)?;
        // Likely to fail in rootlees
        if let Err(error) = mount(
            SYS_NONE,
            &self.sys,
            Some(&MountType::SysFs),
            MsFlags::empty(),
            SYS_NONE,
        ) {
            tracing::debug!(?error, "failed to mount /sys, falling back to a bind");
            bind("/sys", &self.sys, None, None)?;
        }

        tracing::trace!("creating /proc");
        std::fs::create_dir_all(&self.proc)?;
        mount(
            SYS_NONE,
            &self.proc,
            Some(&MountType::Proc),
            MsFlags::empty(),
            SYS_NONE,
        )?;

        tracing::trace!("creating /dev/null");
        bind("/dev/null", self.dev.join("null"), None, None)?;
        tracing::trace!("creating /dev/zero");
        bind("/dev/zero", self.dev.join("zero"), None, None)?;
        tracing::trace!("creating /dev/full");
        bind("/dev/full", self.dev.join("full"), None, None)?;
        tracing::trace!("creating /dev/random");
        bind("/dev/random", self.dev.join("random"), None, None)?;
        tracing::trace!("creating /dev/urandom");
        bind("/dev/urandom", self.dev.join("urandom"), None, None)?;

        tracing::trace!("symlinks fds");
        symlink("/proc/self/fd", self.dev.join("fd"))?;
        symlink("/proc/self/fd/0", self.dev.join("stdin"))?;
        symlink("/proc/self/fd/1", self.dev.join("stdout"))?;
        symlink("/proc/self/fd/2", self.dev.join("stderr"))?;

        Ok(())
    }
}
