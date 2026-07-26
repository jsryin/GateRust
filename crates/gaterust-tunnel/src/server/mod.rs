mod socks5;
mod stream;
mod udp;

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use quinn::{ConnectionError, Endpoint};
use subtle::ConstantTimeEq as _;
use tokio::{
    sync::{OwnedSemaphorePermit, RwLock, Semaphore, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Result, TunnelError, bootstrap,
    close::{ApplicationCloseCode, connection_error_or},
    config::{
        GroupSecret, ServerConfig, ServerQuicConfig, ServerTunnelConfig, TunnelKind,
        validate_group_key,
    },
    identity::validate_device_id,
    protocol::{
        AuthenticationStatus, CertificateBootstrapRequest, CertificateBootstrapResponse,
        ClientHandshake, ControlMessage, HANDSHAKE_TIMEOUT, PROTOCOL_VERSION, ServerControlMessage,
        ServerHandshake, ServerHello, read_frame, validate_declarations, write_frame,
    },
    runtime::{RegisterError, TunnelRuntime},
    tls,
    watcher::ConfigWatcher,
};

const MAX_PENDING_AUTHENTICATIONS: usize = 32;

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
    let (endpoint, certificate) = tls::server_endpoint(&initial.quic)?;
    let certificate = Arc::new(certificate);
    let local_address = endpoint.local_addr()?;
    let groups = Arc::new(RwLock::new(initial.credentials()));
    let mut listeners = ListenerManager::new(runtime.clone());
    listeners.apply(&initial.tunnels).await?;
    let accept_task = tokio::spawn(accept_connections(
        endpoint.clone(),
        runtime,
        Arc::clone(&groups),
        certificate,
        cancellation.child_token(),
    ));
    let immutable = initial.quic;
    tracing::info!(address = %local_address, "QUIC 隧道服务端已启动");

    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            changed = watcher.changed() => {
                if !changed {
                    break;
                }
                reload_server(&config_path, &immutable, &groups, &mut listeners).await;
            }
        }
    }

    cancellation.cancel();
    endpoint.close(
        ApplicationCloseCode::ServerShutdown.value(),
        b"server shutdown",
    );
    listeners.shutdown().await;
    await_task(accept_task, "QUIC 接入任务").await;
    endpoint.wait_idle().await;
    tracing::info!("QUIC 隧道服务端已停止");
    Ok(())
}

async fn reload_server(
    path: &Path,
    immutable: &ServerQuicConfig,
    groups: &RwLock<Vec<(String, GroupSecret)>>,
    listeners: &mut ListenerManager,
) {
    let config = match ServerConfig::load(path) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "新服务端配置无效，继续使用当前配置");
            return;
        }
    };
    if &config.quic != immutable {
        tracing::error!("quic.bind、证书或私钥不支持热更新，本次配置未应用");
        return;
    }
    let credentials = config.credentials();
    if let Err(error) = listeners.apply(&config.tunnels).await {
        tracing::error!(%error, "应用隧道监听配置失败");
        return;
    }
    *groups.write().await = credentials;
    tracing::info!(tunnels = config.tunnels.len(), "服务端配置已热更新");
}

async fn accept_connections(
    endpoint: Endpoint,
    runtime: TunnelRuntime,
    groups: Arc<RwLock<Vec<(String, GroupSecret)>>>,
    certificate: Arc<rustls::pki_types::CertificateDer<'static>>,
    cancellation: CancellationToken,
) {
    let ids = Arc::new(AtomicU64::new(1));
    let authentication_permits = Arc::new(Semaphore::new(MAX_PENDING_AUTHENTICATIONS));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            incoming = endpoint.accept() => match incoming {
                Some(incoming) => {
                    let Ok(permit) = Arc::clone(&authentication_permits).try_acquire_owned() else {
                        drop(incoming);
                        tracing::debug!("待认证客户端数量已达上限，拒绝新连接");
                        continue;
                    };
                    let runtime = runtime.clone();
                    let groups = Arc::clone(&groups);
                    let certificate = Arc::clone(&certificate);
                    let id = ids.fetch_add(1, Ordering::Relaxed);
                    tasks.spawn(async move {
                        match incoming.await {
                            Ok(connection) => {
                                if let Err(error) = authenticate(connection, id, runtime, groups, certificate, permit).await {
                                    tracing::warn!(%error, "QUIC 客户端认证失败");
                                }
                            }
                            Err(error) => tracing::debug!(%error, "QUIC 握手失败"),
                        }
                    });
                }
                None => break,
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
}

async fn authenticate(
    connection: quinn::Connection,
    id: u64,
    runtime: TunnelRuntime,
    groups: Arc<RwLock<Vec<(String, GroupSecret)>>>,
    certificate: Arc<rustls::pki_types::CertificateDer<'static>>,
    authentication_permit: OwnedSemaphorePermit,
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
    let hello = match handshake {
        ClientHandshake::Authenticate(hello) => hello,
        ClientHandshake::Bootstrap(request) => {
            return bootstrap_certificate(
                &connection,
                &mut send,
                request,
                &groups,
                certificate.as_ref(),
                authentication_permit,
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
    let group = if valid_hello {
        let groups = groups.read().await;
        groups.iter().find_map(|(name, secret)| {
            bool::from(secret.as_bytes().ct_eq(hello.key.as_slice())).then(|| name.clone())
        })
    } else {
        None
    };
    let Some(group) = group else {
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
    drop(authentication_permit);

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
    _authentication_permit: OwnedSemaphorePermit,
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
    active: HashMap<String, ListenerHandle>,
    retired: Vec<JoinHandle<()>>,
}

impl ListenerManager {
    fn new(runtime: TunnelRuntime) -> Self {
        Self {
            runtime,
            active: HashMap::new(),
            retired: Vec::new(),
        }
    }

    async fn apply(&mut self, configs: &[ServerTunnelConfig]) -> Result<()> {
        self.reap_retired().await;
        let previous: Vec<_> = self
            .active
            .values()
            .map(|handle| handle.config.clone())
            .collect();
        if let Err(error) = self.apply_once(configs).await {
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
                let handle = start_listener(config.clone(), self.runtime.clone()).await?;
                tracing::info!(tunnel = %config.name, kind = ?config.kind, address = %config.bind, "公网监听已启动");
                self.active.insert(config.name.clone(), handle);
            }
        }
        for config in configs {
            if let Some(handle) = self.active.get_mut(&config.name) {
                handle.config.clone_from(config);
            }
        }
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
        let tasks = std::mem::take(&mut self.retired);
        for mut task in tasks {
            if tokio::time::timeout(Duration::from_secs(10), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
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
        }
        for task in &self.retired {
            task.abort();
        }
    }
}

async fn start_listener(
    config: ServerTunnelConfig,
    runtime: TunnelRuntime,
) -> Result<ListenerHandle> {
    let cancellation = CancellationToken::new();
    let (stopped_sender, stopped) = oneshot::channel();
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

async fn await_task(mut task: JoinHandle<()>, name: &str) {
    match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
        Ok(Ok(())) => {}
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
            local_port: NonZeroU16::new(22),
            limit_bps: None,
            max_connections: 8,
            max_udp_sessions: 8,
            udp_idle_seconds: 30,
        };
        let mut updated = current.clone();
        updated.group = "home".into();
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
