use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::Duration,
};

use futures_util::{StreamExt as _, stream::FuturesUnordered};
use quinn::Connection;
use rand::RngExt as _;
use rustls::pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::{RwLock, watch},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize as _;

use crate::{
    Result, TunnelError, bootstrap,
    certificate::DownloadedServerCertificate,
    client_control::{
        ClientCommand, ClientCommandReceiver, CommandResponse, SERVICE_UPDATE_TIMEOUT,
    },
    close::{ApplicationCloseCode, connection_error_or},
    config::{
        ClientConfig, ClientServerConfig, ClientServiceConfig, TunnelKind, validate_group_key,
    },
    identity::DeviceIdentity,
    protocol::{
        AuthenticationStatus, CertificateBootstrapRequest, CertificateBootstrapResponse,
        ClientHandshake, ClientHello, ControlMessage, HANDSHAKE_TIMEOUT, MAX_DATAGRAM, OpenRequest,
        OpenResponse, PROTOCOL_VERSION, ServerControlMessage, ServerHandshake, ServiceDeclaration,
        read_datagram, read_frame, write_datagram, write_frame,
    },
    rate_limit::RateLimiter,
    relay::{self, QuinnStream},
    tls,
    watcher::ConfigWatcher,
};

const MAX_RESOLVED_ADDRESSES: usize = 8;
const BOOTSTRAP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const CONFIG_RELOAD_GRACE: Duration = Duration::from_millis(100);
const CONFIG_RELOAD_RETRY_DELAY: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientTunnelState {
    Idle,
    Enabled,
    Occupied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientTunnel {
    pub name: String,
    pub kind: TunnelKind,
    pub server_port: u16,
    #[serde(default)]
    pub local_ip: Option<String>,
    pub local_port: Option<u16>,
    pub state: ClientTunnelState,
}

impl ClientTunnel {
    /// 返回客户端应连接的本地目标；旧服务端未下发 IP 时使用回环地址。
    #[must_use]
    pub fn local_target(&self) -> Option<String> {
        if self.kind == TunnelKind::Socks5 {
            return None;
        }
        let port = self.local_port?;
        let host = self.local_ip.as_deref().unwrap_or(crate::DEFAULT_LOCAL_IP);
        if host.parse::<std::net::Ipv6Addr>().is_ok() {
            Some(format!("[{host}]:{port}"))
        } else {
            Some(format!("{host}:{port}"))
        }
    }
}

/// 客户端控制会话状态，供本机管理界面展示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientStatus {
    Starting,
    Unconfigured {
        reason: String,
    },
    Connecting {
        server: String,
    },
    Online {
        server: String,
        device_id: String,
        tunnels: Vec<ClientTunnel>,
    },
    Reconnecting {
        error: String,
        retry_seconds: u64,
    },
    Stopped {
        reason: Option<String>,
    },
}

/// 使用分组密钥证明下载并认证服务端证书。
///
/// 引导 TLS 连接不会发送原始分组密钥；只有服务端返回绑定当前证书的有效密钥证明时，
/// 证书才会交给调用方保存。
///
/// # Errors
///
/// 地址或密钥无效、所有解析地址均连接失败、服务端拒绝密钥或证明无效时返回错误。
pub async fn fetch_server_certificate(
    address: &str,
    key: &str,
) -> Result<DownloadedServerCertificate> {
    validate_group_key(key)?;
    let config = ClientConfig {
        key: key.to_owned(),
        server: ClientServerConfig {
            address: address.to_owned(),
            name: None,
            ca_certificate: None,
        },
        services: Vec::new(),
    };
    config.validate()?;
    let server_name = config.server_name()?.to_owned();
    let client_config = tls::bootstrap_client_config()?;
    let addresses = resolve_addresses(address).await?;
    try_addresses(addresses, |server_address| {
        bootstrap_at_address(
            server_address,
            client_config.clone(),
            &server_name,
            address,
            key,
        )
    })
    .await
}

/// 建立一次受信任的 QUIC 连接并验证分组密钥，成功后立即释放探测会话。
///
/// # Errors
///
/// 配置、TLS、网络或分组认证失败时返回错误。
pub async fn verify_client_credentials(config: &ClientConfig) -> Result<Vec<ClientTunnel>> {
    config.validate()?;
    let (endpoint, connection) = connect_server(config).await?;
    let device_id = format!("credential-check-{:016x}", rand::rng().random::<u64>());
    let result = authenticate(&connection, config, &device_id).await;
    connection.close(
        ApplicationCloseCode::CredentialCheckComplete.value(),
        b"credential check complete",
    );
    endpoint.wait_idle().await;
    match result? {
        AuthenticationResult::Accepted { tunnels, .. } => Ok(tunnels),
        AuthenticationResult::DeviceIdConflict => {
            Err(TunnelError::Protocol("临时认证设备 ID 冲突".into()))
        }
    }
}

/// 运行隧道客户端，监听配置变化并在连接断开后自动重试。
///
/// # Errors
///
/// 初始配置无效、无法创建文件监听器或无法注册退出信号时返回错误。
pub async fn run_client(config_path: impl AsRef<Path>) -> Result<()> {
    let config_path = config_path.as_ref().to_owned();
    let shutdown = CancellationToken::new();
    let client = run_client_with_shutdown(config_path, shutdown.clone());
    tokio::pin!(client);
    tokio::select! {
        result = &mut client => result,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            shutdown.cancel();
            client.await
        }
    }
}

/// 运行隧道客户端，直到取消令牌被触发。
///
/// # Errors
///
/// 初始配置无效或无法创建文件监听器时返回错误。连接类错误会在内部退避重试。
pub async fn run_client_with_shutdown(
    config_path: impl AsRef<Path>,
    shutdown: CancellationToken,
) -> Result<()> {
    let (status, _status_receiver) = watch::channel(ClientStatus::Starting);
    run_client_with_status(config_path, shutdown, status).await
}

/// 运行隧道客户端，并发布连接状态变化。
///
/// # Errors
///
/// 初始配置无效或无法创建文件监听器时返回错误。连接类错误会在内部退避重试。
pub async fn run_client_with_status(
    config_path: impl AsRef<Path>,
    shutdown: CancellationToken,
    status: watch::Sender<ClientStatus>,
) -> Result<()> {
    run_client_loop(config_path, shutdown, status, None).await
}

/// 运行由命令通道管理临时隧道选择的客户端，并发布控制会话状态。
///
/// 配置文件中的服务列表在此模式下不会自动启用，服务选择仅在本次进程中保留。
///
/// # Errors
///
/// 初始配置无效或无法创建文件监听器时返回错误。连接类错误会在内部退避重试。
pub async fn run_managed_client_with_status(
    config_path: impl AsRef<Path>,
    shutdown: CancellationToken,
    status: watch::Sender<ClientStatus>,
    commands: ClientCommandReceiver,
) -> Result<()> {
    run_client_loop(config_path, shutdown, status, Some(commands)).await
}

async fn run_client_loop(
    config_path: impl AsRef<Path>,
    shutdown: CancellationToken,
    status: watch::Sender<ClientStatus>,
    mut commands: Option<ClientCommandReceiver>,
) -> Result<()> {
    let config_path = config_path.as_ref().to_owned();
    let mut watcher = ConfigWatcher::new(&config_path)?;
    let Some(mut config) =
        wait_for_initial_config(&config_path, &mut watcher, &shutdown, &status).await
    else {
        status.send_replace(ClientStatus::Stopped { reason: None });
        return Ok(());
    };
    if commands.is_some() {
        config.services.clear();
    }
    let mut identity = DeviceIdentity::load(&config_path)?;
    let mut retry = Duration::from_secs(1);
    let mut stop_reason = None;

    'running: while !shutdown.is_cancelled() {
        status.send_replace(ClientStatus::Connecting {
            server: config.server.address.clone(),
        });
        match connect_and_run(
            &mut config,
            &identity,
            &config_path,
            &mut watcher,
            &shutdown,
            &status,
            commands.as_mut(),
        )
        .await
        {
            ConnectionEnd::Reconfigure(updated) => {
                config = updated;
                retry = Duration::from_secs(1);
            }
            ConnectionEnd::Disconnected(error) => {
                status.send_replace(ClientStatus::Reconnecting {
                    error: error.to_string(),
                    retry_seconds: retry.as_secs(),
                });
                tracing::warn!(%error, delay_seconds = retry.as_secs(), "客户端控制连接断开，稍后重试");
                let became_unconfigured = tokio::select! {
                    () = shutdown.cancelled() => break 'running,
                    () = tokio::time::sleep(retry) => false,
                    changed = watcher.changed() => {
                        if !changed {
                            break 'running;
                        }
                        match load_client_config_after_change(&config_path).await {
                            Ok(updated) => {
                                config = reconcile_config_update(&config, updated, commands.is_some());
                                false
                            }
                            Err(_) => true,
                        }
                    }
                };
                if became_unconfigured {
                    let Some(updated) =
                        wait_for_initial_config(&config_path, &mut watcher, &shutdown, &status)
                            .await
                    else {
                        break;
                    };
                    config = reconcile_config_update(&config, updated, commands.is_some());
                    retry = Duration::from_secs(1);
                    continue;
                }
                retry = (retry * 2).min(Duration::from_secs(30));
            }
            ConnectionEnd::Unconfigured => {
                let Some(updated) =
                    wait_for_initial_config(&config_path, &mut watcher, &shutdown, &status).await
                else {
                    break;
                };
                config = reconcile_config_update(&config, updated, commands.is_some());
                retry = Duration::from_secs(1);
            }
            ConnectionEnd::DeviceIdConflict => {
                identity.resolve_conflict()?;
                tracing::info!(device_id = identity.as_str(), "设备 ID 冲突，已生成新 ID");
                retry = Duration::from_secs(1);
            }
            ConnectionEnd::AdministratorDisconnected => {
                stop_reason = Some("客户端已被管理员下线".into());
                tracing::warn!(device_id = identity.as_str(), "客户端已被管理员下线");
                break;
            }
            ConnectionEnd::Shutdown => break,
        }
    }
    status.send_replace(ClientStatus::Stopped {
        reason: stop_reason,
    });
    tracing::info!("QUIC 隧道客户端已停止");
    Ok(())
}

async fn wait_for_initial_config(
    config_path: &Path,
    watcher: &mut ConfigWatcher,
    shutdown: &CancellationToken,
    status: &watch::Sender<ClientStatus>,
) -> Option<ClientConfig> {
    loop {
        match ClientConfig::load(config_path) {
            Ok(config) => return Some(config),
            Err(error) => {
                status.send_replace(ClientStatus::Unconfigured {
                    reason: error.to_string(),
                });
            }
        }

        // 初始配置可能由桌面界面稍后补全，等待文件变化而不是快速重试。
        tokio::select! {
            () = shutdown.cancelled() => return None,
            changed = watcher.changed() => {
                if !changed {
                    return None;
                }
            }
        }
    }
}

enum ConnectionEnd {
    Reconfigure(ClientConfig),
    Unconfigured,
    Disconnected(TunnelError),
    DeviceIdConflict,
    AdministratorDisconnected,
    Shutdown,
}

enum ConnectionStep<T> {
    Completed(T),
    Reconfigure(ClientConfig),
    Unconfigured,
    Shutdown,
    WatcherClosed,
}

async fn connect_and_run(
    config: &mut ClientConfig,
    identity: &DeviceIdentity,
    config_path: &Path,
    watcher: &mut ConfigWatcher,
    shutdown: &CancellationToken,
    status: &watch::Sender<ClientStatus>,
    commands: Option<&mut ClientCommandReceiver>,
) -> ConnectionEnd {
    let managed = commands.is_some();
    let (endpoint, connection) = match wait_for_connection_step(
        connect_server(config),
        config_path,
        watcher,
        shutdown,
    )
    .await
    {
        ConnectionStep::Completed(Ok(connected)) => connected,
        ConnectionStep::Completed(Err(error)) => return ConnectionEnd::Disconnected(error),
        ConnectionStep::Reconfigure(updated) => {
            return ConnectionEnd::Reconfigure(reconcile_config_update(config, updated, managed));
        }
        ConnectionStep::Unconfigured => return ConnectionEnd::Unconfigured,
        ConnectionStep::Shutdown => return ConnectionEnd::Shutdown,
        ConnectionStep::WatcherClosed => return config_watcher_closed(),
    };
    let authentication = match wait_for_connection_step(
        authenticate(&connection, config, identity.as_str()),
        config_path,
        watcher,
        shutdown,
    )
    .await
    {
        ConnectionStep::Completed(result) => result,
        ConnectionStep::Reconfigure(updated) => {
            endpoint.close(
                ApplicationCloseCode::ClientReconfigure.value(),
                b"client configuration changed",
            );
            endpoint.wait_idle().await;
            return ConnectionEnd::Reconfigure(reconcile_config_update(config, updated, managed));
        }
        ConnectionStep::Unconfigured => {
            endpoint.close(
                ApplicationCloseCode::ClientReconfigure.value(),
                b"client configuration removed",
            );
            endpoint.wait_idle().await;
            return ConnectionEnd::Unconfigured;
        }
        ConnectionStep::Shutdown => {
            endpoint.close(
                ApplicationCloseCode::ClientShutdown.value(),
                b"client shutting down",
            );
            endpoint.wait_idle().await;
            return ConnectionEnd::Shutdown;
        }
        ConnectionStep::WatcherClosed => {
            endpoint.close(
                ApplicationCloseCode::ClientConnectionError.value(),
                b"configuration watcher closed",
            );
            endpoint.wait_idle().await;
            return config_watcher_closed();
        }
    };
    let (mut control_send, mut control_receive, tunnels) = match authentication {
        Ok(AuthenticationResult::Accepted {
            send,
            receive,
            tunnels,
        }) => (send, receive, tunnels),
        Ok(AuthenticationResult::DeviceIdConflict) => {
            endpoint.close(
                ApplicationCloseCode::ClientShutdown.value(),
                b"device id conflict",
            );
            endpoint.wait_idle().await;
            return ConnectionEnd::DeviceIdConflict;
        }
        Err(error) => {
            endpoint.close(
                ApplicationCloseCode::ClientConnectionError.value(),
                b"authentication failed",
            );
            return ConnectionEnd::Disconnected(error);
        }
    };

    tracing::info!(
        server = %config.server.address,
        device_id = identity.as_str(),
        "客户端控制会话已建立"
    );
    status.send_replace(ClientStatus::Online {
        server: config.server.address.clone(),
        device_id: identity.as_str().into(),
        tunnels,
    });
    let services = Arc::new(RwLock::new(service_map(&config.services)));
    let result = run_connected(
        &connection,
        config,
        services,
        ConnectedContext {
            config_path,
            watcher,
            control_send: &mut control_send,
            control_receive: &mut control_receive,
            status,
            device_id: identity.as_str(),
            shutdown,
            commands,
            managed,
        },
    )
    .await;
    let (close, reason) = match &result {
        ConnectionEnd::Reconfigure(_) | ConnectionEnd::Unconfigured => (
            ApplicationCloseCode::ClientReconfigure,
            b"client configuration changed".as_slice(),
        ),
        ConnectionEnd::Shutdown | ConnectionEnd::AdministratorDisconnected => (
            ApplicationCloseCode::ClientShutdown,
            b"client shutting down".as_slice(),
        ),
        ConnectionEnd::Disconnected(_) => (
            ApplicationCloseCode::ClientConnectionError,
            b"client connection failed".as_slice(),
        ),
        ConnectionEnd::DeviceIdConflict => (
            ApplicationCloseCode::ClientShutdown,
            b"device id conflict".as_slice(),
        ),
    };
    endpoint.close(close.value(), reason);
    endpoint.wait_idle().await;
    result
}

async fn wait_for_connection_step<T>(
    future: impl Future<Output = T>,
    config_path: &Path,
    watcher: &mut ConfigWatcher,
    shutdown: &CancellationToken,
) -> ConnectionStep<T> {
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => ConnectionStep::Completed(result),
        () = shutdown.cancelled() => ConnectionStep::Shutdown,
        changed = watcher.changed() => {
            if !changed {
                return ConnectionStep::WatcherClosed;
            }
            match load_client_config_after_change(config_path).await {
                Ok(updated) => ConnectionStep::Reconfigure(updated),
                Err(_) => ConnectionStep::Unconfigured,
            }
        }
    }
}

fn config_watcher_closed() -> ConnectionEnd {
    ConnectionEnd::Disconnected(TunnelError::Protocol("配置监听器已关闭".into()))
}

async fn load_client_config_after_change(config_path: &Path) -> Result<ClientConfig> {
    let deadline = tokio::time::Instant::now() + CONFIG_RELOAD_GRACE;
    loop {
        match ClientConfig::load(config_path) {
            Ok(config) => return Ok(config),
            Err(_) if tokio::time::Instant::now() < deadline => {
                // Windows 替换配置时会短暂删除旧文件，此时读取失败不代表配置已移除。
                tokio::time::sleep(CONFIG_RELOAD_RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }
}

enum AuthenticationResult {
    Accepted {
        send: quinn::SendStream,
        receive: quinn::RecvStream,
        tunnels: Vec<ClientTunnel>,
    },
    DeviceIdConflict,
}

async fn authenticate(
    connection: &Connection,
    config: &ClientConfig,
    device_id: &str,
) -> Result<AuthenticationResult> {
    let (mut send, mut receive) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.open_bi())
        .await
        .map_err(|_| TunnelError::Timeout("打开认证流"))??;
    let mut handshake = ClientHandshake::Authenticate(ClientHello {
        version: PROTOCOL_VERSION,
        device_id: device_id.into(),
        key: config.key.as_bytes().to_vec(),
        services: declarations(&config.services),
    });
    let write_result = write_frame(&mut send, &handshake).await;
    if let ClientHandshake::Authenticate(hello) = &mut handshake {
        hello.key.zeroize();
    }
    write_result.map_err(|error| connection_error_or(connection, error))?;
    let response: ServerHandshake =
        tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut receive))
            .await
            .map_err(|_| TunnelError::Timeout("等待认证结果"))?
            .map_err(|error| connection_error_or(connection, error))?;
    let ServerHandshake::Authenticate(response) = response else {
        return Err(TunnelError::Protocol(
            "服务端返回了错误的认证响应类型".into(),
        ));
    };
    match response.status {
        AuthenticationStatus::Accepted => Ok(AuthenticationResult::Accepted {
            send,
            receive,
            tunnels: response.tunnels,
        }),
        AuthenticationStatus::DeviceIdConflict => Ok(AuthenticationResult::DeviceIdConflict),
        AuthenticationStatus::Rejected | AuthenticationStatus::ServerBusy => {
            Err(TunnelError::Authentication(response.message))
        }
    }
}

struct ConnectedContext<'a> {
    config_path: &'a Path,
    watcher: &'a mut ConfigWatcher,
    control_send: &'a mut quinn::SendStream,
    control_receive: &'a mut quinn::RecvStream,
    status: &'a watch::Sender<ClientStatus>,
    device_id: &'a str,
    shutdown: &'a CancellationToken,
    commands: Option<&'a mut ClientCommandReceiver>,
    managed: bool,
}

struct PendingServiceUpdate {
    request_id: u64,
    updated: ClientConfig,
    response: Option<CommandResponse>,
    deadline: tokio::time::Instant,
}

async fn run_connected(
    connection: &Connection,
    config: &mut ClientConfig,
    services: Arc<RwLock<HashMap<String, ClientServiceConfig>>>,
    context: ConnectedContext<'_>,
) -> ConnectionEnd {
    let ConnectedContext {
        config_path,
        watcher,
        control_send,
        control_receive,
        status,
        device_id,
        shutdown,
        mut commands,
        managed,
    } = context;
    let mut tasks = JoinSet::new();
    let mut pending_update: Option<PendingServiceUpdate> = None;
    let mut next_request_id = 1_u64;
    let end = loop {
        let update_deadline = pending_update.as_ref().map(|update| update.deadline);
        tokio::select! {
            () = shutdown.cancelled() => break ConnectionEnd::Shutdown,
            () = wait_for_deadline(update_deadline), if update_deadline.is_some() => {
                if let Some(pending) = pending_update.take()
                    && let Some(response) = pending.response
                {
                    let _ = response.send(Err("等待服务端确认服务配置超时".into()));
                }
                break ConnectionEnd::Disconnected(TunnelError::Timeout("等待服务端确认服务配置"));
            }
            error = connection.closed() => {
                if is_administrator_disconnect(&error) {
                    break ConnectionEnd::AdministratorDisconnected;
                }
                break ConnectionEnd::Disconnected(error.into());
            },
            message = read_frame::<_, ServerControlMessage>(control_receive) => {
                match message {
                    Ok(ServerControlMessage::TunnelSnapshot(tunnels)) => {
                        status.send_replace(ClientStatus::Online {
                            server: config.server.address.clone(),
                            device_id: device_id.into(),
                            tunnels,
                        });
                    }
                    Ok(ServerControlMessage::ServicesApplied { request_id, tunnels }) => {
                        let Some(pending) = pending_update.take() else {
                            break ConnectionEnd::Disconnected(TunnelError::Protocol(
                                "收到没有对应请求的服务应用回执".into(),
                            ));
                        };
                        if pending.request_id != request_id {
                            break ConnectionEnd::Disconnected(TunnelError::Protocol(
                                "服务应用回执编号不匹配".into(),
                            ));
                        }
                        *config = pending.updated;
                        status.send_replace(ClientStatus::Online {
                            server: config.server.address.clone(),
                            device_id: device_id.into(),
                            tunnels: tunnels.clone(),
                        });
                        if let Some(response) = pending.response {
                            let _ = response.send(Ok(tunnels));
                        }
                    }
                    Err(error) => {
                        break ConnectionEnd::Disconnected(connection_error_or(connection, error));
                    }
                }
            }
            changed = watcher.changed(), if pending_update.is_none() => {
                if !changed {
                    break ConnectionEnd::Disconnected(TunnelError::Protocol("配置监听器已关闭".into()));
                }
                let Ok(updated) = load_client_config_after_change(config_path).await else {
                    break ConnectionEnd::Unconfigured;
                };
                if connection_identity_changed(config, &updated) {
                    break ConnectionEnd::Reconfigure(reconcile_config_update(config, updated, managed));
                }
                if managed {
                    continue;
                }
                let request_id = take_request_id(&mut next_request_id);
                let update = PendingServiceUpdate {
                    request_id,
                    updated,
                    response: None,
                    deadline: tokio::time::Instant::now() + SERVICE_UPDATE_TIMEOUT,
                };
                match begin_service_update(
                    connection,
                    control_send,
                    &services,
                    config,
                    update,
                ).await {
                    Ok(pending) => pending_update = Some(pending),
                    Err(error) => break ConnectionEnd::Disconnected(error),
                }
            }
            command = receive_client_command(&mut commands), if pending_update.is_none() => {
                let Some(command) = command else {
                    commands = None;
                    continue;
                };
                if command.response.is_closed() {
                    continue;
                }
                let mut updated = config.clone();
                updated.services = command.services;
                let request_id = take_request_id(&mut next_request_id);
                let update = PendingServiceUpdate {
                    request_id,
                    updated,
                    response: Some(command.response),
                    deadline: command.deadline,
                };
                match begin_service_update(
                    connection,
                    control_send,
                    &services,
                    config,
                    update,
                ).await {
                    Ok(pending) => pending_update = Some(pending),
                    Err(error) => break ConnectionEnd::Disconnected(error),
                }
            }
            stream = connection.accept_bi() => match stream {
                Ok((send, receive)) => {
                    let services = Arc::clone(&services);
                    tasks.spawn(async move {
                        if let Err(error) = handle_stream(send, receive, services).await {
                            tracing::debug!(%error, "QUIC 数据流结束");
                        }
                    });
                }
                Err(error) => break ConnectionEnd::Disconnected(error.into()),
            },
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "客户端转发任务异常结束");
                }
            }
        }
    };
    tasks.shutdown().await;
    end
}

async fn receive_client_command(
    commands: &mut Option<&mut ClientCommandReceiver>,
) -> Option<ClientCommand> {
    match commands {
        Some(commands) => commands.receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn wait_for_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn begin_service_update(
    connection: &Connection,
    control_send: &mut quinn::SendStream,
    services: &RwLock<HashMap<String, ClientServiceConfig>>,
    current: &ClientConfig,
    update: PendingServiceUpdate,
) -> Result<PendingServiceUpdate> {
    *services.write().await = service_map(&update.updated.services);
    let message = ControlMessage::UpdateServices {
        request_id: update.request_id,
        services: declarations(&update.updated.services),
    };
    if let Err(error) = write_frame(control_send, &message).await {
        *services.write().await = service_map(&current.services);
        let error = connection_error_or(connection, error);
        if let Some(response) = update.response {
            let _ = response.send(Err(error.to_string()));
        }
        return Err(error);
    }
    Ok(update)
}

fn take_request_id(next: &mut u64) -> u64 {
    let request_id = *next;
    *next = next.wrapping_add(1).max(1);
    request_id
}

fn reconcile_config_update(
    current: &ClientConfig,
    mut updated: ClientConfig,
    managed: bool,
) -> ClientConfig {
    if managed {
        if connection_identity_changed(current, &updated) {
            updated.services.clear();
        } else {
            updated.services.clone_from(&current.services);
        }
    }
    updated
}

async fn handle_stream(
    mut send: quinn::SendStream,
    mut receive: quinn::RecvStream,
    services: Arc<RwLock<HashMap<String, ClientServiceConfig>>>,
) -> Result<()> {
    let request: OpenRequest = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut receive))
        .await
        .map_err(|_| TunnelError::Timeout("读取数据流请求"))??;
    let service = services.read().await.get(&request.service).cloned();
    let Some(service) = service else {
        write_rejection(&mut send, "服务不存在").await?;
        return Ok(());
    };
    match service.kind {
        TunnelKind::Tcp => {
            let target = service
                .target
                .as_deref()
                .ok_or_else(|| TunnelError::InvalidConfig("TCP 服务缺少 target".into()))?;
            handle_tcp(send, receive, target).await
        }
        TunnelKind::Socks5 => {
            let destination = request
                .destination
                .as_deref()
                .ok_or_else(|| TunnelError::Protocol("SOCKS5 请求缺少目标地址".into()))?;
            handle_tcp(send, receive, destination).await
        }
        TunnelKind::Udp => {
            let target = service
                .target
                .as_deref()
                .ok_or_else(|| TunnelError::InvalidConfig("UDP 服务缺少 target".into()))?;
            handle_udp(send, receive, target).await
        }
    }
}

async fn handle_tcp(
    mut send: quinn::SendStream,
    receive: quinn::RecvStream,
    target: &str,
) -> Result<()> {
    let mut stream = match tokio::time::timeout(HANDSHAKE_TIMEOUT, TcpStream::connect(target)).await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            tracing::debug!(%target, %error, "连接内网 TCP 目标失败");
            write_rejection(&mut send, "目标连接失败").await?;
            return Ok(());
        }
        Err(_) => {
            write_rejection(&mut send, "目标连接超时").await?;
            return Ok(());
        }
    };
    write_frame(
        &mut send,
        &OpenResponse {
            accepted: true,
            message: String::new(),
        },
    )
    .await?;
    relay::copy_bidirectional(
        &mut stream,
        &mut QuinnStream(send, receive),
        &RateLimiter::new(None),
    )
    .await
}

async fn handle_udp(
    mut send: quinn::SendStream,
    mut receive: quinn::RecvStream,
    target: &str,
) -> Result<()> {
    let target_address = resolve_one(target).await?;
    let bind: SocketAddr = if target_address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    }
    .parse()?;
    let socket = UdpSocket::bind(bind).await?;
    if let Err(error) = socket.connect(target_address).await {
        tracing::debug!(%target, %error, "连接内网 UDP 目标失败");
        write_rejection(&mut send, "UDP 目标连接失败").await?;
        return Ok(());
    }
    write_frame(
        &mut send,
        &OpenResponse {
            accepted: true,
            message: String::new(),
        },
    )
    .await?;

    let mut quic_buffer = Vec::new();
    let mut udp_buffer = vec![0; MAX_DATAGRAM];
    loop {
        tokio::select! {
            packet = read_datagram(&mut receive, &mut quic_buffer) => {
                let length = packet?;
                socket.send(&quic_buffer[..length]).await?;
            }
            packet = socket.recv(&mut udp_buffer) => {
                let length = packet?;
                write_datagram(&mut send, &udp_buffer[..length]).await?;
            }
        }
    }
}

async fn write_rejection(send: &mut quinn::SendStream, message: &str) -> Result<()> {
    write_frame(
        send,
        &OpenResponse {
            accepted: false,
            message: message.into(),
        },
    )
    .await
}

async fn bootstrap_at_address(
    server_address: SocketAddr,
    client_config: quinn::ClientConfig,
    server_name: &str,
    configured_address: &str,
    key: &str,
) -> Result<DownloadedServerCertificate> {
    let endpoint = tls::client_endpoint(server_address, client_config)?;
    let connection = endpoint.connect(server_address, server_name)?.await?;
    let certificates = peer_certificates(&connection)?;
    let certificate = certificates
        .first()
        .ok_or_else(|| TunnelError::Tls("服务端未提供叶证书".into()))?;
    let client_nonce = bootstrap::random_nonce();
    let proof = bootstrap::client_proof(
        key.as_bytes(),
        PROTOCOL_VERSION,
        &client_nonce,
        certificate.as_ref(),
    )?;
    let (mut send, mut receive) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.open_bi())
        .await
        .map_err(|_| TunnelError::Timeout("打开证书引导流"))??;
    write_frame(
        &mut send,
        &ClientHandshake::Bootstrap(CertificateBootstrapRequest {
            version: PROTOCOL_VERSION,
            client_nonce,
            proof,
        }),
    )
    .await
    .map_err(|error| connection_error_or(&connection, error))?;
    let response: ServerHandshake =
        tokio::time::timeout(BOOTSTRAP_RESPONSE_TIMEOUT, read_frame(&mut receive))
            .await
            .map_err(|_| TunnelError::Timeout("等待证书引导结果"))?
            .map_err(|error| connection_error_or(&connection, error))?;
    let ServerHandshake::Bootstrap(response) = response else {
        return Err(TunnelError::Protocol(
            "服务端返回了错误的证书引导响应类型".into(),
        ));
    };
    match response {
        CertificateBootstrapResponse::Accepted {
            server_nonce,
            proof,
        } if bootstrap::verify_server_proof(
            key.as_bytes(),
            PROTOCOL_VERSION,
            &client_nonce,
            &server_nonce,
            certificate.as_ref(),
            &proof,
        ) => {}
        CertificateBootstrapResponse::Accepted { .. } => {
            return Err(TunnelError::Authentication("服务端证书密钥证明无效".into()));
        }
        CertificateBootstrapResponse::Rejected { message } => {
            return Err(TunnelError::Authentication(message));
        }
    }
    let downloaded = DownloadedServerCertificate::from_chain(&certificates, configured_address)?;
    connection.close(
        ApplicationCloseCode::CredentialCheckComplete.value(),
        b"certificate received",
    );
    endpoint.wait_idle().await;
    Ok(downloaded)
}

fn peer_certificates(connection: &Connection) -> Result<Vec<CertificateDer<'static>>> {
    let identity = connection
        .peer_identity()
        .ok_or_else(|| TunnelError::Tls("QUIC 连接缺少服务端证书身份".into()))?;
    identity
        .downcast::<Vec<CertificateDer<'static>>>()
        .map(|certificates| *certificates)
        .map_err(|_| TunnelError::Tls("无法读取 QUIC 服务端证书链".into()))
}

async fn connect_server(config: &ClientConfig) -> Result<(quinn::Endpoint, Connection)> {
    let addresses = resolve_addresses(&config.server.address).await?;
    let server_name = config.server_name()?.to_owned();
    let client_config = tls::client_config(config.server.ca_certificate.as_deref())?;
    try_addresses(addresses, |server_address| {
        connect_at_address(server_address, server_name.clone(), client_config.clone())
    })
    .await
}

async fn connect_at_address(
    server_address: SocketAddr,
    server_name: String,
    client_config: quinn::ClientConfig,
) -> Result<(quinn::Endpoint, Connection)> {
    let endpoint = tls::client_endpoint(server_address, client_config)?;
    let connection = endpoint.connect(server_address, &server_name)?.await?;
    Ok((endpoint, connection))
}

async fn resolve_addresses(target: &str) -> Result<Vec<SocketAddr>> {
    let mut unique = HashSet::new();
    let addresses = tokio::net::lookup_host(target)
        .await?
        .filter(|address| unique.insert(*address))
        .take(MAX_RESOLVED_ADDRESSES)
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        Err(TunnelError::InvalidConfig(format!(
            "目标地址无法解析: {target}"
        )))
    } else {
        Ok(addresses)
    }
}

async fn resolve_one(target: &str) -> Result<SocketAddr> {
    resolve_addresses(target)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| TunnelError::InvalidConfig(format!("目标地址无法解析: {target}")))
}

async fn try_addresses<T, F, Fut>(addresses: Vec<SocketAddr>, operation: F) -> Result<T>
where
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempts = addresses
        .into_iter()
        .map(operation)
        .collect::<FuturesUnordered<_>>();
    let mut last_error = None;
    while let Some(result) = attempts.next().await {
        match result {
            Ok(value) => return Ok(value),
            Err(error @ TunnelError::Authentication(_)) => return Err(error),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| TunnelError::InvalidConfig("没有可用的服务器地址".into())))
}

fn declarations(services: &[ClientServiceConfig]) -> Vec<ServiceDeclaration> {
    services
        .iter()
        .map(|service| ServiceDeclaration {
            name: service.name.clone(),
            kind: service.kind,
        })
        .collect()
}

fn service_map(services: &[ClientServiceConfig]) -> HashMap<String, ClientServiceConfig> {
    services
        .iter()
        .map(|service| (service.name.clone(), service.clone()))
        .collect()
}

fn connection_identity_changed(current: &ClientConfig, updated: &ClientConfig) -> bool {
    current.server != updated.server || current.key != updated.key
}

fn is_administrator_disconnect(error: &quinn::ConnectionError) -> bool {
    ApplicationCloseCode::AdministratorDisconnect.matches_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_ipv6_local_target() {
        let tunnel = ClientTunnel {
            name: "web".into(),
            kind: TunnelKind::Tcp,
            server_port: 443,
            local_ip: Some("::1".into()),
            local_port: Some(8080),
            state: ClientTunnelState::Idle,
        };

        assert_eq!(tunnel.local_target().as_deref(), Some("[::1]:8080"));
    }

    #[tokio::test]
    async fn invalid_initial_config_waits_for_update() {
        let directory = tempfile::tempdir().expect("创建临时目录");
        let path = directory.path().join("client.toml");
        ClientConfig::ensure_exists(&path).expect("创建初始客户端配置");
        let cancellation = CancellationToken::new();
        let (status_sender, mut status) = watch::channel(ClientStatus::Starting);
        let task_path = path.clone();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            run_client_with_status(task_path, task_cancellation, status_sender).await
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                status.changed().await.expect("状态通道保持打开");
                if matches!(&*status.borrow(), ClientStatus::Unconfigured { .. }) {
                    break;
                }
            }
        })
        .await
        .expect("初始配置无效时应等待更新");

        let mut config = ClientConfig::read(&path).expect("读取初始配置");
        config.server.address = "127.0.0.1:9".into();
        config.server.name = Some("localhost".into());
        config.save(&path).expect("保存有效配置");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                status.changed().await.expect("状态通道保持打开");
                if matches!(
                    &*status.borrow(),
                    ClientStatus::Connecting { .. } | ClientStatus::Reconnecting { .. }
                ) {
                    break;
                }
            }
        })
        .await
        .expect("配置更新后应开始连接");

        cancellation.cancel();
        task.await
            .expect("客户端任务正常结束")
            .expect("客户端运行成功");
    }

    #[tokio::test]
    async fn configuration_change_interrupts_connection_step() {
        let directory = tempfile::tempdir().expect("创建临时目录");
        let path = directory.path().join("client.toml");
        ClientConfig::ensure_exists(&path).expect("创建初始客户端配置");
        let mut watcher = ConfigWatcher::new(&path).expect("创建配置监听器");
        let update_path = path.clone();
        let mut updated = ClientConfig::read(&path).expect("读取初始客户端配置");
        let update = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            std::fs::remove_file(&update_path).expect("删除旧客户端配置");
            tokio::time::sleep(Duration::from_millis(25)).await;
            updated.server.address = "127.0.0.1:24444".into();
            updated.save(&update_path).expect("保存更新后的客户端配置");
        });
        let cancellation = CancellationToken::new();

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_connection_step(
                std::future::pending::<()>(),
                &path,
                &mut watcher,
                &cancellation,
            ),
        )
        .await
        .expect("配置变化应及时打断连接步骤");
        update.await.expect("配置更新任务应正常完成");
        match result {
            ConnectionStep::Reconfigure(config) => {
                assert_eq!(config.server.address, "127.0.0.1:24444");
            }
            ConnectionStep::Completed(())
            | ConnectionStep::Unconfigured
            | ConnectionStep::Shutdown
            | ConnectionStep::WatcherClosed => panic!("连接步骤返回了非预期状态"),
        }
    }
}
