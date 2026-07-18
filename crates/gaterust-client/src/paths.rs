use std::path::PathBuf;

use crate::error::{ClientError, Result};

pub(crate) fn config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    explicit.map_or_else(default_config_path, Ok)
}

#[cfg(windows)]
fn default_config_path() -> Result<PathBuf> {
    environment_directory("APPDATA").map(|path| path.join("GateRust").join("client.toml"))
}

#[cfg(target_os = "macos")]
fn default_config_path() -> Result<PathBuf> {
    environment_directory("HOME")
        .map(|path| path.join("Library/Application Support/GateRust/client.toml"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn default_config_path() -> Result<PathBuf> {
    if let Ok(path) = environment_directory("XDG_CONFIG_HOME") {
        return Ok(path.join("gaterust/client.toml"));
    }
    environment_directory("HOME").map(|path| path.join(".config/gaterust/client.toml"))
}

fn environment_directory(name: &str) -> Result<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(ClientError::ConfigDirectoryUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_config_path_has_priority() {
        let path = PathBuf::from("custom/client.toml");
        assert_eq!(config_path(Some(path.clone())).expect("显式路径有效"), path);
    }
}
