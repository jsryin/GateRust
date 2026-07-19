use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("读取控制平面配置 {path} 失败: {source}")]
    ReadConfig { path: PathBuf, source: io::Error },
    #[error("解析控制平面配置 {path} 失败: {source}")]
    ParseConfig {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("控制平面配置无效: {0}")]
    InvalidConfig(String),
    #[error("{kind}不存在: {name}")]
    ResourceNotFound { kind: &'static str, name: String },
    #[error("读取运行配置失败: {0}")]
    ReadRuntimeConfig(String),
    #[error("写入运行配置失败: {0}")]
    WriteRuntimeConfig(String),
    #[error("启动配置文件监听失败: {0}")]
    Watch(#[from] notify::Error),
    #[error("绑定 Web UI 地址失败: {0}")]
    Bind(#[source] io::Error),
    #[error("Web UI 服务失败: {0}")]
    Serve(#[source] io::Error),
}

pub type Result<T> = std::result::Result<T, ControlError>;
