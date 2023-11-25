use std::{ffi::OsStr, fs::OpenOptions, io::Write, ops::RangeBounds, path::Path};

use nix::unistd::{setresgid, setresuid, Gid, Pid, Uid};
use thiserror::Error;

#[tracing::instrument(level = "trace", skip(supplementary), fields(supplementary = ?supplementary.as_ref()), err(Debug))]
pub fn set_id(uid: Uid, gid: Gid, supplementary: impl AsRef<[Gid]>) -> nix::Result<()> {
    let supplementary = supplementary.as_ref();

    if !supplementary.is_empty() {
        tracing::trace!("setting supplementary groups");
        nix::unistd::setgroups(supplementary)?;
    }
    tracing::trace!("setting gids");
    setresgid(gid, gid, gid)?;
    tracing::trace!("setting uids");
    setresuid(uid, uid, uid)?;

    Ok(())
}

#[tracing::instrument(level = "trace", err(Debug))]
pub fn map_direct(uid: Uid, gid: Gid, set_groups: bool) -> nix::Result<()> {
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
    tracing::trace!("writing /proc/self/setgroups");
    open("/proc/self/setgroups", |w| writeln!(w, "{}", set_groups))
        .map_err(super::std_error_to_nix)?;
    tracing::trace!("writing /proc/self/gid_map");
    open("/proc/self/gid_map", |w| writeln!(w, "{} {} 1", gid, gid))
        .map_err(super::std_error_to_nix)?;
    tracing::trace!("writing /proc/self/uid_map");
    open("/proc/self/uid_map", |w| writeln!(w, "{} {} 1", uid, uid))
        .map_err(super::std_error_to_nix)?;
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
    pub fn push_uid(&mut self, namespace: Uid, parent: Uid) -> &mut Self {
        self.uids.push((namespace.as_raw(), parent.as_raw(), 1));
        self
    }

    pub fn push_gid(&mut self, namespace: Gid, parent: Gid) -> &mut Self {
        self.gids.push((namespace.as_raw(), parent.as_raw(), 1));
        self
    }

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

    #[tracing::instrument(level = "trace", skip(self), err(Debug))]
    pub(crate) fn apply(&self, pid: Option<Pid>) -> nix::Result<()> {
        Self::exec("newuidmap", pid, &self.uids)?;
        Self::exec("newgidmap", pid, &self.gids)?;
        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(map), err(Debug))]
    fn exec(app: &str, pid: Option<Pid>, map: &Vec<(u32, u32, u32)>) -> nix::Result<()> {
        static DEFAULT_MAP: [(u32, u32, u32); 1] = [(0, 0, u32::MAX)];
        let map = if map.is_empty() {
            // We always write this so that someone else doesn't do it for us
            // This is the kernel default
            &DEFAULT_MAP[..]
        } else {
            &map[..]
        };

        let pid = pid.unwrap_or_else(nix::unistd::getpid);
        let mut args = Vec::new();
        args.push(pid.to_string());
        for item in map {
            args.push(item.0.to_string());
            args.push(item.1.to_string());
            args.push(item.2.to_string());
        }

        tracing::trace!(?map, "applying map");
        let proc = std::process::Command::new(app)
            .args(args)
            .output()
            .map_err(super::std_error_to_nix)?;

        if proc.status.success() {
            Ok(())
        } else {
            let stdout = unsafe { OsStr::from_encoded_bytes_unchecked(&proc.stdout) };
            let stderr = unsafe { OsStr::from_encoded_bytes_unchecked(&proc.stderr) };
            tracing::error!(?stdout, ?stderr, "process failed");
            Err(nix::Error::UnknownErrno)
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
