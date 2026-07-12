use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{ErrorKind, Write as _},
    path::{Path, PathBuf},
};

use rand::RngExt as _;

use crate::{Result, TunnelError};

const MAX_DEVICE_ID_BYTES: usize = 64;

pub(crate) struct DeviceIdentity {
    id: String,
    path: PathBuf,
}

impl DeviceIdentity {
    pub(crate) fn load(config_path: &Path) -> Result<Self> {
        let path = identity_path(config_path);
        match std::fs::read_to_string(&path) {
            Ok(id) => {
                let id = id.trim().to_owned();
                validate_device_id(&id)?;
                Ok(Self { id, path })
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let identity = Self {
                    id: base_device_id(),
                    path,
                };
                identity.persist()?;
                Ok(identity)
            }
            Err(source) => Err(TunnelError::ReadConfig { path, source }),
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.id
    }

    pub(crate) fn resolve_conflict(&mut self) -> Result<()> {
        let base = self
            .id
            .rsplit_once('-')
            .filter(|(_, suffix)| {
                suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
            .map_or(self.id.as_str(), |(base, _)| base);
        let suffix = rand::rng().random_range(0..1_000_000_u32);
        let max_base = MAX_DEVICE_ID_BYTES - 7;
        self.id = format!("{}-{suffix:06}", truncate_ascii(base, max_base));
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut random = [0_u8; 8];
        rand::rng().fill(&mut random);
        let temporary = parent.join(format!(
            ".device-id-{:016x}.tmp",
            u64::from_ne_bytes(random)
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(self.id.as_bytes())?;
            file.sync_all()?;
            #[cfg(windows)]
            if self.path.exists() {
                let mut file = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&self.path)?;
                file.write_all(self.id.as_bytes())?;
                file.sync_all()?;
                std::fs::remove_file(&temporary)?;
                return Ok(());
            }
            std::fs::rename(&temporary, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(Into::into)
    }
}

pub(crate) fn validate_device_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > MAX_DEVICE_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(TunnelError::InvalidConfig(
            "设备 ID 必须为 1..=64 个 ASCII 字母、数字、- 或 _".into(),
        ));
    }
    Ok(())
}

fn identity_path(config_path: &Path) -> PathBuf {
    let mut name = config_path
        .file_name()
        .map_or_else(|| OsString::from("client"), OsString::from);
    name.push(".device-id");
    config_path.with_file_name(name)
}

fn base_device_id() -> String {
    let system = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "mac",
        "linux" => "linux",
        other => other,
    };
    let raw_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(system_host_name)
        .unwrap_or_else(|| "device".into());
    let name = sanitize(&raw_name, MAX_DEVICE_ID_BYTES - system.len() - 1);
    format!("{system}-{name}")
}

fn system_host_name() -> Option<String> {
    #[cfg(unix)]
    {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn sanitize(value: &str, limit: usize) -> String {
    let mut result = String::with_capacity(value.len().min(limit));
    for byte in value.bytes().take(limit) {
        let byte = if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            byte
        } else {
            b'-'
        };
        if byte != b'-' || !result.ends_with('-') {
            result.push(char::from(byte));
        }
    }
    let result = result.trim_matches('-');
    if result.is_empty() {
        "device".into()
    } else {
        result.into()
    }
}

fn truncate_ascii(value: &str, limit: usize) -> &str {
    &value[..value.len().min(limit)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_device_name() {
        assert_eq!(sanitize("DESKTOP I2QCOV3", 64), "DESKTOP-I2QCOV3");
        assert_eq!(sanitize("  ", 64), "device");
    }

    #[test]
    fn persists_conflict_suffix() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let config = directory.path().join("client.toml");
        let mut identity = DeviceIdentity {
            id: "win-DESKTOP-I2QCOV3".into(),
            path: identity_path(&config),
        };
        identity.resolve_conflict().expect("生成冲突后缀");
        assert!(identity.id.starts_with("win-DESKTOP-I2QCOV3-"));
        assert_eq!(identity.id.len(), "win-DESKTOP-I2QCOV3-123456".len());
        assert_eq!(
            std::fs::read_to_string(identity_path(&config)).expect("读取设备 ID"),
            identity.id
        );
    }
}
