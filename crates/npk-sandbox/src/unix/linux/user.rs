use std::{
    fs::{File, OpenOptions},
    io::Write,
    ops::RangeBounds,
    os::fd::{FromRawFd, OwnedFd},
    path::Path,
};

use anyhow::{Context, Result};
use nix::{
    fcntl::OFlag,
    sys::stat::Mode,
    unistd::{Gid, Pid, Uid},
};
use thiserror::Error;

pub fn set_id(uid: Uid, gid: Gid, supplementary: impl AsRef<[Gid]>) -> Result<()> {
    let supplementary = supplementary.as_ref();

    if !supplementary.is_empty() {
        setgroups(supplementary).with_context(|| "while setting all IDs of the current process")?;
    }
    setresgid(gid, gid, gid).with_context(|| "while setting all IDs of the current process")?;
    setresuid(uid, uid, uid).with_context(|| "while setting all IDs of the current process")?;

    Ok(())
}

pub fn setresgid(r: Gid, e: Gid, s: Gid) -> Result<()> {
    nix::unistd::setresgid(r, e, s).with_context(|| {
        format!(
            "while setting the real, effective, and saved GID to {:?}, {:?}, and {:?}",
            r, e, s
        )
    })
}

pub fn setresuid(r: Uid, e: Uid, s: Uid) -> Result<()> {
    nix::unistd::setresuid(r, e, s).with_context(|| {
        format!(
            "while setting the real, effective, and saved UID to {:?}, {:?}, and {:?}",
            r, e, s
        )
    })
}

pub fn setgroups(g: impl AsRef<[Gid]>) -> Result<()> {
    let g = g.as_ref();
    nix::unistd::setgroups(g).with_context(|| {
        format!(
            "while setting the supplementary groups to {:?}",
            g.iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

pub fn getuid() -> Uid {
    nix::unistd::getuid()
}

pub fn getgid() -> Gid {
    nix::unistd::getgid()
}

pub fn map_direct(uid: Uid, gid: Gid, set_groups: bool) -> Result<()> {
    fn open(
        path: impl AsRef<Path>,
        f: impl FnOnce(&mut Vec<u8>) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        // Linux parses as soon as things are written, so it needs to go all at once.
        let mut buf = Vec::new();
        f(&mut buf)?;

        let mut file = OpenOptions::new()
            .read(false)
            .write(true)
            .append(true)
            .truncate(false)
            .create(true)
            .open(path)?;
        file.write_all(&buf)
    }

    let set_groups = if set_groups { "allow" } else { "deny" };
    open("/proc/self/setgroups", |w| writeln!(w, "{}", set_groups))
        .with_context(|| "while writing set_groups")?;
    open("/proc/self/gid_map", |w| writeln!(w, "{} {} 1", gid, gid))
        .with_context(|| "while writing gid_map")?;
    open("/proc/self/uid_map", |w| writeln!(w, "{} {} 1", uid, uid))
        .with_context(|| "while writing uid_map")?;
    Ok(())
}

#[derive(Default, Debug, Clone)]
pub struct Mappings {
    gids: Vec<(u32, u32, u32)>,
    uids: Vec<(u32, u32, u32)>,
}

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidMapping {
    #[error("the parent range exceeds the maximum uid/gid range")]
    InvalidParentRange,
    #[error("the child range exceeds the maximum uid/gid range")]
    InvalidSubRange,
}

impl Mappings {
    pub fn push_uid_range(
        &mut self,
        namespace_uid: u32,
        parent_range: impl RangeBounds<u32>,
    ) -> Result<&mut Self, InvalidMapping> {
        self.uids.push(Self::validate(Self::to_triplet(
            namespace_uid,
            parent_range,
        ))?);
        Ok(self)
    }

    pub fn push_gid_range(
        &mut self,
        namespace_gid: u32,
        parent_range: impl RangeBounds<u32>,
    ) -> Result<&mut Self, InvalidMapping> {
        self.gids.push(Self::validate(Self::to_triplet(
            namespace_gid,
            parent_range,
        ))?);
        Ok(self)
    }

    pub(crate) fn apply(&mut self, pid: Option<Pid>) -> Result<()> {
        Self::exec("newuidmap", pid, &mut self.uids).with_context(|| "while mapping uids")?;
        Self::exec("newgidmap", pid, &mut self.gids).with_context(|| "while mapping gids")?;
        Ok(())
    }

    #[inline]
    fn exec(app: &str, pid: Option<Pid>, map: &mut Vec<(u32, u32, u32)>) -> std::io::Result<()> {
        if map.is_empty() {
            // We always write this so that someone else doesn't do it for us
            // This is the kernel default
            map.push((0, 0, u32::MAX));
        }

        let dir = nix::fcntl::open("/proc/self", OFlag::O_DIRECTORY, Mode::empty())
            .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
            .map_err(super::errno_to_stdio_err)?;

        let pid = pid.unwrap_or_else(nix::unistd::getpid);
        let mut args = Vec::new();
        args.push(format!("{}", pid.as_raw()));
        for item in map {
            args.push(item.0.to_string());
            args.push(item.1.to_string());
            args.push(item.2.to_string());
        }

        let proc = std::process::Command::new(app).args(args).output()?;
        drop(dir);
        if !proc.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&proc.stdout));
            eprintln!("{}", String::from_utf8_lossy(&proc.stderr));
            Err(std::io::ErrorKind::Unsupported.into())
        } else {
            Ok(())
        }
    }

    #[inline]
    fn validate(value: (u32, u32, u32)) -> Result<(u32, u32, u32), InvalidMapping> {
        if value.0.overflowing_add(value.2).1 {
            Err(InvalidMapping::InvalidSubRange)
        } else if value.1.overflowing_add(value.2).1 {
            Err(InvalidMapping::InvalidParentRange)
        } else {
            Ok(value)
        }
    }

    #[inline]
    fn to_triplet(ns: u32, parent_range: impl RangeBounds<u32>) -> (u32, u32, u32) {
        match (parent_range.start_bound(), parent_range.end_bound()) {
            (std::ops::Bound::Included(s), std::ops::Bound::Included(e)) => {
                (ns, *s, e.saturating_sub(*s))
            }
            (std::ops::Bound::Included(s), std::ops::Bound::Excluded(e)) => {
                (ns, *s, e.saturating_sub(*s).saturating_sub(1))
            }
            (std::ops::Bound::Included(s), std::ops::Bound::Unbounded) => {
                (ns, *s, u32::MAX - *s.max(&ns))
            }
            (std::ops::Bound::Excluded(s), std::ops::Bound::Included(e)) => (
                ns,
                s.saturating_add(1),
                e.saturating_sub(*s).saturating_sub(1),
            ),
            (std::ops::Bound::Excluded(s), std::ops::Bound::Excluded(e)) => (
                ns,
                s.saturating_add(1),
                e.saturating_sub(*s).saturating_sub(2),
            ),
            (std::ops::Bound::Excluded(s), std::ops::Bound::Unbounded) => (
                ns,
                s.saturating_add(1),
                u32::MAX.saturating_sub(s.max(&ns).saturating_add(1)),
            ),
            (std::ops::Bound::Unbounded, std::ops::Bound::Included(e)) => (ns, 0, *e),
            (std::ops::Bound::Unbounded, std::ops::Bound::Excluded(e)) => {
                (ns, 0, e.saturating_sub(1))
            }
            (std::ops::Bound::Unbounded, std::ops::Bound::Unbounded) => (ns, 0, u32::MAX - ns),
        }
    }
}
