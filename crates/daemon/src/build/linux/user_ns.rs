#![tarpaulin::skip]

use nck_io::PrintableBuffer;
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use which::which;

#[derive(Debug, thiserror::Error)]
pub enum UserNamespaceError {
    #[error("user namespace definition is invalid")]
    NoUserNamespace,
    #[error("failed to read unprivileged userns clone")]
    ReadUnprivilegedUsernsClone(#[source] std::io::Error),
    #[error("failed to parse unprivileged userns clone")]
    ParseUnprivilegedUsernsClone(#[source] std::num::ParseIntError),
    #[error("unknown userns clone value")]
    UnknownUnprivilegedUsernsClone(u8),
    #[error(transparent)]
    IDMapping(#[from] MappingError),
}

type Result<T> = std::result::Result<T, UserNamespaceError>;

#[derive(Debug, thiserror::Error)]
pub enum MappingError {
    #[error("newuidmap/newgidmap binaries could not be found in path")]
    BinaryNotFound,
    #[error("failed to execute newuidmap/newgidmap")]
    Execute(#[source] std::io::Error),
    #[error("newuidmap/newgidmap exited with status {exit_code:?}:\n{stdout}\n{stderr}")]
    FailureStatus {
        exit_code: ExitStatus,
        stdout: String,
        stderr: String,
    },
    #[error("at least one id mapping needs to be defined")]
    NoIDMapping,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinuxIdMapping {
    container_id: u32,
    host_id: u32,
    size: u32,
}

impl LinuxIdMapping {
    pub fn new(container_id: u32, host_id: u32, size: u32) -> Self {
        Self {
            container_id,
            host_id,
            size,
        }
    }

    pub fn container_id(&self) -> u32 {
        self.container_id
    }
    pub fn host_id(&self) -> u32 {
        self.host_id
    }
    pub fn size(&self) -> u32 {
        self.size
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserNamespaceConfig {
    /// Location of the newuidmap binary
    newuidmap: PathBuf,
    /// Location of the newgidmap binary
    newgidmap: PathBuf,
    /// Mappings for user ids
    uid_mappings: Vec<LinuxIdMapping>,
    /// Mappings for group ids
    gid_mappings: Vec<LinuxIdMapping>,
}

impl UserNamespaceConfig {
    pub fn new() -> Result<Self> {
        let (newuidmap, newgidmap) = lookup_map_binaries()?;
        Ok(Self {
            newuidmap,
            newgidmap,
            uid_mappings: Vec::new(),
            gid_mappings: Vec::new(),
        })
    }

    pub fn uid_mappings_mut(&mut self) -> &mut Vec<LinuxIdMapping> {
        &mut self.uid_mappings
    }

    pub fn gid_mappings_mut(&mut self) -> &mut Vec<LinuxIdMapping> {
        &mut self.gid_mappings
    }

    pub fn write_mappings(&self, target_pid: Pid) -> Result<()> {
        self.write_gid_mapping(target_pid)?;
        self.write_uid_mapping(target_pid)
    }

    pub fn write_uid_mapping(&self, target_pid: Pid) -> Result<()> {
        write_id_mapping(target_pid, &self.uid_mappings, self.newuidmap.as_path())?;
        Ok(())
    }

    pub fn write_gid_mapping(&self, target_pid: Pid) -> Result<()> {
        write_id_mapping(target_pid, &self.gid_mappings, self.newgidmap.as_path())?;
        Ok(())
    }
}

pub fn unprivileged_user_ns_enabled() -> Result<bool> {
    let user_ns_sysctl = Path::new("/proc/sys/kernel/unprivileged_userns_clone");
    if !user_ns_sysctl.exists() {
        return Ok(true);
    }

    let content = fs::read_to_string(user_ns_sysctl)
        .map_err(UserNamespaceError::ReadUnprivilegedUsernsClone)?;

    match content
        .trim()
        .parse::<u8>()
        .map_err(UserNamespaceError::ParseUnprivilegedUsernsClone)?
    {
        0 => Ok(false),
        1 => Ok(true),
        v => Err(UserNamespaceError::UnknownUnprivilegedUsernsClone(v)),
    }
}

/// Looks up the location of the newuidmap and newgidmap binaries which
/// are required to write multiple user/group mappings
pub fn lookup_map_binaries() -> std::result::Result<(PathBuf, PathBuf), MappingError> {
    let uidmap = which("newuidmap").ok();
    let gidmap = which("newgidmap").ok();

    match (uidmap, gidmap) {
        (Some(newuidmap), Some(newgidmap)) => Ok((newuidmap, newgidmap)),
        _ => Err(MappingError::BinaryNotFound),
    }
}

fn write_id_mapping(
    pid: Pid,
    mappings: &[LinuxIdMapping],
    map_binary: &Path,
) -> std::result::Result<(), MappingError> {
    tracing::trace!(?pid, ?mappings, "write ID mapping");

    match mappings.len() {
        0 => return Err(MappingError::NoIDMapping),
        _ => {
            let args: Vec<String> = mappings
                .iter()
                .flat_map(|m| {
                    [
                        m.container_id().to_string(),
                        m.host_id().to_string(),
                        m.size().to_string(),
                    ]
                })
                .collect();

            tracing::trace!(?map_binary, ?args, "calling ID binary");

            let status = Command::new(map_binary)
                .arg(pid.to_string())
                .args(args)
                .output()
                .map_err(|err| {
                    tracing::error!(?err, ?map_binary, "failed to execute newuidmap/newgidmap");
                    MappingError::Execute(err)
                })?;

            if !status.status.success() {
                let exit_code = status.status;
                let stdout = PrintableBuffer(&status.stdout).to_string();
                let stderr = PrintableBuffer(&status.stderr).to_string();
                return Err(MappingError::FailureStatus {
                    exit_code,
                    stdout,
                    stderr,
                });
            }
        }
    }

    Ok(())
}
