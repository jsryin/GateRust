use std::sync::Arc;

use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Result, TunnelError,
    config::{ServerTunnelConfig, TunnelKind},
    protocol::{HANDSHAKE_TIMEOUT, OpenRequest, OpenResponse, read_frame, write_frame},
    rate_limit::RateLimiter,
    relay,
    runtime::TunnelRuntime,
};

use super::socks5;

pub(super) async fn bind(config: &ServerTunnelConfig) -> Result<(TcpListener, Arc<Semaphore>)> {
    let listener = TcpListener::bind(config.bind).await?;
    Ok((listener, Arc::new(Semaphore::new(config.max_connections))))
}

pub(super) async fn run(
    listener: TcpListener,
    permits: Arc<Semaphore>,
    config: ServerTunnelConfig,
    runtime: TunnelRuntime,
    cancellation: CancellationToken,
    stopped: oneshot::Sender<()>,
) {
    let limiter = RateLimiter::new(config.limit_bps);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                        tracing::warn!(tunnel = %config.name, %peer, "隧道并发数已满，拒绝连接");
                        continue;
                    };
                    let config = config.clone();
                    let runtime = runtime.clone();
                    let limiter = limiter.clone();
                    connections.spawn(async move {
                        let _permit = permit;
                        if let Err(error) = handle(stream, &config, &runtime, &limiter).await {
                            tracing::debug!(tunnel = %config.name, %peer, %error, "流式隧道连接结束");
                        }
                    });
                }
                Err(error) => {
                    tracing::error!(tunnel = %config.name, %error, "接受公网连接失败");
                    break;
                }
            },
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = result {
                    tracing::warn!(tunnel = %config.name, %error, "转发任务异常结束");
                }
            }
        }
    }
    drop(listener);
    if stopped.send(()).is_err() {
        tracing::debug!(tunnel = %config.name, "监听停止接收方已释放");
    }
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            tracing::warn!(tunnel = %config.name, %error, "转发任务异常结束");
        }
    }
}

async fn handle(
    mut public: TcpStream,
    config: &ServerTunnelConfig,
    runtime: &TunnelRuntime,
    limiter: &RateLimiter,
) -> Result<()> {
    let destination = if config.kind == TunnelKind::Socks5 {
        Some(
            tokio::time::timeout(HANDSHAKE_TIMEOUT, socks5::handshake(&mut public))
                .await
                .map_err(|_| TunnelError::Timeout("SOCKS5 握手"))??,
        )
    } else {
        None
    };
    let Some(session) = runtime.find(&config.name).await else {
        if config.kind == TunnelKind::Socks5 {
            socks5::send_reply(&mut public, 1).await?;
        }
        return Err(TunnelError::Protocol("没有可用的内网客户端".into()));
    };

    let (mut send, mut receive) =
        tokio::time::timeout(HANDSHAKE_TIMEOUT, session.connection.open_bi())
            .await
            .map_err(|_| TunnelError::Timeout("打开 QUIC 数据流"))??;
    write_frame(
        &mut send,
        &OpenRequest {
            service: config.name.clone(),
            destination,
        },
    )
    .await?;
    let response: OpenResponse = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut receive))
        .await
        .map_err(|_| TunnelError::Timeout("等待内网连接响应"))??;
    if !response.accepted {
        if config.kind == TunnelKind::Socks5 {
            socks5::send_reply(&mut public, 5).await?;
        }
        return Err(TunnelError::Protocol(format!(
            "内网目标连接失败: {}",
            response.message
        )));
    }
    if config.kind == TunnelKind::Socks5 {
        socks5::send_reply(&mut public, 0).await?;
    }
    relay::copy_bidirectional(&mut public, &mut relay::QuinnStream(send, receive), limiter).await
}
