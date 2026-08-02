use std::{net::SocketAddr, time::Duration};

use tokio::{net::TcpListener, task::JoinHandle};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use crate::{
    ProxyError, ProxyListenerConfig, Result, connection::ConnectionRuntime, proxy::ProxyService,
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

struct ActiveListener {
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

struct PreparedListeners {
    http: Option<TcpListener>,
    https: Option<TcpListener>,
}

pub(crate) struct ListenerManager {
    connections: ConnectionRuntime,
    cancellation: CancellationToken,
    config: ProxyListenerConfig,
    http: Option<ActiveListener>,
    https: Option<ActiveListener>,
}

impl ListenerManager {
    pub(crate) async fn bind(
        config: ProxyListenerConfig,
        service: ProxyService,
        acceptor: TlsAcceptor,
        cancellation: CancellationToken,
    ) -> Result<(Self, SocketAddr, SocketAddr)> {
        let (http_listener, https_listener) = bind_listener_pair(&config).await?;
        let http_address = http_listener.local_addr()?;
        let https_address = https_listener.local_addr()?;
        let connections = ConnectionRuntime::new(service, acceptor, config.max_connections);
        let http = spawn_listener(cancellation.child_token(), |token| {
            connections.spawn_http(http_listener, token)
        });
        let https = spawn_listener(cancellation.child_token(), |token| {
            connections.spawn_https(https_listener, token)
        });
        Ok((
            Self {
                connections,
                cancellation,
                config,
                http: Some(http),
                https: Some(https),
            },
            http_address,
            https_address,
        ))
    }

    pub(crate) fn config(&self) -> &ProxyListenerConfig {
        &self.config
    }

    pub(crate) async fn apply(&mut self, config: &ProxyListenerConfig) -> Result<()> {
        // 普通切换先绑定新地址，任一绑定失败时旧监听保持不变。
        let prepared = match self.prepare(config).await {
            Ok(prepared) => prepared,
            Err(error) if self.has_internal_address_conflict(config) => {
                return self.rebind_all(config, error).await;
            }
            Err(error) => return Err(error),
        };

        self.connections.set_maximum(config.max_connections);
        let old_http = if let Some(listener) = prepared.http {
            let replacement = self.spawn_http(listener);
            self.http.replace(replacement)
        } else {
            None
        };
        let old_https = if let Some(listener) = prepared.https {
            let replacement = self.spawn_https(listener);
            self.https.replace(replacement)
        } else {
            None
        };
        stop_listeners(old_http, old_https).await;
        self.config.clone_from(config);
        Ok(())
    }

    async fn prepare(&self, config: &ProxyListenerConfig) -> Result<PreparedListeners> {
        let http = if self.config.http_bind == config.http_bind {
            None
        } else {
            Some(bind_listener("HTTP", config.http_bind).await?)
        };
        let https = if self.config.https_bind == config.https_bind {
            None
        } else {
            Some(bind_listener("HTTPS", config.https_bind).await?)
        };
        Ok(PreparedListeners { http, https })
    }

    fn has_internal_address_conflict(&self, config: &ProxyListenerConfig) -> bool {
        (self.config.http_bind != config.http_bind && config.http_bind == self.config.https_bind)
            || (self.config.https_bind != config.https_bind
                && config.https_bind == self.config.http_bind)
    }

    async fn rebind_all(
        &mut self,
        config: &ProxyListenerConfig,
        initial_error: ProxyError,
    ) -> Result<()> {
        // HTTP/HTTPS 互换端口时无法预绑定，短暂停止接入后切换，失败则恢复原地址。
        let previous = self.config.clone();
        self.stop_active().await;
        match bind_listener_pair(config).await {
            Ok((http, https)) => {
                self.connections.set_maximum(config.max_connections);
                self.http = Some(self.spawn_http(http));
                self.https = Some(self.spawn_https(https));
                self.config.clone_from(config);
                Ok(())
            }
            Err(error) => match bind_listener_pair(&previous).await {
                Ok((http, https)) => {
                    self.http = Some(self.spawn_http(http));
                    self.https = Some(self.spawn_https(https));
                    Err(error)
                }
                Err(rollback_error) => Err(ProxyError::Runtime(format!(
                    "切换监听失败（{initial_error}；重试失败: {error}），恢复原监听也失败: {rollback_error}"
                ))),
            },
        }
    }

    fn spawn_http(&self, listener: TcpListener) -> ActiveListener {
        spawn_listener(self.cancellation.child_token(), |token| {
            self.connections.spawn_http(listener, token)
        })
    }

    fn spawn_https(&self, listener: TcpListener) -> ActiveListener {
        spawn_listener(self.cancellation.child_token(), |token| {
            self.connections.spawn_https(listener, token)
        })
    }

    async fn stop_active(&mut self) {
        stop_listeners(self.http.take(), self.https.take()).await;
    }

    pub(crate) async fn shutdown(&mut self) {
        self.stop_active().await;
        self.connections.shutdown().await;
    }
}

fn spawn_listener(
    cancellation: CancellationToken,
    spawn: impl FnOnce(CancellationToken) -> JoinHandle<()>,
) -> ActiveListener {
    let task = spawn(cancellation.clone());
    ActiveListener { cancellation, task }
}

async fn bind_listener(protocol: &'static str, address: SocketAddr) -> Result<TcpListener> {
    TcpListener::bind(address).await.map_err(|error| {
        ProxyError::Runtime(format!("绑定 {protocol} 监听地址 {address} 失败: {error}"))
    })
}

async fn bind_listener_pair(config: &ProxyListenerConfig) -> Result<(TcpListener, TcpListener)> {
    let http = bind_listener("HTTP", config.http_bind).await?;
    let https = bind_listener("HTTPS", config.https_bind).await?;
    Ok((http, https))
}

async fn stop_listeners(http: Option<ActiveListener>, https: Option<ActiveListener>) {
    if let Some(listener) = &http {
        listener.cancellation.cancel();
    }
    if let Some(listener) = &https {
        listener.cancellation.cancel();
    }
    let http = async {
        if let Some(listener) = http {
            await_listener(listener.task, "HTTP").await;
        }
    };
    let https = async {
        if let Some(listener) = https {
            await_listener(listener.task, "HTTPS").await;
        }
    };
    tokio::join!(http, https);
}

async fn await_listener(mut task: JoinHandle<()>, name: &str) {
    match tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(listener = name, %error, "代理监听任务异常结束"),
        Err(_) => {
            tracing::warn!(listener = name, "等待代理监听任务退出超时");
            task.abort();
        }
    }
}
