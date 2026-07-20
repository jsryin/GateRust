//! `GateRust` 客户端应用运行时。

mod error;
mod paths;

use std::{path::PathBuf, sync::Arc};

pub use error::{ClientError, Result};
use gaterust_tunnel::{ClientConfig, ClientStatus, run_client_with_status};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

/// 可嵌入桌面壳层的客户端运行时。
pub struct ClientRuntime {
    config_path: Arc<PathBuf>,
    status: watch::Receiver<ClientStatus>,
    shutdown: CancellationToken,
    task: Mutex<Option<JoinHandle<gaterust_tunnel::Result<()>>>>,
}

impl ClientRuntime {
    /// 初始化配置并启动隧道后台任务。
    ///
    /// # Errors
    ///
    /// 无法进入 `Tokio` 运行时、确定配置目录或创建初始配置时返回错误。
    pub fn start(explicit_config_path: Option<PathBuf>) -> Result<Self> {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| ClientError::RuntimeUnavailable)?;
        let config_path = prepare_config_path(explicit_config_path)?;

        let config_path = Arc::new(config_path);
        let shutdown = CancellationToken::new();
        let (status_sender, status) = watch::channel(ClientStatus::Starting);
        let task_status = status_sender.clone();
        let task_path = Arc::clone(&config_path);
        let task_shutdown = shutdown.clone();
        let task = runtime.spawn(async move {
            let result =
                run_client_with_status(task_path.as_ref(), task_shutdown, status_sender).await;
            if let Err(error) = &result {
                task_status.send_replace(ClientStatus::Stopped {
                    reason: Some(error.to_string()),
                });
            }
            result
        });

        Ok(Self {
            config_path,
            status,
            shutdown,
            task: Mutex::new(Some(task)),
        })
    }

    /// 返回当前配置文件路径。
    #[must_use]
    pub fn config_path(&self) -> &std::path::Path {
        self.config_path.as_ref()
    }

    /// 读取当前客户端配置。
    ///
    /// # Errors
    ///
    /// 后台任务无法调度或配置文件不可读时返回错误。
    pub async fn config(&self) -> Result<ClientConfig> {
        let path = Arc::clone(&self.config_path);
        tokio::task::spawn_blocking(move || ClientConfig::read(path.as_ref()))
            .await?
            .map_err(ClientError::from)
    }

    /// 校验并保存客户端配置。
    ///
    /// # Errors
    ///
    /// 后台任务无法调度、配置无效或文件无法写入时返回错误。
    pub async fn save_config(&self, config: ClientConfig) -> Result<ClientConfig> {
        let path = Arc::clone(&self.config_path);
        tokio::task::spawn_blocking(move || {
            config.save(path.as_ref())?;
            Ok::<_, gaterust_tunnel::TunnelError>(config)
        })
        .await?
        .map_err(ClientError::from)
    }

    /// 返回最近一次连接状态。
    #[must_use]
    pub fn status(&self) -> ClientStatus {
        self.status.borrow().clone()
    }

    /// 取消并等待后台隧道任务退出；重复调用是安全的。
    ///
    /// # Errors
    ///
    /// 后台任务异常退出或隧道清理失败时返回错误。
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown.cancel();
        let task = self.task.lock().await.take();
        if let Some(task) = task {
            task.await??;
        }
        Ok(())
    }
}

/// 解析并初始化客户端配置路径。
///
/// # Errors
///
/// 无法确定配置目录或无法创建初始配置时返回错误。
pub fn prepare_config_path(explicit_config_path: Option<PathBuf>) -> Result<PathBuf> {
    let path = paths::config_path(explicit_config_path)?;
    let created = ClientConfig::ensure_exists(&path)?;
    if created {
        tracing::info!(path = %path.display(), "已创建客户端初始配置");
    }
    Ok(path)
}
