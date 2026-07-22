use std::{
    collections::HashMap, future::Future, net::SocketAddr, path::Path, sync::Arc, time::Duration,
};

use quinn::{Connection, VarInt};
use serde::{Deserialize, Serialize};
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::{RwLock, watch},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Result, TunnelError,
    config::{ClientConfig, ClientServiceConfig, TunnelKind},
    identity::DeviceIdentity,
    protocol::{
        AuthenticationStatus, ClientHello, ControlMessage, HANDSHAKE_TIMEOUT, MAX_DATAGRAM,
        OpenRequest, OpenResponse, PROTOCOL_VERSION, ServerControlMessage, ServerHello,
        ServiceDeclaration, read_datagram, read_frame, write_datagram, write_frame,
    },
    rate_limit::RateLimiter,
    relay::{self, QuinnStream},
    runtime::ADMINISTRATOR_CLOSE_CODE,
    tls,
    watcher::ConfigWatcher,
};

const CLOSE_RECONFIGURE: VarInt = VarInt::from_u32(10);
const CLOSE_SHUTDOWN: VarInt = VarInt::from_u32(11);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientTunnelState {
    Idle,
    Connected,
    Occupied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientTunnel {
    pub name: String,
    pub kind: TunnelKind,
    pub server_port: u16,
    pub local_port: Option<u16>,
    pub state: ClientTunnelState,
}

/// 客户端连接状态，供本机管理界面展示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientStatus {
    Starting,
    Unconfigured {
        reason: String,
    },
    Connecting {
        server: String,
    },
    Connected {
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
    let config_path = config_path.as_ref().to_owned();
    let mut watcher = ConfigWatcher::new(&config_path)?;
    let Some(mut config) =
        wait_for_initial_config(&config_path, &mut watcher, &shutdown, &status).await
    else {
        status.send_replace(ClientStatus::Stopped { reason: None });
        return Ok(());
    };
    let mut identity = DeviceIdentity::load(&config_path)?;
    let mut retry = Duration::from_secs(1);
    let mut stop_reason = None;

    while !shutdown.is_cancelled() {
        status.send_replace(ClientStatus::Connecting {
            server: config.server.address.clone(),
        });
        match connect_and_run(
            config.clone(),
            &identity,
            &config_path,
            &mut watcher,
            &shutdown,
            &status,
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
    Disconnected(TunnelError),
    DeviceIdConflict,
    AdministratorDisconnected,
    Shutdown,
}

enum ConnectionStep<T> {
    Completed(T),
    Reconfigure(ClientConfig),
    Shutdown,
    WatcherClosed,
}

async fn connect_and_run(
    mut config: ClientConfig,
    identity: &DeviceIdentity,
    config_path: &Path,
    watcher: &mut ConfigWatcher,
    shutdown: &CancellationToken,
    status: &watch::Sender<ClientStatus>,
) -> ConnectionEnd {
    let server_address = match wait_for_connection_step(
        resolve_one(&config.server.address),
        config_path,
        watcher,
        shutdown,
    )
    .await
    {
        ConnectionStep::Completed(Ok(address)) => address,
        ConnectionStep::Completed(Err(error)) => return ConnectionEnd::Disconnected(error),
        ConnectionStep::Reconfigure(updated) => return ConnectionEnd::Reconfigure(updated),
        ConnectionStep::Shutdown => return ConnectionEnd::Shutdown,
        ConnectionStep::WatcherClosed => return config_watcher_closed(),
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
    let connection = match wait_for_connection_step(connecting, config_path, watcher, shutdown)
        .await
    {
        ConnectionStep::Completed(Ok(connection)) => connection,
        ConnectionStep::Completed(Err(error)) => return ConnectionEnd::Disconnected(error.into()),
        ConnectionStep::Reconfigure(updated) => return ConnectionEnd::Reconfigure(updated),
        ConnectionStep::Shutdown => return ConnectionEnd::Shutdown,
        ConnectionStep::WatcherClosed => return config_watcher_closed(),
    };
    let authentication = match wait_for_connection_step(
        authenticate(&connection, &config, identity.as_str()),
        config_path,
        watcher,
        shutdown,
    )
    .await
    {
        ConnectionStep::Completed(result) => result,
        ConnectionStep::Reconfigure(updated) => {
            endpoint.close(CLOSE_RECONFIGURE, b"client configuration changed");
            endpoint.wait_idle().await;
            return ConnectionEnd::Reconfigure(updated);
        }
        ConnectionStep::Shutdown => {
            endpoint.close(CLOSE_SHUTDOWN, b"client shutting down");
            endpoint.wait_idle().await;
            return ConnectionEnd::Shutdown;
        }
        ConnectionStep::WatcherClosed => {
            endpoint.close(CLOSE_SHUTDOWN, b"configuration watcher closed");
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
    status.send_replace(ClientStatus::Connected {
        server: config.server.address.clone(),
        device_id: identity.as_str().into(),
        tunnels,
    });
    let services = Arc::new(RwLock::new(service_map(&config.services)));
    let result = run_connected(
        &connection,
        &mut config,
        services,
        ConnectedContext {
            config_path,
            watcher,
            control_send: &mut control_send,
            control_receive: &mut control_receive,
            status,
            device_id: identity.as_str(),
            shutdown,
        },
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

async fn wait_for_connection_step<T>(
    future: impl Future<Output = T>,
    config_path: &Path,
    watcher: &mut ConfigWatcher,
    shutdown: &CancellationToken,
) -> ConnectionStep<T> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return ConnectionStep::Completed(result),
            () = shutdown.cancelled() => return ConnectionStep::Shutdown,
            changed = watcher.changed() => {
                if !changed {
                    return ConnectionStep::WatcherClosed;
                }
                match ClientConfig::load(config_path) {
                    Ok(updated) => return ConnectionStep::Reconfigure(updated),
                    Err(error) => {
                        tracing::error!(%error, "新客户端配置无效，继续使用当前配置");
                    }
                }
            }
        }
    }
}

fn config_watcher_closed() -> ConnectionEnd {
    ConnectionEnd::Disconnected(TunnelError::Protocol("配置监听器已关闭".into()))
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
        AuthenticationStatus::Accepted => Ok(AuthenticationResult::Accepted {
            send,
            receive,
            tunnels: response.tunnels,
        }),
        AuthenticationStatus::DeviceIdConflict => Ok(AuthenticationResult::DeviceIdConflict),
        AuthenticationStatus::Rejected | AuthenticationStatus::ServerBusy => {
            Err(TunnelError::Protocol(response.message))
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
    } = context;
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
            message = read_frame::<_, ServerControlMessage>(control_receive) => {
                match message {
                    Ok(ServerControlMessage::TunnelSnapshot(tunnels)) => {
                        status.send_replace(ClientStatus::Connected {
                            server: config.server.address.clone(),
                            device_id: device_id.into(),
                            tunnels,
                        });
                    }
                    Err(error) => break ConnectionEnd::Disconnected(error),
                }
            }
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
                let services_changed = config.services != updated.services;
                let declarations = declarations(&updated.services);
                if let Err(error) = write_frame(
                    control_send,
                    &ControlMessage::UpdateServices(declarations),
                )
                .await
                {
                    break ConnectionEnd::Disconnected(error);
                }
                if services_changed {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let update = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let mut config = ClientConfig::read(&update_path).expect("读取初始客户端配置");
            config.server.address = "127.0.0.1:24444".into();
            config.save(&update_path).expect("保存更新后的客户端配置");
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
            | ConnectionStep::Shutdown
            | ConnectionStep::WatcherClosed => panic!("连接步骤返回了非预期状态"),
        }
    }
}
