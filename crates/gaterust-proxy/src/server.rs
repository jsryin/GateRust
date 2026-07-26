use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use http::Request;
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{RwLock, Semaphore},
    task::JoinHandle,
};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use crate::{
    ProxyConfig, ProxyRuntime, Result,
    config::ProxyListenerConfig,
    proxy::{ProxyBody, ProxyService},
    router::Router,
    runtime::listener_restart_required,
    tls::CertificateResolver,
    watcher::ConfigWatcher,
};

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// 运行反向代理，直到收到 Ctrl-C。
///
/// # Errors
///
/// 初始配置、监听地址或文件监听器初始化失败时返回错误。
pub async fn run_proxy(config_path: impl AsRef<Path>) -> Result<()> {
    let cancellation = CancellationToken::new();
    let runtime = ProxyRuntime::new();
    let proxy = run_proxy_with_runtime(config_path, runtime, cancellation.clone());
    tokio::pin!(proxy);
    tokio::select! {
        result = &mut proxy => result,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            cancellation.cancel();
            proxy.await
        }
    }
}

/// 使用内部运行时启动代理，保留原有公开调用方式。
///
/// # Errors
///
/// 初始配置、TLS、监听地址或文件监听器初始化失败时返回错误。
pub async fn run_proxy_with_shutdown(
    config_path: impl AsRef<Path>,
    cancellation: CancellationToken,
) -> Result<()> {
    run_proxy_with_runtime(config_path, ProxyRuntime::new(), cancellation).await
}

/// 使用共享运行时启动代理，供控制面发起证书操作和读取状态。
///
/// # Errors
///
/// 初始配置、TLS、监听地址、运行时或文件监听器初始化失败时返回错误。
pub async fn run_proxy_with_runtime(
    config_path: impl AsRef<Path>,
    runtime: ProxyRuntime,
    cancellation: CancellationToken,
) -> Result<()> {
    let config_path = config_path.as_ref().to_owned();
    let initial = ProxyConfig::load(&config_path)?;
    let mut watcher = ConfigWatcher::new(&config_path)?;
    let http_listener = TcpListener::bind(initial.proxy.http_bind).await?;
    let https_listener = TcpListener::bind(initial.proxy.https_bind).await?;
    let http_address = http_listener.local_addr()?;
    let https_address = https_listener.local_addr()?;

    let routes = Arc::new(RwLock::new(Arc::new(Router::new(&initial)?)));
    let service = ProxyService::new(routes);
    let resolver = CertificateResolver::new();
    let tls_config = resolver.server_config();
    let manager_runtime = runtime.clone();
    let manager_token = cancellation.child_token();
    let manager_task = tokio::spawn(async move {
        manager_runtime
            .run_manager(resolver.clone(), manager_token)
            .await
    });
    runtime.apply_config(initial.clone(), false).await?;

    let permits = Arc::new(Semaphore::new(initial.proxy.max_connections));
    let http_task = tokio::spawn(run_http_listener(
        http_listener,
        service.clone(),
        Arc::clone(&permits),
        cancellation.child_token(),
    ));
    let https_task = tokio::spawn(run_https_listener(
        https_listener,
        service.clone(),
        TlsAcceptor::from(tls_config),
        permits,
        cancellation.child_token(),
    ));
    let running_listener = initial.proxy;
    tracing::info!(http = %http_address, https = %https_address, "反向代理已启动");

    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            changed = watcher.changed() => {
                if !changed {
                    break;
                }
                reload(&config_path, &running_listener, &service, &runtime).await;
            }
        }
    }

    cancellation.cancel();
    await_listener(http_task, "HTTP").await;
    await_listener(https_task, "HTTPS").await;
    service.shutdown().await;
    match manager_task.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, "证书运行时异常结束"),
        Err(error) => tracing::warn!(%error, "证书运行时任务异常结束"),
    }
    tracing::info!("反向代理已停止");
    Ok(())
}

async fn reload(
    path: &Path,
    running_listener: &ProxyListenerConfig,
    service: &ProxyService,
    runtime: &ProxyRuntime,
) {
    let config = match ProxyConfig::load(path) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "新代理配置无效，继续使用当前配置");
            return;
        }
    };
    let routes = match Router::new(&config) {
        Ok(routes) => routes,
        Err(error) => {
            tracing::error!(%error, "编译新代理路由失败，继续使用当前配置");
            return;
        }
    };
    let restart_required = listener_restart_required(running_listener, &config.proxy);
    if let Err(error) = runtime.apply_config(config.clone(), restart_required).await {
        tracing::error!(%error, "应用证书配置失败，继续使用当前配置");
        return;
    }
    service.replace_routes(routes).await;
    if restart_required {
        tracing::warn!("代理监听参数已保存，需重启服务后生效；证书与路由已热更新");
    } else {
        tracing::info!(
            routes = config.routes.len(),
            certificates = config.certificates.len(),
            "代理配置已热更新"
        );
    }
}

async fn run_http_listener(
    listener: TcpListener,
    service: ProxyService,
    permits: Arc<Semaphore>,
    cancellation: CancellationToken,
) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            () = cancellation.cancelled() => break,
            accepted = listener.accept() => accepted,
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    tracing::debug!(%error, "HTTP 连接任务异常结束");
                }
                continue;
            }
        };
        let (stream, remote) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "接受 HTTP 连接失败");
                continue;
            }
        };
        let permit = tokio::select! {
            () = cancellation.cancelled() => break,
            permit = Arc::clone(&permits).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            }
        };
        let service = service.clone();
        connections.spawn(async move {
            let _permit = permit;
            let handler = service_fn(move |request| handle_http(request, remote, service.clone()));
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler)
                .with_upgrades()
                .await
            {
                tracing::debug!(%remote, %error, "HTTP 连接结束");
            }
        });
    }
    connections.shutdown().await;
}

async fn handle_http(
    request: Request<Incoming>,
    remote: SocketAddr,
    service: ProxyService,
) -> std::result::Result<http::Response<ProxyBody>, std::convert::Infallible> {
    Ok(service.handle(request, remote, None).await)
}

async fn run_https_listener(
    listener: TcpListener,
    service: ProxyService,
    acceptor: TlsAcceptor,
    permits: Arc<Semaphore>,
    cancellation: CancellationToken,
) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            () = cancellation.cancelled() => break,
            accepted = listener.accept() => accepted,
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    tracing::debug!(%error, "HTTPS 连接任务异常结束");
                }
                continue;
            }
        };
        let (stream, remote) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "接受 HTTPS 连接失败");
                continue;
            }
        };
        let permit = tokio::select! {
            () = cancellation.cancelled() => break,
            permit = Arc::clone(&permits).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            }
        };
        let service = service.clone();
        let acceptor = acceptor.clone();
        connections.spawn(async move {
            let _permit = permit;
            serve_tls(stream, remote, acceptor, service).await;
        });
    }
    connections.shutdown().await;
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
