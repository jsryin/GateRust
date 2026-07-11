use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{ControlError, Result};

const DEFAULT_TOKEN_TTL_SECONDS: u64 = 3_600;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlConfig {
    pub web: WebConfig,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    pub bind: SocketAddr,
    pub static_dir: Option<PathBuf>,
    pub admin_username: String,
    pub admin_password_hash: String,
    pub jwt_secret: String,
    #[serde(default = "default_token_ttl_seconds")]
    pub token_ttl_seconds: u64,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

impl ControlConfig {
    /// 加载并验证控制平面配置。
    ///
    /// # Errors
    ///
    /// 文件不可读、TOML 无效或认证参数不安全时返回错误。
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|source| ControlError::ReadConfig {
            path: path.to_owned(),
            source,
        })?;
        let mut config: Self =
            toml::from_str(&content).map_err(|source| ControlError::ParseConfig {
                path: path.to_owned(),
                source,
            })?;
        if let Some(static_dir) = &mut config.web.static_dir
            && static_dir.is_relative()
        {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            *static_dir = parent.join(&*static_dir);
        }
        config.web.validate()?;
        Ok(config)
    }
}

impl WebConfig {
    fn validate(&self) -> Result<()> {
        if self.admin_username.is_empty() || self.admin_username.len() > 64 {
            return Err(ControlError::InvalidConfig(
                "管理员用户名长度必须为 1..=64".into(),
            ));
        }
        if self.jwt_secret.len() < 32 {
            return Err(ControlError::InvalidConfig(
                "jwt_secret 至少需要 32 字节".into(),
            ));
        }
        if !(300..=86_400).contains(&self.token_ttl_seconds) {
            return Err(ControlError::InvalidConfig(
                "token_ttl_seconds 必须为 300..=86400".into(),
            ));
        }
        for origin in &self.allowed_origins {
            let valid = origin.starts_with("http://") || origin.starts_with("https://");
            if !valid || origin.parse::<http::HeaderValue>().is_err() {
                return Err(ControlError::InvalidConfig(format!(
                    "allowed_origins 包含无效来源: {origin}"
                )));
            }
        }
        Ok(())
    }
}

const fn default_token_ttl_seconds() -> u64 {
    DEFAULT_TOKEN_TTL_SECONDS
}
