//! `GateRust` 客户端应用运行时。

mod error;
mod paths;

use std::{collections::HashSet, path::PathBuf, sync::Arc};

pub use error::{ClientError, Result};
use gaterust_tunnel::{
    ClientConfig, ClientServiceConfig, ClientStatus, ClientTunnel, ClientTunnelState,
    MAX_CLIENT_SERVICES, TunnelKind, run_client_with_status,
};
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

    /// 保存服务器凭据并触发登录，切换服务器时清除原有 TLS 覆盖项。
    ///
    /// # Errors
    ///
    /// 地址或密钥无效、配置不可读写时返回错误。
    pub async fn login(&self, address: String, key: String) -> Result<ClientConfig> {
        let mut config = self.config().await?;
        let address = address.trim().to_owned();
        if config.server.address != address {
            config.server.name = None;
            config.server.ca_certificate = None;
        }
        config.server.address = address;
        config.key = key;
        config.services.clear();
        self.save_config(config).await
    }

    /// 将选择的空闲隧道映射到服务端指定的本地回环端口。
    ///
    /// # Errors
    ///
    /// 尚未登录、隧道不存在或已被其他客户端占用时返回错误。
    pub async fn connect_tunnels(&self, names: Vec<String>) -> Result<ClientConfig> {
        let ClientStatus::Connected { tunnels, .. } = self.status() else {
            return Err(ClientError::InvalidOperation("尚未登录服务器".into()));
        };
        let services = services_for_selection(tunnels, names)?;

        let mut config = self.config().await?;
        config.services = services;
        self.save_config(config).await
    }

    /// 释放当前客户端占用的全部隧道，同时保持服务器登录。
    ///
    /// # Errors
    ///
    /// 配置不可读写时返回错误。
    pub async fn disconnect_tunnels(&self) -> Result<ClientConfig> {
        let mut config = self.config().await?;
        config.services.clear();
        self.save_config(config).await
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

fn services_for_selection(
    tunnels: Vec<ClientTunnel>,
    names: Vec<String>,
) -> Result<Vec<ClientServiceConfig>> {
    let requested = names.into_iter().collect::<HashSet<_>>();
    if requested.is_empty() {
        return Err(ClientError::InvalidOperation(
            "请至少选择一个空闲隧道".into(),
        ));
    }
    if requested.len() > MAX_CLIENT_SERVICES {
        return Err(ClientError::InvalidOperation(format!(
            "单个客户端最多连接 {MAX_CLIENT_SERVICES} 个隧道"
        )));
    }

    let mut services = Vec::with_capacity(requested.len());
    for tunnel in tunnels {
        if !requested.contains(&tunnel.name) {
            continue;
        }
        if tunnel.state == ClientTunnelState::Occupied {
            return Err(ClientError::InvalidOperation(format!(
                "隧道 {} 已被其他客户端占用",
                tunnel.name
            )));
        }
        let target = match tunnel.kind {
            TunnelKind::Tcp | TunnelKind::Udp => Some(format!(
                "127.0.0.1:{}",
                tunnel.local_port.ok_or_else(|| {
                    ClientError::InvalidOperation(format!("隧道 {} 缺少本地端口配置", tunnel.name))
                })?
            )),
            TunnelKind::Socks5 => None,
        };
        services.push(ClientServiceConfig {
            name: tunnel.name,
            kind: tunnel.kind,
            target,
        });
    }
    if services.len() != requested.len() {
        return Err(ClientError::InvalidOperation(
            "选择中包含服务器未提供的隧道".into(),
        ));
    }
    services.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    Ok(services)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_server_tunnels_to_local_services() {
        let tunnels = vec![
            ClientTunnel {
                name: "ssh".into(),
                kind: TunnelKind::Tcp,
                server_port: 22022,
                local_port: Some(22),
                state: ClientTunnelState::Idle,
            },
            ClientTunnel {
                name: "proxy".into(),
                kind: TunnelKind::Socks5,
                server_port: 1080,
                local_port: None,
                state: ClientTunnelState::Idle,
            },
        ];
        let services = services_for_selection(tunnels, vec!["proxy".into(), "ssh".into()])
            .expect("选择应生成本地服务");

        assert_eq!(services[0].name, "proxy");
        assert_eq!(services[0].target, None);
        assert_eq!(services[1].name, "ssh");
        assert_eq!(services[1].target.as_deref(), Some("127.0.0.1:22"));
    }

    #[test]
    fn rejects_occupied_tunnel_selection() {
        let tunnels = vec![ClientTunnel {
            name: "ssh".into(),
            kind: TunnelKind::Tcp,
            server_port: 22022,
            local_port: Some(22),
            state: ClientTunnelState::Occupied,
        }];

        assert!(services_for_selection(tunnels, vec!["ssh".into()]).is_err());
    }
}
