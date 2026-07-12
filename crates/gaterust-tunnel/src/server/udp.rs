use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Result, TunnelError,
    config::ServerTunnelConfig,
    protocol::{
        HANDSHAKE_TIMEOUT, MAX_DATAGRAM, OpenRequest, OpenResponse, read_datagram, read_frame,
        write_datagram, write_frame,
    },
    rate_limit::RateLimiter,
    runtime::TunnelRuntime,
};

const SESSION_QUEUE: usize = 64;

pub(super) async fn bind(config: &ServerTunnelConfig) -> Result<UdpSocket> {
    Ok(UdpSocket::bind(config.bind).await?)
}

pub(super) async fn run(
    socket: UdpSocket,
    config: ServerTunnelConfig,
    runtime: TunnelRuntime,
    cancellation: CancellationToken,
    stopped: oneshot::Sender<()>,
) {
    let socket = Arc::new(socket);
    let limiter = RateLimiter::new(config.limit_bps);
    let (cleanup_sender, mut cleanup_receiver) = mpsc::channel(config.max_udp_sessions);
    let mut sessions = HashMap::<SocketAddr, mpsc::Sender<Vec<u8>>>::new();
    let mut tasks = JoinSet::new();
    let mut buffer = vec![0; MAX_DATAGRAM];

    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            received = socket.recv_from(&mut buffer) => match received {
                Ok((length, peer)) => {
                    if let Some(sender) = sessions.get(&peer) {
                        if sender.try_send(buffer[..length].to_vec()).is_err() {
                            tracing::debug!(tunnel = %config.name, %peer, "UDP 会话队列已满，丢弃数据报");
                        }
                        continue;
                    }
                    if sessions.len() >= config.max_udp_sessions {
                        tracing::warn!(tunnel = %config.name, %peer, "UDP 会话数已满，丢弃数据报");
                        continue;
                    }
                    let (sender, receiver) = mpsc::channel(SESSION_QUEUE);
                    if sender.try_send(buffer[..length].to_vec()).is_err() {
                        continue;
                    }
                    sessions.insert(peer, sender);
                    let context = SessionContext {
                        socket: Arc::clone(&socket),
                        runtime: runtime.clone(),
                        limiter: limiter.clone(),
                        cleanup: cleanup_sender.clone(),
                        config: config.clone(),
                    };
                    tasks.spawn(async move {
                        if let Err(error) = run_session(peer, receiver, &context).await {
                            tracing::debug!(tunnel = %context.config.name, %peer, %error, "UDP 会话结束");
                        }
                        if context.cleanup.send(peer).await.is_err() {
                            tracing::debug!(tunnel = %context.config.name, %peer, "UDP 监听已停止");
                        }
                    });
                }
                Err(error) => {
                    tracing::error!(tunnel = %config.name, %error, "接收公网 UDP 数据失败");
                    break;
                }
            },
            Some(peer) = cleanup_receiver.recv() => {
                sessions.remove(&peer);
            }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result {
                    tracing::warn!(tunnel = %config.name, %error, "UDP 转发任务异常结束");
                }
            }
        }
    }
    drop(socket);
    sessions.clear();
    if stopped.send(()).is_err() {
        tracing::debug!(tunnel = %config.name, "监听停止接收方已释放");
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::warn!(tunnel = %config.name, %error, "UDP 转发任务异常结束");
        }
    }
}

struct SessionContext {
    socket: Arc<UdpSocket>,
    runtime: TunnelRuntime,
    limiter: RateLimiter,
    cleanup: mpsc::Sender<SocketAddr>,
    config: ServerTunnelConfig,
}

async fn run_session(
    peer: SocketAddr,
    mut inbound: mpsc::Receiver<Vec<u8>>,
    context: &SessionContext,
) -> Result<()> {
    let Some(session) = context.runtime.find(&context.config.name).await else {
        return Err(TunnelError::Protocol("没有可用的内网客户端".into()));
    };
    let (mut send, mut receive) =
        tokio::time::timeout(HANDSHAKE_TIMEOUT, session.connection.open_bi())
            .await
            .map_err(|_| TunnelError::Timeout("打开 QUIC UDP 数据流"))??;
    write_frame(
        &mut send,
        &OpenRequest {
            service: context.config.name.clone(),
            destination: None,
        },
    )
    .await?;
    let response: OpenResponse = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut receive))
        .await
        .map_err(|_| TunnelError::Timeout("等待 UDP 目标响应"))??;
    if !response.accepted {
        return Err(TunnelError::Protocol(format!(
            "内网 UDP 目标连接失败: {}",
            response.message
        )));
    }

    let idle = Duration::from_secs(context.config.udp_idle_seconds);
    let mut buffer = Vec::new();
    loop {
        let event = tokio::time::timeout(idle, async {
            tokio::select! {
                packet = inbound.recv() => match packet {
                    Some(packet) => Ok(SessionEvent::Inbound(packet)),
                    None => Ok(SessionEvent::Closed),
                },
                packet = read_datagram(&mut receive, &mut buffer) => {
                    packet.map(SessionEvent::Outbound)
                }
            }
        })
        .await
        .map_err(|_| TunnelError::Timeout("UDP 会话空闲"))??;

        match event {
            SessionEvent::Inbound(packet) => {
                context.limiter.acquire(packet.len()).await;
                write_datagram(&mut send, &packet).await?;
            }
            SessionEvent::Outbound(length) => {
                context.limiter.acquire(length).await;
                context.socket.send_to(&buffer[..length], peer).await?;
            }
            SessionEvent::Closed => return Ok(()),
        }
    }
}

enum SessionEvent {
    Inbound(Vec<u8>),
    Outbound(usize),
    Closed,
}
