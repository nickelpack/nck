use nix::unistd::Pid;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::{env, path::PathBuf};

// Wrap the uid/gid path function into a struct for dependency injection. This
// allows us to mock the id mapping logic in unit tests by using a different
// base path other than `/proc`.
#[derive(Debug, Clone)]
pub struct UserNamespaceIDMapper {
    base_path: PathBuf,
}

impl Default for UserNamespaceIDMapper {
    fn default() -> Self {
        Self {
            // By default, the `uid_map` and `gid_map` files are located in the
            // `/proc` directory. In the production code, we can use the
            // default.
            base_path: PathBuf::from("/proc"),
        }
    }
}

impl UserNamespaceIDMapper {
    // In production code, we can direct use the `new` function without the
    // need to worry about the default.
    pub fn new() -> Self {
        Default::default()
    }

    pub fn get_uid_path(&self, pid: &Pid) -> PathBuf {
        self.base_path.join(pid.to_string()).join("uid_map")
    }
    pub fn get_gid_path(&self, pid: &Pid) -> PathBuf {
        self.base_path.join(pid.to_string()).join("gid_map")
    }

    #[cfg(test)]
    pub fn ensure_uid_path(&self, pid: &Pid) -> std::result::Result<(), std::io::Error> {
        std::fs::create_dir_all(self.get_uid_path(pid).parent().unwrap())?;

        Ok(())
    }

    #[cfg(test)]
    pub fn ensure_gid_path(&self, pid: &Pid) -> std::result::Result<(), std::io::Error> {
        std::fs::create_dir_all(self.get_gid_path(pid).parent().unwrap())?;

        Ok(())
    }

    #[cfg(test)]
    // In test, we need to fake the base path to a temporary directory.
    pub fn new_test(path: PathBuf) -> Self {
        Self { base_path: path }
    }
}

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
    #[error("could not find PATH")]
    NoPathEnv,
    #[error("failed to execute newuidmap/newgidmap")]
    Execute(#[source] std::io::Error),
    #[error("at least one id mapping needs to be defined")]
    NoIDMapping,
    #[error("failed to write id mapping")]
    WriteIDMapping(#[source] std::io::Error),
}

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone, Default)]
pub struct UserNamespaceConfig {
    /// Location of the newuidmap binary
    pub newuidmap: Option<PathBuf>,
    /// Location of the newgidmap binary
    pub newgidmap: Option<PathBuf>,
    /// Mappings for user ids
    pub(crate) uid_mappings: Option<Vec<LinuxIdMapping>>,
    /// Mappings for group ids
    pub(crate) gid_mappings: Option<Vec<LinuxIdMapping>>,
    /// Path to the id mappings
    pub id_mapper: UserNamespaceIDMapper,
}

impl UserNamespaceConfig {
    pub fn new(mut user_ns_config: UserNamespaceConfig) -> Result<Self> {
        if let Some((uid_binary, gid_binary)) = lookup_map_binaries()? {
            user_ns_config.newuidmap = Some(uid_binary);
            user_ns_config.newgidmap = Some(gid_binary);
        }

        Ok(user_ns_config)
    }

    pub fn write_uid_mapping(&self, target_pid: Pid) -> Result<()> {
        tracing::debug!("write UID mapping for {:?}", target_pid);
        if let Some(uid_mappings) = self.uid_mappings.as_ref() {
            write_id_mapping(
                target_pid,
                self.id_mapper.get_uid_path(&target_pid).as_path(),
                uid_mappings,
                self.newuidmap.as_deref(),
            )?;
        }
        Ok(())
    }

    pub fn write_gid_mapping(&self, target_pid: Pid) -> Result<()> {
        tracing::debug!("write GID mapping for {:?}", target_pid);
        if let Some(gid_mappings) = self.gid_mappings.as_ref() {
            write_id_mapping(
                target_pid,
                self.id_mapper.get_gid_path(&target_pid).as_path(),
                gid_mappings,
                self.newgidmap.as_deref(),
            )?;
        }
        Ok(())
    }

    pub fn with_id_mapper(&mut self, mapper: UserNamespaceIDMapper) {
        self.id_mapper = mapper
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

fn is_id_mapped(id: u32, mappings: &[LinuxIdMapping]) -> bool {
    mappings
        .iter()
        .any(|m| id >= m.container_id() && id <= m.container_id() + m.size())
}

/// Looks up the location of the newuidmap and newgidmap binaries which
/// are required to write multiple user/group mappings
pub fn lookup_map_binaries() -> std::result::Result<Option<(PathBuf, PathBuf)>, MappingError> {
    let uidmap = lookup_map_binary("newuidmap")?;
    let gidmap = lookup_map_binary("newgidmap")?;

    match (uidmap, gidmap) {
        (Some(newuidmap), Some(newgidmap)) => Ok(Some((newuidmap, newgidmap))),
        _ => Err(MappingError::BinaryNotFound),
    }
}

fn lookup_map_binary(binary: &str) -> std::result::Result<Option<PathBuf>, MappingError> {
    let paths = env::var("PATH").map_err(|_| MappingError::NoPathEnv)?;
    Ok(paths
        .split_terminator(':')
        .map(|p| Path::new(p).join(binary))
        .find(|p| p.exists()))
}

fn write_id_mapping(
    pid: Pid,
    map_file: &Path,
    mappings: &[LinuxIdMapping],
    map_binary: Option<&Path>,
) -> std::result::Result<(), MappingError> {
    tracing::debug!("Write ID mapping: {:?}", mappings);

    match mappings.len() {
        0 => return Err(MappingError::NoIDMapping),
        1 => {
            let mapping = mappings
                .first()
                .and_then(|m| format!("{} {} {}", m.container_id(), m.host_id(), m.size()).into())
                .unwrap();
            std::fs::write(map_file, &mapping).map_err(|err| {
                tracing::error!(?err, ?map_file, ?mapping, "failed to write uid/gid mapping");
                MappingError::WriteIDMapping(err)
            })?;
        }
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

            Command::new(map_binary.unwrap())
                .arg(pid.to_string())
                .args(args)
                .output()
                .map_err(|err| {
                    tracing::error!(?err, ?map_binary, "failed to execute newuidmap/newgidmap");
                    MappingError::Execute(err)
                })?;
        }
    }

    Ok(())
}
