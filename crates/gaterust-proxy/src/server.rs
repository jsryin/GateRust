use std::{path::Path, sync::Arc};

use tokio::sync::RwLock;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use crate::{
    ProxyConfig, ProxyRuntime, Result, listener::ListenerManager, proxy::ProxyService,
    router::Router, tls::CertificateResolver, watcher::ConfigWatcher,
};

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
    runtime.apply_config(initial.clone()).await?;

    let listener_result = ListenerManager::bind(
        initial.proxy.clone(),
        service.clone(),
        TlsAcceptor::from(tls_config),
        cancellation.clone(),
    )
    .await;
    let (mut listeners, http_address, https_address) = match listener_result {
        Ok(listeners) => listeners,
        Err(error) => {
            cancellation.cancel();
            match manager_task.await {
                Ok(Ok(())) => {}
                Ok(Err(manager_error)) => {
                    tracing::warn!(%manager_error, "证书运行时异常结束");
                }
                Err(join_error) => {
                    tracing::warn!(%join_error, "等待证书运行时停止失败");
                }
            }
            return Err(error);
        }
    };
    runtime.report_config_applied(&initial);
    tracing::info!(http = %http_address, https = %https_address, "反向代理已启动");

    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            changed = watcher.changed() => {
                if !changed {
                    break;
                }
                reload(&config_path, &service, &runtime, &mut listeners).await;
            }
        }
    }

    cancellation.cancel();
    listeners.shutdown().await;
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
    service: &ProxyService,
    runtime: &ProxyRuntime,
    listeners: &mut ListenerManager,
) {
    let config = match ProxyConfig::load(path) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "新代理配置无效，继续使用当前配置");
            runtime.report_config_load_error(error.to_string());
            return;
        }
    };
    let routes = match Router::new(&config) {
        Ok(routes) => routes,
        Err(error) => {
            tracing::error!(%error, "编译新代理路由失败，继续使用当前配置");
            runtime.report_config_failed(&config, error.to_string());
            return;
        }
    };
    let previous_listener = listeners.config().clone();
    if let Err(error) = listeners.apply(&config.proxy).await {
        tracing::error!(%error, "应用代理监听配置失败，继续使用当前监听");
        runtime.report_config_failed(&config, error.to_string());
        return;
    }
    if let Err(error) = runtime.apply_config(config.clone()).await {
        tracing::error!(%error, "应用证书配置失败，恢复原代理监听");
        if let Err(rollback_error) = listeners.apply(&previous_listener).await {
            tracing::error!(%rollback_error, "恢复原代理监听失败");
        }
        runtime.report_config_failed(&config, error.to_string());
        return;
    }
    service.replace_routes(routes).await;
    runtime.report_config_applied(&config);
    tracing::info!(
        http = %config.proxy.http_bind,
        https = %config.proxy.https_bind,
        max_connections = config.proxy.max_connections,
        routes = config.routes.len(),
        certificates = config.certificates.len(),
        "代理配置已热更新"
    );
}
