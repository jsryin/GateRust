//! `GateRust` 客户端应用运行时。

mod error;
mod paths;
mod trust;

use std::{collections::HashSet, future::Future, path::PathBuf, sync::Arc, time::Duration};

pub use error::{ClientError, Result};
use gaterust_tunnel::{
    ClientConfig, ClientController, ClientServiceConfig, ClientStatus, ClientTunnel,
    ClientTunnelState, MAX_CLIENT_SERVICES, TunnelKind, client_control_channel,
    run_managed_client_with_status, verify_client_credentials,
};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

/// 可嵌入桌面壳层的客户端运行时。
pub struct ClientRuntime {
    config_path: Arc<PathBuf>,
    status: watch::Receiver<ClientStatus>,
    shutdown: CancellationToken,
    task: Mutex<Option<JoinHandle<gaterust_tunnel::Result<()>>>>,
    login: Mutex<Option<LoginOperation>>,
    controller: ClientController,
}

#[derive(Clone)]
struct LoginOperation {
    cancellation: CancellationToken,
    completed: CancellationToken,
}

impl LoginOperation {
    fn new() -> Self {
        Self {
            cancellation: CancellationToken::new(),
            completed: CancellationToken::new(),
        }
    }
}

const LOGIN_TIMEOUT: Duration = Duration::from_mins(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const TUNNEL_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

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
        clear_saved_services(&config_path)?;

        let config_path = Arc::new(config_path);
        let shutdown = CancellationToken::new();
        let (status_sender, status) = watch::channel(ClientStatus::Starting);
        let (controller, commands) = client_control_channel();
        let task_status = status_sender.clone();
        let task_path = Arc::clone(&config_path);
        let task_shutdown = shutdown.clone();
        let task = runtime.spawn(async move {
            let result = run_managed_client_with_status(
                task_path.as_ref(),
                task_shutdown,
                status_sender,
                commands,
            )
            .await;
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
            login: Mutex::new(None),
            controller,
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
        let operation = self.begin_login().await?;
        let deadline = Instant::now() + timeout;
        let result = self
            .login_inner(address, key, &operation.cancellation, deadline)
            .await;
        let result = if result.is_err() {
            self.reset_after_failed_login()
                .await
                .map_or_else(Err, |()| result)
        } else {
            result
        };
        self.finish_login(&operation).await;
        result
    }

    async fn begin_login(&self) -> Result<LoginOperation> {
        let mut active = self.login.lock().await;
        if active.is_some() {
            return Err(ClientError::InvalidOperation(
                "正在获取连接配置，请稍候".into(),
            ));
        }
        let operation = LoginOperation::new();
        *active = Some(operation.clone());
        Ok(operation)
    }

    async fn finish_login(&self, operation: &LoginOperation) {
        self.login.lock().await.take();
        operation.completed.cancel();
    }

    async fn login_inner(
        &self,
        address: String,
        key: String,
        cancellation: &CancellationToken,
        deadline: Instant,
    ) -> Result<ClientConfig> {
        check_login_state(cancellation, deadline)?;
        let mut config = self.config().await?;
        check_login_state(cancellation, deadline)?;
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
        trust::prepare(
            &mut config,
            self.config_path.as_ref(),
            server_changed,
            cancellation,
            deadline,
        )
        .await?;
        check_login_state(cancellation, deadline)?;
        let runtime_config = config.resolved(self.config_path.as_ref())?;
        run_login_step(
            verify_client_credentials(&runtime_config),
            cancellation,
            deadline,
        )
        .await?;
        check_login_state(cancellation, deadline)?;
        let config = self.save_config(config).await?;
        check_login_state(cancellation, deadline)?;
        Ok(config)
    }

    async fn reset_after_failed_login(&self) -> Result<()> {
        let path = Arc::clone(&self.config_path);
        tokio::task::spawn_blocking(move || ClientConfig::reset(path.as_ref())).await??;
        Ok(())
    }

    /// 取消并等待当前连接配置获取结束；没有在途任务时直接返回。
    pub async fn cancel_login(&self) {
        let operation = self.login.lock().await.clone();
        if let Some(operation) = operation {
            operation.cancellation.cancel();
            operation.completed.cancelled().await;
        }
    }

    /// 启用选择的空闲隧道，并等待服务端确认最终状态。
    ///
    /// # Errors
    ///
    /// 控制会话未在线、隧道不可用、请求超时或服务端未能启用全部选择时返回错误。
    pub async fn enable_tunnels(&self, names: Vec<String>) -> Result<ClientStatus> {
        let ClientStatus::Online { tunnels, .. } = self.status() else {
            return Err(ClientError::InvalidOperation("尚未登录服务器".into()));
        };
        let services = services_for_selection(tunnels, names)?;
        let requested = services
            .iter()
            .map(|service| service.name.clone())
            .collect::<HashSet<_>>();
        let tunnels = self.update_services(services).await?;
        let enabled = tunnels
            .iter()
            .filter(|tunnel| tunnel.state == ClientTunnelState::Enabled)
            .map(|tunnel| tunnel.name.as_str())
            .collect::<HashSet<_>>();
        let mut failed = requested
            .iter()
            .filter(|name| !enabled.contains(name.as_str()))
            .map(String::as_str)
            .collect::<Vec<_>>();
        failed.sort_unstable();
        if !failed.is_empty() {
            return Err(ClientError::InvalidOperation(format!(
                "以下隧道未能启用: {}",
                failed.join(", ")
            )));
        }
        Ok(self.status())
    }

    /// 停用当前客户端的全部隧道，同时保持控制会话在线。
    ///
    /// # Errors
    ///
    /// 控制请求超时、中断或服务端未能释放全部隧道时返回错误。
    pub async fn disable_tunnels(&self) -> Result<ClientStatus> {
        let tunnels = self.update_services(Vec::new()).await?;
        if tunnels
            .iter()
            .any(|tunnel| tunnel.state == ClientTunnelState::Enabled)
        {
            return Err(ClientError::InvalidOperation("部分隧道未能停用".into()));
        }
        Ok(self.status())
    }

    async fn update_services(
        &self,
        services: Vec<ClientServiceConfig>,
    ) -> Result<Vec<ClientTunnel>> {
        tokio::time::timeout(
            TUNNEL_OPERATION_TIMEOUT,
            self.controller.update_services(services),
        )
        .await
        .map_err(|_| ClientError::TunnelOperationTimeout)?
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
    /// 后台任务异常退出、隧道清理失败或超过退出期限时返回错误。
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_with_timeout(SHUTDOWN_TIMEOUT).await
    }

    async fn shutdown_with_timeout(&self, grace_period: Duration) -> Result<()> {
        self.shutdown.cancel();
        let login = self.login.lock().await.clone();
        if let Some(operation) = &login {
            operation.cancellation.cancel();
        }
        let mut task = self.task.lock().await.take();

        let graceful_shutdown = async {
            if let Some(operation) = login {
                operation.completed.cancelled().await;
            }
            if let Some(task) = task.as_mut() {
                task.await??;
            }
            Ok(())
        };
        if let Ok(result) = tokio::time::timeout(grace_period, graceful_shutdown).await {
            result
        } else {
            // 退出期限耗尽后必须终止后台任务，避免窗口关闭但进程继续驻留。
            if let Some(task) = task {
                task.abort();
            }
            Err(ClientError::ShutdownTimeout)
        }
    }
}

fn check_login_state(cancellation: &CancellationToken, deadline: Instant) -> Result<()> {
    if cancellation.is_cancelled() {
        Err(ClientError::LoginCancelled)
    } else if Instant::now() >= deadline {
        Err(ClientError::LoginTimeout)
    } else {
        Ok(())
    }
}

async fn run_login_step<T, E>(
    future: impl Future<Output = std::result::Result<T, E>>,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<T>
where
    ClientError: From<E>,
{
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(ClientError::LoginCancelled),
        result = tokio::time::timeout_at(deadline, future) => {
            match result {
                Ok(result) => result.map_err(ClientError::from),
                Err(_) => Err(ClientError::LoginTimeout),
            }
        }
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

fn clear_saved_services(path: &std::path::Path) -> Result<()> {
    let Ok(mut config) = ClientConfig::read(path) else {
        return Ok(());
    };
    if config.services.is_empty() {
        return Ok(());
    }
    // 桌面端的隧道选择是进程内状态，避免重新打开应用时未经确认自动启用。
    config.services.clear();
    if config.validate().is_err() {
        return Ok(());
    }
    config.save(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, generate_simple_self_signed};

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

    #[test]
    fn removes_legacy_saved_services_for_desktop_runtime() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let path = directory.path().join("client.toml");
        let mut config = ClientConfig::initial();
        config.server.address = "127.0.0.1:2333".into();
        config.services.push(ClientServiceConfig {
            name: "ssh".into(),
            kind: TunnelKind::Tcp,
            target: Some("127.0.0.1:22".into()),
        });
        config.save(&path).expect("保存旧版客户端配置");

        clear_saved_services(&path).expect("清理旧版持久化服务");

        assert!(
            ClientConfig::read(&path)
                .expect("读取迁移后的客户端配置")
                .services
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_login_stops_retrying_existing_config() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let config_path = directory.path().join("client.toml");
        let sink = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("绑定无响应 UDP 端口");
        let address = sink.local_addr().expect("读取 UDP 地址").to_string();
        let certificate =
            generate_simple_self_signed(vec!["localhost".into()]).expect("生成测试证书");
        std::fs::write(directory.path().join("server.pem"), certificate.cert.pem())
            .expect("保存服务端证书");
        ClientConfig::ensure_exists(&config_path).expect("创建客户端配置");
        let mut existing = ClientConfig::read(&config_path).expect("读取客户端配置");
        existing.server.address = address.clone();
        existing.server.name = Some("localhost".into());
        existing.server.ca_certificate = Some(PathBuf::from("server.pem"));
        existing.key = TEST_KEY.into();
        existing.save(&config_path).expect("保存现有客户端配置");
        let runtime = ClientRuntime::start(Some(config_path.clone())).expect("启动客户端运行时");

        let result = runtime
            .login_with_timeout(address, TEST_KEY.into(), Duration::from_millis(50))
            .await;

        assert!(matches!(result, Err(ClientError::LoginTimeout)));
        let stored = ClientConfig::read(&config_path).expect("读取客户端配置");
        assert!(stored.server.address.is_empty());
        tokio::time::timeout(Duration::from_secs(1), async {
            while !matches!(runtime.status(), ClientStatus::Unconfigured { .. }) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("后台重试应停止");
        runtime.shutdown().await.expect("停止客户端运行时");
    }

    #[tokio::test]
    async fn cancellation_stops_active_login() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let config_path = directory.path().join("client.toml");
        let sink = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("绑定无响应 UDP 端口");
        let address = sink.local_addr().expect("读取 UDP 地址").to_string();
        let runtime =
            Arc::new(ClientRuntime::start(Some(config_path.clone())).expect("启动客户端运行时"));
        let login_runtime = Arc::clone(&runtime);
        let login_address = address.clone();
        let login =
            tokio::spawn(async move { login_runtime.login(login_address, TEST_KEY.into()).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if runtime.login.lock().await.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("获取任务应进入运行状态");
        let duplicate = runtime.login(address, TEST_KEY.into()).await;
        assert!(matches!(duplicate, Err(ClientError::InvalidOperation(_))));

        runtime.cancel_login().await;
        assert!(matches!(
            login.await.expect("获取任务正常结束"),
            Err(ClientError::LoginCancelled)
        ));
        assert!(runtime.login.lock().await.is_none());
        assert!(
            ClientConfig::read(&config_path)
                .expect("读取取消后的配置")
                .server
                .address
                .is_empty()
        );

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
                if matches!(runtime.status(), ClientStatus::Online { .. }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("客户端应使用下载证书完成连接");

        let mut ca_params =
            CertificateParams::new(vec!["localhost".into()]).expect("创建旧证书参数");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().expect("生成旧证书密钥");
        let ca_certificate = ca_params.self_signed(&ca_key).expect("生成旧 CA 证书");
        std::fs::write(directory.path().join("server.pem"), ca_certificate.pem())
            .expect("写入旧 CA 证书");

        runtime
            .login(address.to_string(), TEST_KEY.into())
            .await
            .expect("应重新引导并替换旧 CA 证书");
        let downloaded =
            std::fs::read(directory.path().join("server.pem")).expect("读取替换后的服务端证书");
        gaterust_tunnel::server_name_from_pem(&downloaded, &address.to_string())
            .expect("替换后的证书应为服务端叶证书");

        runtime.shutdown().await.expect("停止客户端运行时");
        cancellation.cancel();
        server
            .await
            .expect("服务端任务正常结束")
            .expect("服务端正常退出");
    }

    #[tokio::test]
    async fn shutdown_aborts_runtime_task_after_deadline() {
        struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }

        let directory = tempfile::tempdir().expect("创建临时目录");
        let (_status_sender, status) = watch::channel(ClientStatus::Starting);
        let (dropped_sender, dropped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _signal = DropSignal(Some(dropped_sender));
            std::future::pending::<gaterust_tunnel::Result<()>>().await
        });
        let runtime = ClientRuntime {
            config_path: Arc::new(directory.path().join("client.toml")),
            status,
            shutdown: CancellationToken::new(),
            task: Mutex::new(Some(task)),
            login: Mutex::new(None),
            controller: client_control_channel().0,
        };

        let result = runtime
            .shutdown_with_timeout(Duration::from_millis(10))
            .await;

        assert!(matches!(result, Err(ClientError::ShutdownTimeout)));
        tokio::time::timeout(Duration::from_secs(1), dropped)
            .await
            .expect("中止任务应及时释放 future")
            .expect("中止任务应发送释放信号");
        runtime.shutdown().await.expect("重复关闭应直接完成");
    }
}
