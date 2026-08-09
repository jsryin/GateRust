mod endpoint;
mod socks5;
mod stream;
mod udp;

use std::{
    collections::HashMap,
    net::IpAddr,
    num::NonZeroU64,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{StreamExt as _, stream::FuturesUnordered};
use quinn::{ConnectionError, Endpoint};
use subtle::ConstantTimeEq as _;
use tokio::{
    sync::{OwnedSemaphorePermit, RwLock, Semaphore, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize as _;

use crate::{
    Result, TunnelError, bootstrap,
    close::{ApplicationCloseCode, connection_error_or},
    config::{GroupSecret, ServerConfig, ServerTunnelConfig, TunnelKind, validate_group_key},
    identity::validate_device_id,
    protocol::{
        AuthenticationStatus, CertificateBootstrapRequest, CertificateBootstrapResponse,
        ClientHandshake, ControlMessage, HANDSHAKE_TIMEOUT, PROTOCOL_VERSION, ServerControlMessage,
        ServerHandshake, ServerHello, read_frame, validate_declarations, write_frame,
    },
    rate_limit::RateLimiter,
    resource::ResourceBudget,
    runtime::{RegisterError, TunnelRuntime, credential_digest},
    tls,
    watcher::ConfigWatcher,
};

use self::endpoint::QuicEndpoint;

const MAX_PENDING_AUTHENTICATIONS: usize = 32;
const MAX_PENDING_AUTHENTICATIONS_PER_IP: usize = 4;
const TASK_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// 运行隧道服务端，并按配置变化增删公网监听。
///
/// # Errors
///
/// 初始配置、TLS、监听地址或文件监听器初始化失败时返回错误。
pub async fn run_server(config_path: impl AsRef<Path>) -> Result<()> {
    let config_path = config_path.as_ref().to_owned();
    let cancellation = CancellationToken::new();
    let server = run_server_with_shutdown(config_path, cancellation.clone());
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            cancellation.cancel();
            server.await
        }
    }
}

/// 运行隧道服务端，直到取消令牌被触发。
///
/// # Errors
///
/// 初始配置、TLS、监听地址或文件监听器初始化失败时返回错误。
pub async fn run_server_with_shutdown(
    config_path: impl AsRef<Path>,
    cancellation: CancellationToken,
) -> Result<()> {
    run_server_with_runtime(config_path, TunnelRuntime::new(), cancellation).await
}

/// 使用可供控制面查询的运行时句柄启动隧道服务端。
///
/// # Errors
///
/// 初始配置、TLS、监听地址或文件监听器初始化失败时返回错误。
pub async fn run_server_with_runtime(
    config_path: impl AsRef<Path>,
    runtime: TunnelRuntime,
    cancellation: CancellationToken,
) -> Result<()> {
    let config_path = config_path.as_ref().to_owned();
    let initial = ServerConfig::load(&config_path)?;
    let mut watcher = ConfigWatcher::new(&config_path)?;
    let mut quic = QuicEndpoint::bind(initial.quic.clone())?;
    let local_address = quic.endpoint().local_addr()?;
    let credentials = initial.credentials();
    runtime.apply_credentials(&credentials).await;
    let groups = Arc::new(RwLock::new(credentials));
    let mut listeners = ListenerManager::new(runtime.clone());
    listeners.apply(&initial.tunnels).await?;
    runtime.report_config_applied(&initial)?;
    let mut accept_task = tokio::spawn(accept_connections(
        quic.endpoint().clone(),
        runtime.clone(),
        Arc::clone(&groups),
        quic.subscribe_credentials(),
        cancellation.child_token(),
    ));
    tracing::info!(address = %local_address, "QUIC 隧道服务端已启动");

    // Quinn 驱动失效会结束 accept 流，必须让主服务退出并交由进程管理器恢复。
    let (failure, accept_task_finished) = loop {
        tokio::select! {
            () = cancellation.cancelled() => break (None, false),
            result = &mut accept_task => {
                if cancellation.is_cancelled() {
                    break (None, true);
                }
                let error = match result {
                    Ok(Ok(())) => TunnelError::Protocol("QUIC 接入任务意外结束".into()),
                    Ok(Err(error)) => error,
                    Err(error) => TunnelError::Protocol(format!("QUIC 接入任务异常结束: {error}")),
                };
                break (Some(error), true);
            }
            changed = watcher.changed() => {
                if !changed {
                    break (None, false);
                }
                reload_server(
                    &config_path,
                    &runtime,
                    &groups,
                    &mut quic,
                    &mut listeners,
                ).await;
            }
        }
    };

    cancellation.cancel();
    quic.endpoint().close(
        ApplicationCloseCode::ServerShutdown.value(),
        b"server shutdown",
    );
    listeners.shutdown().await;
    if !accept_task_finished {
        await_task(accept_task, "QUIC 接入任务").await;
    }
    quic.endpoint().wait_idle().await;
    tracing::info!("QUIC 隧道服务端已停止");
    failure.map_or(Ok(()), Err)
}

async fn reload_server(
    path: &Path,
    runtime: &TunnelRuntime,
    groups: &RwLock<Vec<(String, GroupSecret)>>,
    quic: &mut QuicEndpoint,
    listeners: &mut ListenerManager,
) {
    let config = match ServerConfig::load(path) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "新服务端配置无效，继续使用当前配置");
            runtime.report_config_load_error(error.to_string());
            return;
        }
    };
    let quic_update = match quic.prepare(&config.quic) {
        Ok(update) => update,
        Err(error) => {
            tracing::error!(%error, "应用 QUIC 监听配置失败，继续使用当前入口");
            report_config_failed(runtime, &config, error.to_string());
            return;
        }
    };
    let credentials = config.credentials();
    let previous_tunnels = quic_update.as_ref().map(|_| listeners.configs());
    if let Err(error) = listeners.apply(&config.tunnels).await {
        tracing::error!(%error, "应用隧道监听配置失败");
        report_config_failed(runtime, &config, error.to_string());
        return;
    }
    if let Some(update) = quic_update
        && let Err(error) = quic.apply(update)
    {
        tracing::error!(%error, "切换 QUIC 监听配置失败，继续使用当前入口");
        let error = match listeners
            .apply(previous_tunnels.as_deref().unwrap_or_default())
            .await
        {
            Ok(()) => error.to_string(),
            Err(rollback_error) => {
                format!("切换 QUIC 入口失败: {error}; 回滚隧道监听也失败: {rollback_error}")
            }
        };
        report_config_failed(runtime, &config, error);
        return;
    }
    runtime.apply_credentials(&credentials).await;
    *groups.write().await = credentials;
    if let Err(error) = runtime.report_config_applied(&config) {
        tracing::error!(%error, "记录隧道配置应用状态失败");
    }
    tracing::info!(
        quic = %config.quic.bind,
        tunnels = config.tunnels.len(),
        "服务端配置已热更新"
    );
}

fn report_config_failed(runtime: &TunnelRuntime, config: &ServerConfig, error: String) {
    if let Err(status_error) = runtime.report_config_failed(config, error) {
        tracing::error!(%status_error, "记录隧道配置应用失败状态失败");
    }
}

async fn accept_connections(
    endpoint: Endpoint,
    runtime: TunnelRuntime,
    groups: Arc<RwLock<Vec<(String, GroupSecret)>>>,
    credentials: watch::Receiver<Arc<tls::ServerCredentials>>,
    cancellation: CancellationToken,
) -> Result<()> {
    let ids = Arc::new(AtomicU64::new(1));
    let authentication_permits = Arc::new(Semaphore::new(MAX_PENDING_AUTHENTICATIONS));
    let peer_admission = Arc::new(PeerAdmission::default());
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            incoming = endpoint.accept() => match incoming {
                Some(incoming) => {
                    let remote = incoming.remote_address();
                    if !incoming.remote_address_validated() {
                        if let Err(error) = incoming.retry() {
                            tracing::debug!(%remote, %error, "发送 QUIC Retry 失败");
                        }
                        continue;
                    }
                    let Some(peer_permit) = PeerAdmission::try_acquire(&peer_admission, remote.ip()) else {
                        tracing::debug!(%remote, "同一来源的待认证客户端数量已达上限");
                        continue;
                    };
                    let Ok(permit) = Arc::clone(&authentication_permits).try_acquire_owned() else {
                        drop(incoming);
                        tracing::debug!("待认证客户端数量已达上限，拒绝新连接");
                        continue;
                    };
                    let credentials = credentials.borrow().clone();
                    let connecting = match incoming.accept_with(Arc::clone(&credentials.server_config)) {
                        Ok(connecting) => connecting,
                        Err(error) => {
                            tracing::debug!(%remote, %error, "接受 QUIC 连接失败");
                            continue;
                        }
                    };
                    let runtime = runtime.clone();
                    let groups = Arc::clone(&groups);
                    let id = ids.fetch_add(1, Ordering::Relaxed);
                    tasks.spawn(async move {
                        let admission = AuthenticationAdmission {
                            _global: permit,
                            _peer: peer_permit,
                        };
                        match tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting).await {
                            Err(_) => tracing::debug!(%remote, "QUIC 握手超时"),
                            Ok(connection) => {
                                match connection {
                                    Ok(connection) => {
                                        if let Err(error) = authenticate(connection, id, runtime, groups, credentials, admission).await {
                                            tracing::warn!(%error, "QUIC 客户端认证失败");
                                        }
                                    }
                                    Err(error) => tracing::debug!(%error, "QUIC 握手失败"),
                                }
                            }
                        }
                    });
                }
                None if cancellation.is_cancelled() => break,
                None => return Err(TunnelError::Protocol("QUIC 端点接入驱动已停止".into())),
            },
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "QUIC 客户端任务异常结束");
                }
            }
        }
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::warn!(%error, "QUIC 客户端任务异常结束");
        }
    }
    Ok(())
}

async fn authenticate(
    connection: quinn::Connection,
    id: u64,
    runtime: TunnelRuntime,
    groups: Arc<RwLock<Vec<(String, GroupSecret)>>>,
    credentials: Arc<tls::ServerCredentials>,
    authentication_admission: AuthenticationAdmission,
) -> Result<()> {
    let mut changes = runtime.subscribe();
    let remote = connection.remote_address();
    let (mut send, mut receive) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi())
        .await
        .map_err(|_| TunnelError::Timeout("等待认证流"))??;
    let handshake: ClientHandshake =
        tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut receive))
            .await
            .map_err(|_| TunnelError::Timeout("读取认证信息"))?
            .map_err(|error| connection_error_or(&connection, error))?;
    let mut hello = match handshake {
        ClientHandshake::Authenticate(hello) => hello,
        ClientHandshake::Bootstrap(request) => {
            return bootstrap_certificate(
                &connection,
                &mut send,
                request,
                &groups,
                &credentials.certificate,
                authentication_admission,
            )
            .await;
        }
    };
    let valid_key =
        std::str::from_utf8(&hello.key).is_ok_and(|key| validate_group_key(key).is_ok());
    let valid_hello = hello.version == PROTOCOL_VERSION
        && valid_key
        && validate_device_id(&hello.device_id).is_ok()
        && validate_declarations(&hello.services).is_ok();
    let authenticated_group = if valid_hello {
        let groups = groups.read().await;
        groups.iter().find_map(|(name, secret)| {
            bool::from(secret.as_bytes().ct_eq(hello.key.as_slice()))
                .then(|| (name.clone(), credential_digest(secret.as_bytes())))
        })
    } else {
        None
    };
    hello.key.zeroize();
    let Some((group, authenticated_credential)) = authenticated_group else {
        reject_authentication(
            &connection,
            &mut send,
            AuthenticationStatus::Rejected,
            "认证失败",
        )
        .await?;
        return Err(TunnelError::Protocol("密钥或设备信息无效".into()));
    };
    let declared_services = hello.services.len();
    match runtime
        .register(
            id,
            hello.device_id.clone(),
            group.clone(),
            authenticated_credential,
            connection.clone(),
            hello.services,
        )
        .await
    {
        Ok(()) => {}
        Err(RegisterError::DeviceIdConflict) => {
            reject_authentication(
                &connection,
                &mut send,
                AuthenticationStatus::DeviceIdConflict,
                "设备 ID 已在线",
            )
            .await?;
            return Ok(());
        }
        Err(RegisterError::Capacity) => {
            reject_authentication(
                &connection,
                &mut send,
                AuthenticationStatus::ServerBusy,
                "在线客户端数量已达上限",
            )
            .await?;
            return Ok(());
        }
        Err(RegisterError::CredentialsChanged) => {
            reject_authentication(
                &connection,
                &mut send,
                AuthenticationStatus::Rejected,
                "认证凭据已变更，请重试",
            )
            .await?;
            return Ok(());
        }
    }
    // 先确认已观察到注册变更，再生成快照；后续变更不会在握手窗口内丢失。
    changes.borrow_and_update();
    let tunnels = runtime.catalog(id).await;
    let active_tunnels = tunnels
        .iter()
        .filter(|tunnel| tunnel.state == crate::ClientTunnelState::Enabled)
        .count();
    if let Err(error) = write_frame(
        &mut send,
        &ServerHandshake::Authenticate(ServerHello {
            status: AuthenticationStatus::Accepted,
            message: String::new(),
            tunnels,
        }),
    )
    .await
    {
        runtime.unregister(id).await;
        return Err(connection_error_or(&connection, error));
    }
    drop(authentication_admission);

    let device_id = hello.device_id;
    tracing::info!(
        session_id = id,
        group,
        %device_id,
        %remote,
        declared_services,
        active_tunnels,
        "客户端控制会话已上线"
    );
    let result = loop {
        tokio::select! {
            error = connection.closed() => break SessionEnd::Connection(error),
            message = read_frame::<_, ControlMessage>(&mut receive) => {
                match message {
                    Ok(ControlMessage::UpdateServices { request_id, services }) => {
                        if let Err(error) = validate_declarations(&services) {
                            break SessionEnd::Control(error);
                        }
                        let Some(update) = runtime.update_services(id, services).await else {
                            break SessionEnd::Control(TunnelError::Protocol("客户端会话已注销".into()));
                        };
                        // 回执中的快照已经覆盖本次运行时修订，避免再向请求方发送重复快照。
                        changes.borrow_and_update();
                        let response = ServerControlMessage::ServicesApplied {
                            request_id,
                            tunnels: runtime.catalog(id).await,
                        };
                        if let Err(error) = write_frame(&mut send, &response).await {
                            break session_end(&connection, error);
                        }
                        if update.changed {
                            tracing::info!(
                                session_id = id,
                                group,
                                %device_id,
                                declared_services = update.declared_services,
                                active_tunnels = update.active_tunnels,
                                claimed_tunnels = update.claimed_tunnels,
                                released_tunnels = update.released_tunnels,
                                "客户端隧道声明已应用"
                            );
                        } else {
                            tracing::debug!(
                                session_id = id,
                                group,
                                %device_id,
                                declared_services = update.declared_services,
                                active_tunnels = update.active_tunnels,
                                "客户端隧道声明无变化"
                            );
                        }
                    }
                    Err(error) => break session_end(&connection, error),
                }
            }
            changed = changes.changed() => {
                if changed.is_err() {
                    break SessionEnd::Control(TunnelError::Protocol("隧道状态通道已关闭".into()));
                }
                let snapshot = ServerControlMessage::TunnelSnapshot(runtime.catalog(id).await);
                if let Err(error) = write_frame(&mut send, &snapshot).await {
                    break session_end(&connection, error);
                }
            }
        }
    };
    if matches!(result, SessionEnd::Control(_)) {
        connection.close(
            ApplicationCloseCode::ServerConnectionError.value(),
            b"server control channel failed",
        );
    }
    runtime.unregister(id).await;
    log_session_end(id, &group, &device_id, remote, &result);
    Ok(())
}

enum SessionEnd {
    Connection(ConnectionError),
    Control(TunnelError),
}

fn session_end(connection: &quinn::Connection, fallback: TunnelError) -> SessionEnd {
    connection
        .close_reason()
        .map_or(SessionEnd::Control(fallback), SessionEnd::Connection)
}

fn log_session_end(
    session_id: u64,
    group: &str,
    device_id: &str,
    remote: std::net::SocketAddr,
    end: &SessionEnd,
) {
    match end {
        SessionEnd::Connection(error) if is_expected_client_close(error) => {
            tracing::info!(
                session_id,
                group,
                %device_id,
                %remote,
                reason = %error,
                "客户端控制会话已下线"
            );
        }
        SessionEnd::Connection(error) => {
            tracing::warn!(session_id, group, %device_id, %remote, %error, "客户端控制会话异常结束");
        }
        SessionEnd::Control(error) => {
            tracing::warn!(session_id, group, %device_id, %remote, %error, "客户端控制通道异常结束");
        }
    }
}

fn is_expected_client_close(error: &ConnectionError) -> bool {
    matches!(error, ConnectionError::LocallyClosed)
        || ApplicationCloseCode::ClientReconfigure.matches_error(error)
        || ApplicationCloseCode::ClientShutdown.matches_error(error)
        || ApplicationCloseCode::CredentialCheckComplete.matches_error(error)
}

async fn bootstrap_certificate(
    connection: &quinn::Connection,
    send: &mut quinn::SendStream,
    request: CertificateBootstrapRequest,
    groups: &RwLock<Vec<(String, GroupSecret)>>,
    certificate: &rustls::pki_types::CertificateDer<'_>,
    _authentication_admission: AuthenticationAdmission,
) -> Result<()> {
    let accepted = if request.version == PROTOCOL_VERSION {
        let groups = groups.read().await;
        groups.iter().find_map(|(_, secret)| {
            bootstrap::verify_client_proof(
                secret.as_bytes(),
                request.version,
                &request.client_nonce,
                certificate.as_ref(),
                &request.proof,
            )
            .then(|| {
                let server_nonce = bootstrap::random_nonce();
                bootstrap::server_proof(
                    secret.as_bytes(),
                    request.version,
                    &request.client_nonce,
                    &server_nonce,
                    certificate.as_ref(),
                )
                .map(|proof| CertificateBootstrapResponse::Accepted {
                    server_nonce,
                    proof,
                })
            })
        })
    } else {
        None
    };
    let (response, valid) = match accepted.transpose()? {
        Some(response) => (response, true),
        None => (
            CertificateBootstrapResponse::Rejected {
                message: "密钥验证失败".into(),
            },
            false,
        ),
    };
    write_frame(send, &ServerHandshake::Bootstrap(response))
        .await
        .map_err(|error| connection_error_or(connection, error))?;
    send.finish()
        .map_err(|error| TunnelError::Protocol(format!("结束证书引导响应流失败: {error}")))?;
    let _ = tokio::time::timeout(HANDSHAKE_TIMEOUT, send.stopped()).await;
    connection.close(
        ApplicationCloseCode::CertificateBootstrapComplete.value(),
        b"certificate bootstrap complete",
    );
    if valid {
        Ok(())
    } else {
        Err(TunnelError::Protocol("证书引导密钥验证失败".into()))
    }
}

async fn reject_authentication(
    connection: &quinn::Connection,
    send: &mut quinn::SendStream,
    status: AuthenticationStatus,
    message: &str,
) -> Result<()> {
    write_frame(
        send,
        &ServerHandshake::Authenticate(ServerHello {
            status,
            message: message.into(),
            tunnels: Vec::new(),
        }),
    )
    .await
    .map_err(|error| connection_error_or(connection, error))?;
    send.finish()
        .map_err(|error| TunnelError::Protocol(format!("结束认证响应流失败: {error}")))?;
    let _ = tokio::time::timeout(HANDSHAKE_TIMEOUT, send.stopped()).await;
    connection.close(
        ApplicationCloseCode::AuthenticationFailed.value(),
        b"authentication failed",
    );
    Ok(())
}

struct ListenerHandle {
    config: ServerTunnelConfig,
    cancellation: CancellationToken,
    stopped: oneshot::Receiver<()>,
    task: JoinHandle<()>,
}

struct ListenerManager {
    runtime: TunnelRuntime,
    budget: ResourceBudget,
    limiters: HashMap<String, LimiterEntry>,
    active: HashMap<String, ListenerHandle>,
    retired: Vec<JoinHandle<()>>,
}

impl ListenerManager {
    fn new(runtime: TunnelRuntime) -> Self {
        Self {
            runtime,
            budget: ResourceBudget::new(),
            limiters: HashMap::new(),
            active: HashMap::new(),
            retired: Vec::new(),
        }
    }

    fn configs(&self) -> Vec<ServerTunnelConfig> {
        self.active
            .values()
            .map(|handle| handle.config.clone())
            .collect()
    }

    async fn apply(&mut self, configs: &[ServerTunnelConfig]) -> Result<()> {
        self.reap_retired().await;
        let previous: Vec<_> = self
            .active
            .values()
            .map(|handle| handle.config.clone())
            .collect();
        let previous_limiters = self.limiters.clone();
        if let Err(error) = self.apply_once(configs).await {
            self.limiters = previous_limiters;
            if let Err(rollback_error) = self.apply_once(&previous).await {
                return Err(TunnelError::InvalidConfig(format!(
                    "应用监听配置失败: {error}; 回滚也失败: {rollback_error}"
                )));
            }
            return Err(error);
        }
        self.runtime.apply_tunnels(configs).await;
        Ok(())
    }

    async fn apply_once(&mut self, configs: &[ServerTunnelConfig]) -> Result<()> {
        let desired: HashMap<_, _> = configs
            .iter()
            .map(|config| (config.name.clone(), config.clone()))
            .collect();
        let removed: Vec<_> = self
            .active
            .iter()
            .filter(|(name, handle)| {
                desired
                    .get(*name)
                    .is_none_or(|config| !same_listener(&handle.config, config))
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in removed {
            self.stop(&name).await;
        }
        for config in configs {
            if !self.active.contains_key(&config.name) {
                let limiter = match self.limiters.get(&config.name) {
                    Some(entry) if entry.limit == config.limit_bps => entry.limiter.clone(),
                    _ => {
                        let limiter = RateLimiter::new(config.limit_bps);
                        self.limiters.insert(
                            config.name.clone(),
                            LimiterEntry {
                                limit: config.limit_bps,
                                limiter: limiter.clone(),
                            },
                        );
                        limiter
                    }
                };
                let handle = start_listener(
                    config.clone(),
                    self.runtime.clone(),
                    self.budget.clone(),
                    limiter,
                )
                .await?;
                tracing::info!(tunnel = %config.name, kind = ?config.kind, address = %config.bind, "公网监听已启动");
                self.active.insert(config.name.clone(), handle);
            }
        }
        for config in configs {
            if let Some(handle) = self.active.get_mut(&config.name) {
                handle.config.clone_from(config);
            }
        }
        self.limiters.retain(|name, _| desired.contains_key(name));
        Ok(())
    }

    async fn reap_retired(&mut self) {
        let mut index = 0;
        while index < self.retired.len() {
            if self.retired[index].is_finished() {
                let task = self.retired.swap_remove(index);
                if let Err(error) = task.await {
                    tracing::warn!(%error, "已停止的监听任务异常结束");
                }
            } else {
                index += 1;
            }
        }
    }

    async fn stop(&mut self, name: &str) {
        let Some(handle) = self.active.remove(name) else {
            return;
        };
        handle.cancellation.cancel();
        if handle.stopped.await.is_err() {
            tracing::debug!(tunnel = name, "监听任务未发送停止确认");
        }
        tracing::info!(tunnel = name, "公网监听已停止");
        self.retired.push(handle.task);
    }

    async fn shutdown(&mut self) {
        let names: Vec<_> = self.active.keys().cloned().collect();
        for name in names {
            self.stop(&name).await;
        }
        let mut tasks = std::mem::take(&mut self.retired)
            .into_iter()
            .collect::<FuturesUnordered<_>>();
        let graceful = async {
            while let Some(result) = tasks.next().await {
                if let Err(error) = result {
                    tracing::warn!(%error, "已停止的监听任务异常结束");
                }
            }
        };
        if tokio::time::timeout(TASK_SHUTDOWN_TIMEOUT, graceful)
            .await
            .is_err()
        {
            tracing::warn!("等待监听连接退出超时，正在终止剩余任务");
            for task in &tasks {
                task.abort();
            }
            while tasks.next().await.is_some() {}
        }
    }
}

fn same_listener(current: &ServerTunnelConfig, updated: &ServerTunnelConfig) -> bool {
    current.name == updated.name
        && current.kind == updated.kind
        && current.bind == updated.bind
        && current.limit_bps == updated.limit_bps
        && match current.kind {
            TunnelKind::Tcp | TunnelKind::Socks5 => {
                current.max_connections == updated.max_connections
            }
            TunnelKind::Udp => {
                current.max_udp_sessions == updated.max_udp_sessions
                    && current.udp_idle_seconds == updated.udp_idle_seconds
            }
        }
}

impl Drop for ListenerManager {
    fn drop(&mut self) {
        for handle in self.active.values() {
            handle.cancellation.cancel();
            handle.task.abort();
        }
        for task in &self.retired {
            task.abort();
        }
    }
}

async fn start_listener(
    config: ServerTunnelConfig,
    runtime: TunnelRuntime,
    budget: ResourceBudget,
    limiter: RateLimiter,
) -> Result<ListenerHandle> {
    let cancellation = CancellationToken::new();
    let (stopped_sender, stopped) = oneshot::channel();
    let resources = ListenerResources { budget, limiter };
    let task = match config.kind {
        TunnelKind::Tcp | TunnelKind::Socks5 => {
            let (listener, permits) = stream::bind(&config).await?;
            let child = cancellation.clone();
            let task_config = config.clone();
            tokio::spawn(stream::run(
                listener,
                permits,
                task_config,
                runtime,
                resources,
                child,
                stopped_sender,
            ))
        }
        TunnelKind::Udp => {
            let socket = udp::bind(&config).await?;
            let child = cancellation.clone();
            let task_config = config.clone();
            tokio::spawn(udp::run(
                socket,
                task_config,
                runtime,
                resources,
                child,
                stopped_sender,
            ))
        }
    };
    Ok(ListenerHandle {
        config,
        cancellation,
        stopped,
        task,
    })
}

#[derive(Clone)]
struct LimiterEntry {
    limit: Option<NonZeroU64>,
    limiter: RateLimiter,
}

struct ListenerResources {
    budget: ResourceBudget,
    limiter: RateLimiter,
}

struct AuthenticationAdmission {
    _global: OwnedSemaphorePermit,
    _peer: PeerAdmissionPermit,
}

#[derive(Default)]
struct PeerAdmission {
    active: Mutex<HashMap<IpAddr, usize>>,
}

impl PeerAdmission {
    fn try_acquire(admission: &Arc<Self>, address: IpAddr) -> Option<PeerAdmissionPermit> {
        let mut active = admission
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = active.entry(address).or_default();
        if *count >= MAX_PENDING_AUTHENTICATIONS_PER_IP {
            return None;
        }
        *count += 1;
        Some(PeerAdmissionPermit {
            admission: Arc::clone(admission),
            address,
        })
    }
}

struct PeerAdmissionPermit {
    admission: Arc<PeerAdmission>,
    address: IpAddr,
}

impl Drop for PeerAdmissionPermit {
    fn drop(&mut self) {
        let mut active = self
            .admission
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = active.get_mut(&self.address) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.address);
            }
        }
    }
}

async fn await_task(mut task: JoinHandle<Result<()>>, name: &str) {
    match tokio::time::timeout(TASK_SHUTDOWN_TIMEOUT, &mut task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => tracing::warn!(%error, task = name, "后台任务异常结束"),
        Ok(Err(error)) => tracing::warn!(%error, task = name, "后台任务异常结束"),
        Err(_) => {
            tracing::warn!(task = name, "等待后台任务退出超时");
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use quinn::ApplicationClose;

    use super::*;

    #[test]
    fn runtime_metadata_change_keeps_listener() {
        let current = ServerTunnelConfig {
            name: "ssh".into(),
            group: "office".into(),
            kind: TunnelKind::Tcp,
            bind: "127.0.0.1:22022".parse().expect("测试地址有效"),
            local_ip: "127.0.0.1".into(),
            local_port: NonZeroU16::new(22),
            limit_bps: None,
            max_connections: 8,
            max_udp_sessions: 8,
            udp_idle_seconds: 30,
        };
        let mut updated = current.clone();
        updated.group = "home".into();
        updated.local_ip = "localhost".into();
        updated.local_port = NonZeroU16::new(2222);
        assert!(same_listener(&current, &updated));

        updated.max_connections += 1;
        assert!(!same_listener(&current, &updated));
    }

    #[test]
    fn classifies_only_normal_client_session_closes_as_expected() {
        for code in [
            ApplicationCloseCode::ClientReconfigure,
            ApplicationCloseCode::ClientShutdown,
            ApplicationCloseCode::CredentialCheckComplete,
        ] {
            let error = ConnectionError::ApplicationClosed(ApplicationClose {
                error_code: code.value(),
                reason: "normal".into(),
            });
            assert!(is_expected_client_close(&error));
        }

        let client_error = ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: ApplicationCloseCode::ClientConnectionError.value(),
            reason: "failed".into(),
        });
        assert!(!is_expected_client_close(&client_error));
        assert!(!is_expected_client_close(&ConnectionError::TimedOut));
        assert!(is_expected_client_close(&ConnectionError::LocallyClosed));
    }
}
