//! `GateRust` 客户端应用运行时。

mod error;
mod paths;
mod trust;

use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

pub use error::{ClientError, Result};
use gaterust_tunnel::{
    ClientConfig, ClientServiceConfig, ClientStatus, ClientTunnel, ClientTunnelState,
    MAX_CLIENT_SERVICES, TunnelKind, run_client_with_status, verify_client_credentials,
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
    login: Mutex<()>,
}

const LOGIN_TIMEOUT: Duration = Duration::from_mins(1);

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
            login: Mutex::new(()),
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

    /// 验证服务器凭据，必要时下载证书，并在成功后保存配置。
    ///
    /// # Errors
    ///
    /// 地址或密钥无效、证书引导或认证失败、操作超时、配置不可读写时返回错误。
    pub async fn login(&self, address: String, key: String) -> Result<ClientConfig> {
        self.login_with_timeout(address, key, LOGIN_TIMEOUT).await
    }

    async fn login_with_timeout(
        &self,
        address: String,
        key: String,
        timeout: Duration,
    ) -> Result<ClientConfig> {
        let candidate_address = address.trim().to_owned();
        let candidate_key = key.clone();
        let result = match tokio::time::timeout(timeout, self.login_inner(address, key)).await {
            Ok(result) => result,
            Err(_) => Err(ClientError::LoginTimeout),
        };
        if result.is_err() {
            self.discard_unverified_candidate(&candidate_address, &candidate_key)
                .await?;
        }
        result
    }

    async fn login_inner(&self, address: String, key: String) -> Result<ClientConfig> {
        let _login = self.login.lock().await;
        let mut config = self.config().await?;
        let address = address.trim().to_owned();
        let server_changed = config.server.address != address;
        if server_changed {
            config.server.name = None;
            config.server.ca_certificate = None;
        }
        config.server.address = address;
        config.key = key;
        config.services.clear();
        config.validate()?;

        // 新凭据通过证书引导和正常 QUIC 认证后才落盘，超时不会触发后台自动重试。
        trust::prepare(&mut config, self.config_path.as_ref(), server_changed).await?;
        let runtime_config = config.resolved(self.config_path.as_ref())?;
        verify_client_credentials(&runtime_config).await?;
        self.save_config(config).await
    }

    async fn discard_unverified_candidate(&self, address: &str, key: &str) -> Result<()> {
        let config = self.config().await?;
        if config.server.ca_certificate.is_none()
            && config.server.address == address
            && config.key == key
        {
            let path = Arc::clone(&self.config_path);
            tokio::task::spawn_blocking(move || ClientConfig::reset(path.as_ref())).await??;
        }
        Ok(())
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
    use rcgen::generate_simple_self_signed;

    use super::*;

    const TEST_KEY: &str = "12345678901234567890123456789012";

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

    #[tokio::test]
    async fn login_timeout_does_not_persist_or_retry_candidate() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let config_path = directory.path().join("client.toml");
        let sink = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("绑定无响应 UDP 端口");
        let address = sink.local_addr().expect("读取 UDP 地址").to_string();
        ClientConfig::ensure_exists(&config_path).expect("创建旧版客户端配置");
        let mut legacy = ClientConfig::read(&config_path).expect("读取旧版客户端配置");
        legacy.server.address = address.clone();
        legacy.key = TEST_KEY.into();
        legacy.save(&config_path).expect("保存旧版未认证配置");
        let runtime = ClientRuntime::start(Some(config_path.clone())).expect("启动客户端运行时");

        let result = runtime
            .login_with_timeout(address, TEST_KEY.into(), Duration::from_millis(50))
            .await;

        assert!(matches!(result, Err(ClientError::LoginTimeout)));
        let stored = ClientConfig::read(&config_path).expect("读取客户端配置");
        assert!(stored.server.address.is_empty());
        assert!(!directory.path().join("server.pem").exists());
        tokio::time::timeout(Duration::from_secs(1), async {
            while !matches!(runtime.status(), ClientStatus::Unconfigured { .. }) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("旧版后台重试应停止");
        runtime.shutdown().await.expect("停止客户端运行时");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn login_downloads_certificate_before_persisting_config() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let certificate =
            generate_simple_self_signed(vec!["localhost".into()]).expect("生成测试证书");
        std::fs::write(
            directory.path().join("source-server.pem"),
            certificate.cert.pem(),
        )
        .expect("保存服务端证书");
        std::fs::write(
            directory.path().join("server-key.pem"),
            certificate.signing_key.serialize_pem(),
        )
        .expect("保存服务端私钥");
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("预留 QUIC 端口");
        let address = socket.local_addr().expect("读取 QUIC 地址");
        drop(socket);
        let server_config = format!(
            r#"
[quic]
bind = "{address}"
certificate = "source-server.pem"
private_key = "server-key.pem"

[[groups]]
name = "test"
key = "{TEST_KEY}"
"#
        );
        let server_path = directory.path().join("server.toml");
        std::fs::write(&server_path, server_config).expect("保存服务端配置");
        let cancellation = CancellationToken::new();
        let server_cancel = cancellation.clone();
        let server = tokio::spawn(async move {
            gaterust_tunnel::run_server_with_shutdown(server_path, server_cancel).await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config_path = directory.path().join("client.toml");
        let runtime = ClientRuntime::start(Some(config_path.clone())).expect("启动客户端运行时");
        let rejected = runtime
            .login(
                address.to_string(),
                "00000000000000000000000000000000".into(),
            )
            .await;
        assert!(matches!(
            rejected,
            Err(ClientError::Tunnel(
                gaterust_tunnel::TunnelError::Authentication(_)
            ))
        ));
        assert!(!directory.path().join("server.pem").exists());
        assert!(
            ClientConfig::read(&config_path)
                .expect("读取未认证配置")
                .server
                .address
                .is_empty()
        );

        let config = runtime
            .login(address.to_string(), TEST_KEY.into())
            .await
            .expect("获取连接配置");
        assert_eq!(config.server.name.as_deref(), Some("localhost"));
        assert_eq!(
            config.server.ca_certificate.as_deref(),
            Some(std::path::Path::new("server.pem"))
        );
        assert!(directory.path().join("server.pem").is_file());
        ClientConfig::load(&config_path).expect("下载的证书应能加载");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if matches!(runtime.status(), ClientStatus::Connected { .. }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("客户端应使用下载证书完成连接");

        runtime.shutdown().await.expect("停止客户端运行时");
        cancellation.cancel();
        server
            .await
            .expect("服务端任务正常结束")
            .expect("服务端正常退出");
    }
}
