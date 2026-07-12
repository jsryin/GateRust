use std::{
    collections::HashSet,
    net::SocketAddr,
    num::NonZeroU64,
    path::{Path, PathBuf},
};

use rand::distr::{Alphanumeric, SampleString as _};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

use crate::{Result, TunnelError};

const DEFAULT_MAX_CONNECTIONS: usize = 1_024;
const DEFAULT_MAX_UDP_SESSIONS: usize = 1_024;
const DEFAULT_UDP_IDLE_SECONDS: u64 = 60;
pub(crate) const MIN_GROUP_KEY_LENGTH: usize = 32;
pub(crate) const MAX_GROUP_KEY_LENGTH: usize = 124;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelKind {
    Tcp,
    Udp,
    Socks5,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub quic: ServerQuicConfig,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
    #[serde(default)]
    pub tunnels: Vec<ServerTunnelConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerQuicConfig {
    pub bind: SocketAddr,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupConfig {
    pub name: String,
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    pub key: String,
    pub server: ClientServerConfig,
    #[serde(default)]
    pub services: Vec<ClientServiceConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientServerConfig {
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_certificate: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientServiceConfig {
    pub name: String,
    pub kind: TunnelKind,
    pub target: Option<String>,
}

#[derive(Clone)]
pub(crate) struct GroupSecret(Vec<u8>);

impl GroupSecret {
    pub(crate) fn new(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
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

    pub(crate) fn credentials(&self) -> Vec<(String, GroupSecret)> {
        self.groups
            .iter()
            .map(|group| (group.name.clone(), GroupSecret::new(&group.key)))
            .collect()
    }

    /// 验证服务端分组和隧道配置。
    ///
    /// # Errors
    ///
    /// 名称、密钥、监听地址或数量不满足约束时返回错误。
    pub fn validate(&self) -> Result<()> {
        if self.groups.len() > 256 || self.tunnels.len() > 1_024 {
            return Err(TunnelError::InvalidConfig(
                "分组不能超过 256 个，隧道不能超过 1024 个".into(),
            ));
        }
        let mut groups = HashSet::new();
        let mut keys = HashSet::new();
        for group in &self.groups {
            validate_name("分组", &group.name)?;
            if !groups.insert(group.name.as_str()) {
                return Err(TunnelError::InvalidConfig(format!(
                    "分组名称重复: {}",
                    group.name
                )));
            }
            validate_group_key(&group.key)?;
            if !keys.insert(group.key.as_str()) {
                return Err(TunnelError::InvalidConfig(
                    "不同分组不能使用相同密钥".into(),
                ));
            }
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
        if let Some(certificate) = &mut config.server.ca_certificate {
            resolve_path(path, certificate);
        }
        config.validate()?;
        Ok(config)
    }

    /// 验证客户端配置中的认证信息和服务声明。
    ///
    /// # Errors
    ///
    /// 名称、密钥、服务目标或数量不满足约束时返回错误。
    pub fn validate(&self) -> Result<()> {
        validate_group_key(&self.key)?;
        if self.server.address.is_empty() {
            return Err(TunnelError::InvalidConfig("QUIC 服务器地址不能为空".into()));
        }
        self.server_name()?;

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

    pub(crate) fn server_name(&self) -> Result<&str> {
        if let Some(name) = self.server.name.as_deref() {
            if name.is_empty() {
                return Err(TunnelError::InvalidConfig("TLS 服务器名称不能为空".into()));
            }
            return Ok(name);
        }
        address_host(&self.server.address)
            .ok_or_else(|| TunnelError::InvalidConfig("无法从服务器地址推导 TLS 服务器名称".into()))
    }
}

/// 生成满足配置约束的随机分组密钥。
#[must_use]
pub fn generate_group_key() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), MIN_GROUP_KEY_LENGTH)
}

pub(crate) fn validate_group_key(value: &str) -> Result<()> {
    let length = value.chars().take(MAX_GROUP_KEY_LENGTH + 1).count();
    if !(MIN_GROUP_KEY_LENGTH..=MAX_GROUP_KEY_LENGTH).contains(&length) {
        return Err(TunnelError::InvalidConfig(format!(
            "分组密钥长度必须为 {MIN_GROUP_KEY_LENGTH}..={MAX_GROUP_KEY_LENGTH} 个字符"
        )));
    }
    Ok(())
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

fn address_host(address: &str) -> Option<&str> {
    if let Some(bracketed) = address.strip_prefix('[') {
        return bracketed.split_once(']').map(|(host, _)| host);
    }
    address.rsplit_once(':').and_then(|(host, port)| {
        (!host.is_empty() && !port.is_empty() && !host.contains(':')).then_some(host)
    })
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
    fn validates_group_key_length_boundaries() {
        assert!(validate_group_key(&"a".repeat(31)).is_err());
        assert!(validate_group_key(&"a".repeat(32)).is_ok());
        assert!(validate_group_key(&"密".repeat(32)).is_ok());
        assert!(validate_group_key(&"a".repeat(124)).is_ok());
        assert!(validate_group_key(&"a".repeat(125)).is_err());
    }

    #[test]
    fn generates_valid_group_key() {
        let key = generate_group_key();
        assert_eq!(key.len(), MIN_GROUP_KEY_LENGTH);
        assert!(key.bytes().all(|byte| byte.is_ascii_alphanumeric()));
    }

    #[test]
    fn rejects_duplicate_group_keys() {
        let config = ServerConfig {
            quic: ServerQuicConfig {
                bind: "127.0.0.1:2333".parse().expect("测试地址有效"),
                certificate: "server.pem".into(),
                private_key: "server-key.pem".into(),
            },
            groups: vec![
                GroupConfig {
                    name: "first".into(),
                    key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
                },
                GroupConfig {
                    name: "second".into(),
                    key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
                },
            ],
            tunnels: Vec::new(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_socks_target() {
        let config = ClientConfig {
            key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            server: ClientServerConfig {
                address: "127.0.0.1:4433".into(),
                name: Some("localhost".into()),
                ca_certificate: Some("ca.pem".into()),
            },
            services: vec![ClientServiceConfig {
                name: "proxy".into(),
                kind: TunnelKind::Socks5,
                target: Some("127.0.0.1:1080".into()),
            }],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn derives_server_name_from_address() {
        let mut config = ClientConfig {
            key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            server: ClientServerConfig {
                address: "tunnel.example.com:2333".into(),
                name: None,
                ca_certificate: None,
            },
            services: Vec::new(),
        };
        assert_eq!(
            config.server_name().expect("应推导域名"),
            "tunnel.example.com"
        );
        config.server.address = "[::1]:2333".into();
        assert_eq!(config.server_name().expect("应推导 IPv6"), "::1");
    }
}
