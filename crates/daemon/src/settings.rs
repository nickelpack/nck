use std::{path::PathBuf, sync::LazyLock};

use serde::Deserialize;

pub static STORE_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/var/nck/store"));
pub const SOCKET_PATH: &str = "/var/nck/nck-daemon.socket";
pub static TMP_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| std::env::temp_dir().join("nck"));

#[derive(Debug, Clone, Deserialize)]
pub struct LinuxSubIdSetting {
    min: u32,
    max: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinuxSandboxSettings {
    pub sub_uid: LinuxSubIdSetting,
    pub sub_gid: LinuxSubIdSetting,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TcpSettings {
    pub bind: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    #[cfg(target_os = "linux")]
    pub linux: LinuxSandboxSettings,

    #[serde(default = "default_tcp_settings")]
    pub tcp: TcpSettings,
}

fn default_tcp_settings() -> TcpSettings {
    TcpSettings { bind: Vec::new() }
}

impl From<Settings> for nck_sandbox::Settings {
    fn from(val: Settings) -> Self {
        nck_sandbox::Settings {
            tmp_directory: TMP_DIRECTORY.clone(),
            store_directory: STORE_DIRECTORY.clone(),

            #[cfg(target_os = "linux")]
            linux: val.linux.into(),
        }
    }
}

impl From<LinuxSandboxSettings> for nck_sandbox::linux::Settings {
    fn from(val: LinuxSandboxSettings) -> Self {
        nck_sandbox::linux::Settings {
            uid_min: val.sub_uid.min,
            uid_max: val.sub_uid.max,
            gid_min: val.sub_gid.min,
            gid_max: val.sub_gid.max,
        }
    }
}
