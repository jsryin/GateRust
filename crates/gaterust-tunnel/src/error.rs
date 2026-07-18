use std::{io, net::AddrParseError, path::PathBuf};

/// 隧道模块的统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("读取配置 {path} 失败: {source}")]
    ReadConfig { path: PathBuf, source: io::Error },
    #[error("解析配置 {path} 失败: {source}")]
    ParseConfig {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("序列化配置 {path} 失败: {source}")]
    SerializeConfig {
        path: PathBuf,
        source: toml::ser::Error,
    },
    #[error("写入配置 {path} 失败: {source}")]
    WriteConfig { path: PathBuf, source: io::Error },
    #[error("配置无效: {0}")]
    InvalidConfig(String),
    #[error("地址无效: {0}")]
    InvalidAddress(#[from] AddrParseError),
    #[error("I/O 错误: {0}")]
    Io(#[from] io::Error),
    #[error("QUIC 连接失败: {0}")]
    QuinnConnection(#[from] quinn::ConnectionError),
    #[error("QUIC 连接建立失败: {0}")]
    QuinnConnect(#[from] quinn::ConnectError),
    #[error("QUIC 写入失败: {0}")]
    QuinnWrite(#[from] quinn::WriteError),
    #[error("QUIC 读取失败: {0}")]
    QuinnRead(#[from] quinn::ReadExactError),
    #[error("TLS 配置失败: {0}")]
    Tls(String),
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("操作超时: {0}")]
    Timeout(&'static str),
    #[error("文件监听失败: {0}")]
    Notify(#[from] notify::Error),
}

pub type Result<T, E = TunnelError> = std::result::Result<T, E>;
