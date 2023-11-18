use anyhow::{Context, Result};
use nix::unistd::{Gid, Uid};

pub fn set_id(uid: Uid, gid: Gid, supplementary: impl AsRef<[Gid]>) -> Result<()> {
    let supplementary = supplementary.as_ref();

    if supplementary.len() != 0 {
        setgroups(supplementary)
            .with_context(|| format!("while setting all IDs of the current process"))?;
    }
    setresgid(gid, gid, gid)
        .with_context(|| format!("while setting all IDs of the current process"))?;
    setresuid(uid, uid, uid)
        .with_context(|| format!("while setting all IDs of the current process"))?;

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
