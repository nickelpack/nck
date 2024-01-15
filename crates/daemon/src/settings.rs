use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct LinuxSubIdSetting {
    pub min: u32,
    pub max: u32,
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
pub struct StoreSettings {
    pub path: PathBuf,
    pub temp: PathBuf,
}

impl Default for StoreSettings {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/nck/store"),
            temp: std::env::temp_dir().join("nck"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonSettings {
    #[cfg(target_os = "linux")]
    #[serde(flatten)]
    pub linux: LinuxSandboxSettings,

    #[serde(default)]
    pub tcp: TcpSettings,

    #[serde(default = "default_socket_path")]
    pub socket_path: PathBuf,
}

fn default_socket_path() -> PathBuf {
    "/var/nck/daemon.sock".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub store: StoreSettings,

    pub daemon: DaemonSettings,
}
