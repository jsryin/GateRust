use tokio::task::JoinError;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("无法确定当前用户的客户端配置目录")]
    ConfigDirectoryUnavailable,
    #[error("客户端运行时必须在 Tokio 上下文中启动")]
    RuntimeUnavailable,
    #[error(transparent)]
    Tunnel(#[from] gaterust_tunnel::TunnelError),
    #[error("等待客户端运行时任务失败: {0}")]
    Task(#[from] JoinError),
}

pub type Result<T> = std::result::Result<T, ClientError>;
