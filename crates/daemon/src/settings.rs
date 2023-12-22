use std::{path::PathBuf, sync::LazyLock};

use serde::Deserialize;

pub static ROOT_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/var/nck"));
pub static STORE_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| ROOT_DIRECTORY.join("store"));
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

#[derive(Debug, Clone, Deserialize, Default)]
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
    TcpSettings::default()
}
