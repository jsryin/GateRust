use std::{
    future::Future,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify},
    task::{JoinHandle, JoinSet},
};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use crate::proxy::ProxyService;

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

struct ConnectionLimit {
    maximum: AtomicUsize,
    active: AtomicUsize,
    changed: Notify,
}

struct ConnectionPermit {
    limit: Arc<ConnectionLimit>,
}

#[derive(Clone)]
struct ConnectionTasks {
    tasks: Arc<Mutex<Option<JoinSet<()>>>>,
}

pub(crate) struct ConnectionRuntime {
    service: ProxyService,
    acceptor: TlsAcceptor,
    limit: Arc<ConnectionLimit>,
    tasks: ConnectionTasks,
}

impl ConnectionLimit {
    fn new(maximum: usize) -> Self {
        Self {
            maximum: AtomicUsize::new(maximum),
            active: AtomicUsize::new(0),
            changed: Notify::new(),
        }
    }

    fn set_maximum(&self, maximum: usize) {
        self.maximum.store(maximum, Ordering::Release);
        self.changed.notify_waiters();
    }

    async fn acquire(
        self: &Arc<Self>,
        cancellation: &CancellationToken,
    ) -> Option<ConnectionPermit> {
        loop {
            // 先注册通知再读取计数，避免释放名额与进入等待之间丢失唤醒。
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let active = self.active.load(Ordering::Acquire);
            let maximum = self.maximum.load(Ordering::Acquire);
            if active < maximum {
                if self
                    .active
                    .compare_exchange_weak(active, active + 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Some(ConnectionPermit {
                        limit: Arc::clone(self),
                    });
                }
                continue;
            }

            tokio::select! {
                () = cancellation.cancelled() => return None,
                () = &mut notified => {}
            }
        }
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.limit.active.fetch_sub(1, Ordering::AcqRel);
        self.limit.changed.notify_one();
    }
}

impl ConnectionTasks {
    fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(Some(JoinSet::new()))),
        }
    }

    async fn spawn<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().await;
        let Some(tasks) = tasks.as_mut() else {
            return;
        };
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                tracing::debug!(%error, "代理连接任务异常结束");
            }
        }
        tasks.spawn(task);
    }

    async fn shutdown(&self) {
        let tasks = self.tasks.lock().await.take();
        if let Some(mut tasks) = tasks {
            tasks.shutdown().await;
        }
    }
}

impl ConnectionRuntime {
    pub(crate) fn new(service: ProxyService, acceptor: TlsAcceptor, maximum: usize) -> Self {
        Self {
            service,
            acceptor,
            limit: Arc::new(ConnectionLimit::new(maximum)),
            tasks: ConnectionTasks::new(),
        }
    }

    pub(crate) fn set_maximum(&self, maximum: usize) {
        self.limit.set_maximum(maximum);
    }

    pub(crate) fn spawn_http(
        &self,
        listener: TcpListener,
        cancellation: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(run_http_listener(
            listener,
            self.service.clone(),
            Arc::clone(&self.limit),
            self.tasks.clone(),
            cancellation,
        ))
    }

    pub(crate) fn spawn_https(
        &self,
        listener: TcpListener,
        cancellation: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(run_https_listener(
            listener,
            self.service.clone(),
            self.acceptor.clone(),
            Arc::clone(&self.limit),
            self.tasks.clone(),
            cancellation,
        ))
    }

    pub(crate) async fn shutdown(&self) {
        self.tasks.shutdown().await;
    }
}

async fn run_http_listener(
    listener: TcpListener,
    service: ProxyService,
    limit: Arc<ConnectionLimit>,
    tasks: ConnectionTasks,
    cancellation: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            () = cancellation.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let (stream, remote) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "接受 HTTP 连接失败");
                continue;
            }
        };
        let Some(permit) = limit.acquire(&cancellation).await else {
            break;
        };
        let service = service.clone();
        tasks
            .spawn(async move {
                let _permit = permit;
                let handler = service_fn(move |request| {
                    let service = service.clone();
                    async move {
                        Ok::<_, std::convert::Infallible>(
                            service.handle(request, remote, None).await,
                        )
                    }
                });
                if let Err(error) = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), handler)
                    .with_upgrades()
                    .await
                {
                    tracing::debug!(%remote, %error, "HTTP 连接结束");
                }
            })
            .await;
    }
}

async fn run_https_listener(
    listener: TcpListener,
    service: ProxyService,
    acceptor: TlsAcceptor,
    limit: Arc<ConnectionLimit>,
    tasks: ConnectionTasks,
    cancellation: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            () = cancellation.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let (stream, remote) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "接受 HTTPS 连接失败");
                continue;
            }
        };
        let Some(permit) = limit.acquire(&cancellation).await else {
            break;
        };
        let service = service.clone();
        let acceptor = acceptor.clone();
        tasks
            .spawn(async move {
                let _permit = permit;
                serve_tls(stream, remote, acceptor, service).await;
            })
            .await;
    }
}

async fn serve_tls(
    stream: TcpStream,
    remote: SocketAddr,
    acceptor: TlsAcceptor,
    service: ProxyService,
) {
    let tls = match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            tracing::debug!(%remote, %error, "TLS 握手失败");
            return;
        }
        Err(_) => {
            tracing::debug!(%remote, "TLS 握手超时");
            return;
        }
    };
    let Some(server_name) = tls.get_ref().1.server_name().map(Arc::<str>::from) else {
        tracing::debug!(%remote, "TLS 客户端未提供 SNI");
        return;
    };
    let handler = service_fn(move |request| {
        let service = service.clone();
        let server_name = Arc::clone(&server_name);
        async move {
            Ok::<_, std::convert::Infallible>(
                service.handle(request, remote, Some(&server_name)).await,
            )
        }
    });
    if let Err(error) = http1::Builder::new()
        .serve_connection(TokioIo::new(tls), handler)
        .with_upgrades()
        .await
    {
        tracing::debug!(%remote, %error, "HTTPS 连接结束");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connection_limit_updates_apply_to_new_connections() {
        let limit = Arc::new(ConnectionLimit::new(1));
        let cancellation = CancellationToken::new();
        let first = limit
            .acquire(&cancellation)
            .await
            .expect("第一个连接应获得名额");

        assert!(
            tokio::time::timeout(Duration::from_millis(10), limit.acquire(&cancellation))
                .await
                .is_err()
        );

        limit.set_maximum(2);
        let second = tokio::time::timeout(Duration::from_millis(50), limit.acquire(&cancellation))
            .await
            .expect("提高上限后不应等待")
            .expect("运行时未取消");
        limit.set_maximum(1);
        drop(first);

        assert!(
            tokio::time::timeout(Duration::from_millis(10), limit.acquire(&cancellation))
                .await
                .is_err(),
            "已有连接达到新上限时不能接入新连接"
        );

        drop(second);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), limit.acquire(&cancellation))
                .await
                .expect("释放名额后不应等待")
                .is_some()
        );
    }
}
