use std::{io, net::SocketAddr};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("无法确定当前用户的客户端配置目录")]
    ConfigDirectoryUnavailable,
    #[error(transparent)]
    Tunnel(#[from] gaterust_tunnel::TunnelError),
    #[error("绑定客户端本机 API 地址 {address} 失败: {source}")]
    Bind {
        address: SocketAddr,
        source: io::Error,
    },
    #[error("客户端本机 API 异常退出: {0}")]
    Serve(io::Error),
    #[error("监听退出信号失败: {0}")]
    Signal(io::Error),
    #[error("等待客户端后台任务失败: {0}")]
    Task(#[from] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, ClientError>;
