use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use tokio::{
    net::UdpSocket,
    sync::{OwnedSemaphorePermit, mpsc, oneshot},
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
    resource::ResourceBudget,
    runtime::TunnelRuntime,
};

use super::ListenerResources;

const SESSION_QUEUE: usize = 8;

pub(super) async fn bind(config: &ServerTunnelConfig) -> Result<UdpSocket> {
    Ok(UdpSocket::bind(config.bind).await?)
}

pub(super) async fn run(
    socket: UdpSocket,
    config: ServerTunnelConfig,
    runtime: TunnelRuntime,
    resources: ListenerResources,
    cancellation: CancellationToken,
    stopped: oneshot::Sender<()>,
) {
    let ListenerResources { budget, limiter } = resources;
    let socket = Arc::new(socket);
    let mut sessions = HashMap::<SocketAddr, mpsc::Sender<QueuedDatagram>>::new();
    let mut tasks = JoinSet::new();
    let mut buffer = vec![0; MAX_DATAGRAM];

    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            received = socket.recv_from(&mut buffer) => match received {
                Ok((length, peer)) => {
                    if let Some(sender) = sessions.get(&peer) {
                        let Ok(slot) = sender.try_reserve() else {
                            tracing::debug!(tunnel = %config.name, %peer, "UDP 会话队列已满，丢弃数据报");
                            continue;
                        };
                        let Some(packet) = queued_datagram(&budget, &buffer[..length]) else {
                            tracing::debug!(tunnel = %config.name, %peer, "UDP 全局排队字节已满，丢弃数据报");
                            continue;
                        };
                        slot.send(packet);
                        continue;
                    }
                    if sessions.len() >= config.max_udp_sessions {
                        tracing::warn!(tunnel = %config.name, %peer, "UDP 会话数已满，丢弃数据报");
                        continue;
                    }
                    let Some(stream_permit) = budget.try_data_stream() else {
                        tracing::warn!(tunnel = %config.name, %peer, "服务端数据流总数已满，丢弃数据报");
                        continue;
                    };
                    let Some(session_permit) = budget.try_udp_session() else {
                        tracing::warn!(tunnel = %config.name, %peer, "服务端 UDP 会话总数已满，丢弃数据报");
                        continue;
                    };
                    let Some(packet) = queued_datagram(&budget, &buffer[..length]) else {
                        tracing::debug!(tunnel = %config.name, %peer, "UDP 全局排队字节已满，丢弃数据报");
                        continue;
                    };
                    let (sender, receiver) = mpsc::channel(SESSION_QUEUE);
                    if sender.try_send(packet).is_err() {
                        continue;
                    }
                    sessions.insert(peer, sender);
                    let context = SessionContext {
                        socket: Arc::clone(&socket),
                        runtime: runtime.clone(),
                        limiter: limiter.clone(),
                        config: config.clone(),
                    };
                    tasks.spawn(async move {
                        let _permits = (stream_permit, session_permit);
                        (peer, run_session(peer, receiver, &context).await)
                    });
                }
                Err(error) => {
                    tracing::error!(tunnel = %config.name, %error, "接收公网 UDP 数据失败");
                    break;
                }
            },
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                match result {
                    Ok((peer, result)) => {
                        sessions.remove(&peer);
                        if let Err(error) = result {
                            tracing::debug!(tunnel = %config.name, %peer, %error, "UDP 会话结束");
                        }
                    }
                    Err(error) => tracing::warn!(tunnel = %config.name, %error, "UDP 转发任务异常结束"),
                }
            }
        }
    }
    sessions.clear();
    tasks.abort_all();
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result
            && !error.is_cancelled()
        {
            tracing::warn!(tunnel = %config.name, %error, "UDP 转发任务异常结束");
        }
    }
    drop(socket);
    if stopped.send(()).is_err() {
        tracing::debug!(tunnel = %config.name, "监听停止接收方已释放");
    }
}

struct SessionContext {
    socket: Arc<UdpSocket>,
    runtime: TunnelRuntime,
    limiter: RateLimiter,
    config: ServerTunnelConfig,
}

struct QueuedDatagram {
    payload: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

fn queued_datagram(budget: &ResourceBudget, payload: &[u8]) -> Option<QueuedDatagram> {
    let permit = budget.try_queue_udp_bytes(payload.len())?;
    Some(QueuedDatagram {
        payload: payload.to_vec(),
        _permit: permit,
    })
}

async fn run_session(
    peer: SocketAddr,
    mut inbound: mpsc::Receiver<QueuedDatagram>,
    context: &SessionContext,
) -> Result<()> {
    let Some(session) = context.runtime.find(&context.config.name).await else {
        return Err(TunnelError::Protocol("没有可用的内网客户端".into()));
    };

    tokio::select! {
        biased;
        () = session.tunnel_shutdown.cancelled() => Ok(()),
        result = relay_session(peer, &mut inbound, context, session.connection) => result,
    }
}

async fn relay_session(
    peer: SocketAddr,
    inbound: &mut mpsc::Receiver<QueuedDatagram>,
    context: &SessionContext,
    connection: quinn::Connection,
) -> Result<()> {
    let (mut send, mut receive) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.open_bi())
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
                context.limiter.acquire(packet.payload.len()).await;
                write_datagram(&mut send, &packet.payload).await?;
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
    Inbound(QueuedDatagram),
    Outbound(usize),
    Closed,
}
