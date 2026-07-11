use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    num::NonZeroU64,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

use crate::{Result, TunnelError};

const DEFAULT_MAX_CONNECTIONS: usize = 1_024;
const DEFAULT_MAX_UDP_SESSIONS: usize = 1_024;
const DEFAULT_UDP_IDLE_SECONDS: u64 = 60;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelKind {
    Tcp,
    Udp,
    Socks5,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub quic: ServerQuicConfig,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
    #[serde(default)]
    pub tunnels: Vec<ServerTunnelConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServerQuicConfig {
    pub bind: SocketAddr,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupConfig {
    pub name: String,
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServerTunnelConfig {
    pub name: String,
    pub group: String,
    pub kind: TunnelKind,
    pub bind: SocketAddr,
    pub limit_bps: Option<NonZeroU64>,
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    #[serde(default = "default_max_udp_sessions")]
    pub max_udp_sessions: usize,
    #[serde(default = "default_udp_idle_seconds")]
    pub udp_idle_seconds: u64,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    pub server: ClientServerConfig,
    pub group: ClientGroupConfig,
    #[serde(default)]
    pub services: Vec<ClientServiceConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClientServerConfig {
    pub address: String,
    pub name: String,
    pub ca_certificate: PathBuf,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientGroupConfig {
    pub name: String,
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientServiceConfig {
    pub name: String,
    pub kind: TunnelKind,
    pub target: Option<String>,
}

#[derive(Clone)]
pub(crate) struct GroupSecret([u8; 32]);

impl GroupSecret {
    pub(crate) fn decode(value: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| TunnelError::InvalidConfig("分组密钥必须是 URL-safe Base64".into()))?;
        let value: [u8; 32] = bytes
            .try_into()
            .map_err(|_| TunnelError::InvalidConfig("分组密钥解码后必须恰好为 32 字节".into()))?;
        Ok(Self(value))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for GroupSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl ServerConfig {
    /// 读取并验证服务端配置，相对路径以配置文件所在目录为基准。
    ///
    /// # Errors
    ///
    /// 文件不可读、TOML 格式错误或字段不满足约束时返回错误。
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|source| TunnelError::ReadConfig {
            path: path.to_owned(),
            source,
        })?;
        let mut config: Self =
            toml::from_str(&content).map_err(|source| TunnelError::ParseConfig {
                path: path.to_owned(),
                source,
            })?;
        resolve_path(path, &mut config.quic.certificate);
        resolve_path(path, &mut config.quic.private_key);
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn secrets(&self) -> Result<HashMap<String, GroupSecret>> {
        self.groups
            .iter()
            .map(|group| Ok((group.name.clone(), GroupSecret::decode(&group.key)?)))
            .collect()
    }

    fn validate(&self) -> Result<()> {
        if self.groups.len() > 256 || self.tunnels.len() > 1_024 {
            return Err(TunnelError::InvalidConfig(
                "分组不能超过 256 个，隧道不能超过 1024 个".into(),
            ));
        }
        let mut groups = HashSet::new();
        for group in &self.groups {
            validate_name("分组", &group.name)?;
            if !groups.insert(group.name.as_str()) {
                return Err(TunnelError::InvalidConfig(format!(
                    "分组名称重复: {}",
                    group.name
                )));
            }
            GroupSecret::decode(&group.key)?;
        }

        let mut names = HashSet::new();
        let mut stream_binds = HashSet::new();
        let mut udp_binds = HashSet::new();
        for tunnel in &self.tunnels {
            validate_name("隧道", &tunnel.name)?;
            if !names.insert(tunnel.name.as_str()) {
                return Err(TunnelError::InvalidConfig(format!(
                    "隧道名称重复: {}",
                    tunnel.name
                )));
            }
            if !groups.contains(tunnel.group.as_str()) {
                return Err(TunnelError::InvalidConfig(format!(
                    "隧道 {} 引用了不存在的分组 {}",
                    tunnel.name, tunnel.group
                )));
            }
            if tunnel.max_connections == 0 {
                return Err(TunnelError::InvalidConfig(format!(
                    "隧道 {} 的 max_connections 必须大于 0",
                    tunnel.name
                )));
            }
            if tunnel.max_udp_sessions == 0 || tunnel.udp_idle_seconds == 0 {
                return Err(TunnelError::InvalidConfig(format!(
                    "隧道 {} 的 UDP 会话限制和空闲时间必须大于 0",
                    tunnel.name
                )));
            }
            let inserted = match tunnel.kind {
                TunnelKind::Udp => udp_binds.insert(tunnel.bind),
                TunnelKind::Tcp | TunnelKind::Socks5 => stream_binds.insert(tunnel.bind),
            };
            if !inserted {
                return Err(TunnelError::InvalidConfig(format!(
                    "监听地址重复: {}",
                    tunnel.bind
                )));
            }
        }
        Ok(())
    }
}

impl ClientConfig {
    /// 读取并验证客户端配置，相对路径以配置文件所在目录为基准。
    ///
    /// # Errors
    ///
    /// 文件不可读、TOML 格式错误或字段不满足约束时返回错误。
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|source| TunnelError::ReadConfig {
            path: path.to_owned(),
            source,
        })?;
        let mut config: Self =
            toml::from_str(&content).map_err(|source| TunnelError::ParseConfig {
                path: path.to_owned(),
                source,
            })?;
        resolve_path(path, &mut config.server.ca_certificate);
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn secret(&self) -> Result<GroupSecret> {
        GroupSecret::decode(&self.group.key)
    }

    fn validate(&self) -> Result<()> {
        validate_name("分组", &self.group.name)?;
        GroupSecret::decode(&self.group.key)?;
        if self.server.name.is_empty() {
            return Err(TunnelError::InvalidConfig("TLS 服务器名称不能为空".into()));
        }
        if self.server.address.is_empty() {
            return Err(TunnelError::InvalidConfig("QUIC 服务器地址不能为空".into()));
        }

        if self.services.len() > 256 {
            return Err(TunnelError::InvalidConfig(
                "单个客户端最多声明 256 个服务".into(),
            ));
        }
        let mut names = HashSet::new();
        for service in &self.services {
            validate_name("服务", &service.name)?;
            if !names.insert(service.name.as_str()) {
                return Err(TunnelError::InvalidConfig(format!(
                    "服务名称重复: {}",
                    service.name
                )));
            }
            match (&service.kind, &service.target) {
                (TunnelKind::Tcp | TunnelKind::Udp, Some(target)) if !target.is_empty() => {}
                (TunnelKind::Socks5, None) => {}
                (TunnelKind::Socks5, Some(_)) => {
                    return Err(TunnelError::InvalidConfig(format!(
                        "SOCKS5 服务 {} 不应配置固定 target",
                        service.name
                    )));
                }
                _ => {
                    return Err(TunnelError::InvalidConfig(format!(
                        "TCP/UDP 服务 {} 必须配置 target",
                        service.name
                    )));
                }
            }
        }
        Ok(())
    }
}

fn validate_name(kind: &str, name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(TunnelError::InvalidConfig(format!(
            "{kind}名称长度必须为 1..=64"
        )));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(TunnelError::InvalidConfig(format!(
            "{kind}名称只能包含 ASCII 字母、数字、- 和 _"
        )));
    }
    Ok(())
}

fn resolve_path(config_path: &Path, value: &mut PathBuf) {
    if value.is_relative() {
        let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
        *value = parent.join(&*value);
    }
}

const fn default_max_connections() -> usize {
    DEFAULT_MAX_CONNECTIONS
}

const fn default_max_udp_sessions() -> usize {
    DEFAULT_MAX_UDP_SESSIONS
}

const fn default_udp_idle_seconds() -> u64 {
    DEFAULT_UDP_IDLE_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_socks_target() {
        let config = ClientConfig {
            server: ClientServerConfig {
                address: "127.0.0.1:4433".into(),
                name: "localhost".into(),
                ca_certificate: "ca.pem".into(),
            },
            group: ClientGroupConfig {
                name: "main".into(),
                key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            },
            services: vec![ClientServiceConfig {
                name: "proxy".into(),
                kind: TunnelKind::Socks5,
                target: Some("127.0.0.1:1080".into()),
            }],
        };
        assert!(config.validate().is_err());
    }
}
