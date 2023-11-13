use anyhow::{Context, Result};
use derive_more::Display;
use nix::unistd::{Gid, Uid};

#[derive(Debug, Display)]
#[display(fmt = "failed to set id")]
pub struct SetIdFailure;

pub fn set_id(uid: Uid, gid: Gid, supplementary: impl AsRef<[Gid]>) -> Result<()> {
    let supplementary = supplementary.as_ref();

    if supplementary.len() != 0 {
        setgroups(supplementary).with_context(|| SetIdFailure)?;
    }
    setresgid(gid, gid, gid).with_context(|| SetIdFailure)?;
    setresuid(uid, uid, uid).with_context(|| SetIdFailure)?;

    Ok(())
}

#[derive(Debug, Display)]
#[display(fmt = "failed to set the gid to {} {} {}", _0, _1, _2)]
pub struct SetResGidFailure(Gid, Gid, Gid);

pub fn setresgid(r: Gid, e: Gid, s: Gid) -> Result<()> {
    nix::unistd::setresgid(r, e, s).with_context(|| SetResGidFailure(r, e, s))
}

#[derive(Debug, Display)]
#[display(fmt = "failed to set the uid to {} {} {}", _0, _1, _2)]
pub struct SetResUidFailure(Uid, Uid, Uid);

pub fn setresuid(r: Uid, e: Uid, s: Uid) -> Result<()> {
    nix::unistd::setresuid(r, e, s).with_context(|| SetResUidFailure(r, e, s))
}

#[derive(Debug)]
pub struct SetGroupsFailure(Vec<Gid>);

impl std::fmt::Display for SetGroupsFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("failed to set the groups to [ ")?;
        for i in &self.0 {
            f.write_fmt(format_args!("{}, ", i))?;
        }
        f.write_str("]")
    }
}

pub fn setgroups(g: impl AsRef<[Gid]>) -> Result<()> {
    let g = g.as_ref();
    nix::unistd::setgroups(g).with_context(|| SetGroupsFailure(g.iter().cloned().collect()))
}
