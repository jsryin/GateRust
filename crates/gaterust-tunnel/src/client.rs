use std::{collections::HashMap, net::SocketAddr, path::Path, sync::Arc, time::Duration};

use quinn::{Connection, VarInt};
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::RwLock,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Result, TunnelError,
    config::{ClientConfig, ClientServiceConfig, TunnelKind},
    identity::DeviceIdentity,
    protocol::{
        AuthenticationStatus, ClientHello, ControlMessage, HANDSHAKE_TIMEOUT, MAX_DATAGRAM,
        OpenRequest, OpenResponse, PROTOCOL_VERSION, ServerHello, ServiceDeclaration,
        read_datagram, read_frame, write_datagram, write_frame,
    },
    rate_limit::RateLimiter,
    relay::{self, QuinnStream},
    runtime::ADMINISTRATOR_CLOSE_CODE,
    tls,
    watcher::ConfigWatcher,
};

const CLOSE_RECONFIGURE: VarInt = VarInt::from_u32(10);
const CLOSE_SHUTDOWN: VarInt = VarInt::from_u32(11);

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
    let config_path = config_path.as_ref().to_owned();
    let mut config = ClientConfig::load(&config_path)?;
    let mut identity = DeviceIdentity::load(&config_path)?;
    let mut watcher = ConfigWatcher::new(&config_path)?;
    let mut retry = Duration::from_secs(1);

    while !shutdown.is_cancelled() {
        match connect_and_run(
            config.clone(),
            &identity,
            &config_path,
            &mut watcher,
            &shutdown,
        )
        .await
        {
            ConnectionEnd::Reconfigure(updated) => {
                config = updated;
                retry = Duration::from_secs(1);
            }
            ConnectionEnd::Disconnected(error) => {
                tracing::warn!(%error, delay_seconds = retry.as_secs(), "隧道连接断开，稍后重试");
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    () = tokio::time::sleep(retry) => {}
                    changed = watcher.changed() => {
                        if changed {
                            match ClientConfig::load(&config_path) {
                                Ok(updated) => config = updated,
                                Err(error) => tracing::error!(%error, "新客户端配置无效，继续使用当前配置"),
                            }
                        }
                    }
                }
                retry = (retry * 2).min(Duration::from_secs(30));
            }
            ConnectionEnd::DeviceIdConflict => {
                identity.resolve_conflict()?;
                tracing::info!(device_id = identity.as_str(), "设备 ID 冲突，已生成新 ID");
                retry = Duration::from_secs(1);
            }
            ConnectionEnd::AdministratorDisconnected => {
                tracing::warn!(device_id = identity.as_str(), "客户端已被管理员下线");
                break;
            }
            ConnectionEnd::Shutdown => break,
        }
    }
    tracing::info!("QUIC 隧道客户端已停止");
    Ok(())
}

enum ConnectionEnd {
    Reconfigure(ClientConfig),
    Disconnected(TunnelError),
    DeviceIdConflict,
    AdministratorDisconnected,
    Shutdown,
}

async fn connect_and_run(
    mut config: ClientConfig,
    identity: &DeviceIdentity,
    config_path: &Path,
    watcher: &mut ConfigWatcher,
    shutdown: &CancellationToken,
) -> ConnectionEnd {
    let server_address = match resolve_one(&config.server.address).await {
        Ok(address) => address,
        Err(error) => return ConnectionEnd::Disconnected(error),
    };
    let endpoint =
        match tls::client_endpoint(server_address, config.server.ca_certificate.as_deref()) {
            Ok(endpoint) => endpoint,
            Err(error) => return ConnectionEnd::Disconnected(error),
        };
    let server_name = match config.server_name() {
        Ok(name) => name.to_owned(),
        Err(error) => return ConnectionEnd::Disconnected(error),
    };
    let connecting = match endpoint.connect(server_address, &server_name) {
        Ok(connecting) => connecting,
        Err(error) => return ConnectionEnd::Disconnected(error.into()),
    };
    let connection = tokio::select! {
        () = shutdown.cancelled() => return ConnectionEnd::Shutdown,
        result = connecting => match result {
            Ok(connection) => connection,
            Err(error) => return ConnectionEnd::Disconnected(error.into()),
        }
    };
    let mut control_send = match authenticate(&connection, &config, identity.as_str()).await {
        Ok(AuthenticationResult::Accepted(control_send)) => control_send,
        Ok(AuthenticationResult::DeviceIdConflict) => {
            endpoint.close(CLOSE_SHUTDOWN, b"device id conflict");
            endpoint.wait_idle().await;
            return ConnectionEnd::DeviceIdConflict;
        }
        Err(error) => {
            endpoint.close(CLOSE_SHUTDOWN, b"authentication failed");
            return ConnectionEnd::Disconnected(error);
        }
    };

    tracing::info!(
        server = %config.server.address,
        device_id = identity.as_str(),
        "已连接 QUIC 隧道服务端"
    );
    let services = Arc::new(RwLock::new(service_map(&config.services)));
    let result = run_connected(
        &connection,
        &mut config,
        config_path,
        watcher,
        services,
        &mut control_send,
        shutdown,
    )
    .await;
    let close = match &result {
        ConnectionEnd::Reconfigure(_) => CLOSE_RECONFIGURE,
        _ => CLOSE_SHUTDOWN,
    };
    endpoint.close(close, b"client connection ending");
    endpoint.wait_idle().await;
    result
}

enum AuthenticationResult {
    Accepted(quinn::SendStream),
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
    write_frame(
        &mut send,
        &ClientHello {
            version: PROTOCOL_VERSION,
            device_id: device_id.into(),
            key: config.key.as_bytes().to_vec(),
            services: declarations(&config.services),
        },
    )
    .await?;
    let response: ServerHello = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut receive))
        .await
        .map_err(|_| TunnelError::Timeout("等待认证结果"))??;
    match response.status {
        AuthenticationStatus::Accepted => Ok(AuthenticationResult::Accepted(send)),
        AuthenticationStatus::DeviceIdConflict => Ok(AuthenticationResult::DeviceIdConflict),
        AuthenticationStatus::Rejected | AuthenticationStatus::ServerBusy => {
            Err(TunnelError::Protocol(response.message))
        }
    }
}

async fn run_connected(
    connection: &Connection,
    config: &mut ClientConfig,
    config_path: &Path,
    watcher: &mut ConfigWatcher,
    services: Arc<RwLock<HashMap<String, ClientServiceConfig>>>,
    control_send: &mut quinn::SendStream,
    shutdown: &CancellationToken,
) -> ConnectionEnd {
    let mut tasks = JoinSet::new();
    let end = loop {
        tokio::select! {
            () = shutdown.cancelled() => break ConnectionEnd::Shutdown,
            error = connection.closed() => {
                if is_administrator_disconnect(&error) {
                    break ConnectionEnd::AdministratorDisconnected;
                }
                break ConnectionEnd::Disconnected(error.into());
            },
            changed = watcher.changed() => {
                if !changed {
                    break ConnectionEnd::Disconnected(TunnelError::Protocol("配置监听器已关闭".into()));
                }
                let updated = match ClientConfig::load(config_path) {
                    Ok(updated) => updated,
                    Err(error) => {
                        tracing::error!(%error, "新客户端配置无效，继续使用当前配置");
                        continue;
                    }
                };
                if connection_identity_changed(config, &updated) {
                    break ConnectionEnd::Reconfigure(updated);
                }
                if config.services != updated.services {
                    let declarations = declarations(&updated.services);
                    if let Err(error) = write_frame(
                        control_send,
                        &ControlMessage::UpdateServices(declarations),
                    )
                    .await
                    {
                        break ConnectionEnd::Disconnected(error);
                    }
                    *services.write().await = service_map(&updated.services);
                    tracing::info!(services = updated.services.len(), "客户端服务配置已热更新");
                }
                *config = updated;
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

async fn resolve_one(target: &str) -> Result<SocketAddr> {
    tokio::net::lookup_host(target)
        .await?
        .next()
        .ok_or_else(|| TunnelError::InvalidConfig(format!("目标地址无法解析: {target}")))
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
    matches!(
        error,
        quinn::ConnectionError::ApplicationClosed(close)
            if close.error_code == VarInt::from_u32(ADMINISTRATOR_CLOSE_CODE)
    )
}
