use std::{io, path::PathBuf};

/// 代理模块统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("读取配置 {path} 失败: {source}")]
    ReadConfig { path: PathBuf, source: io::Error },
    #[error("解析配置 {path} 失败: {source}")]
    ParseConfig {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("配置无效: {0}")]
    InvalidConfig(String),
    #[error("I/O 错误: {0}")]
    Io(#[from] io::Error),
    #[error("文件监听失败: {0}")]
    Notify(#[from] notify::Error),
    #[error("HTTP 错误: {0}")]
    Http(#[from] http::Error),
    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TLS 配置失败: {0}")]
    Tls(String),
    #[error("ACME 操作失败: {0}")]
    Acme(String),
    #[error("Cloudflare API 操作失败: {0}")]
    Cloudflare(String),
}

pub type Result<T, E = ProxyError> = std::result::Result<T, E>;
