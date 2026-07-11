//! `GateRust` Web 控制平面。

mod api;
mod auth;
mod config;
mod error;
mod store;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower_http::services::{ServeDir, ServeFile};

pub use auth::hash_password;
pub use config::ControlConfig;
pub use error::{ControlError, Result};
use store::{ConfigKind, ConfigStore};

#[derive(Clone)]
pub struct ControlOptions {
    pub tunnel_enabled: bool,
    pub proxy_enabled: bool,
    #[cfg(feature = "tunnel")]
    pub tunnel_config: PathBuf,
    #[cfg(feature = "proxy")]
    pub proxy_config: PathBuf,
}

/// 启动 Web API、配置监听和可选的静态 SPA 服务。
///
/// # Errors
///
/// 配置无效、文件监听失败或 HTTP 服务无法启动时返回错误。
pub async fn run_control_with_shutdown(
    config_path: &Path,
    options: ControlOptions,
    cancellation: CancellationToken,
) -> Result<()> {
    let config = ControlConfig::load(config_path)?;
    let auth = auth::AuthService::new(&config.web)?;
    let store = ConfigStore::new(&options)?;
    let (mut watcher, mut changes) = watch_configs(&store)?;
    let mut app = api::router(&config.web, auth, store.clone());
    if let Some(static_dir) = &config.web.static_dir {
        let index = static_dir.join("index.html");
        if index.is_file() {
            app = app.fallback_service(
                ServeDir::new(static_dir).not_found_service(ServeFile::new(index)),
            );
        } else {
            tracing::warn!(path = %static_dir.display(), "Web UI 静态目录缺少 index.html，仅启动 API");
        }
    }

    let listener = tokio::net::TcpListener::bind(config.web.bind)
        .await
        .map_err(ControlError::Bind)?;
    tracing::info!(address = %config.web.bind, "Web 控制平面已启动");
    let reload_cancellation = CancellationToken::new();
    let reload_token = reload_cancellation.clone();
    let reload_store = store.clone();
    let reload_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = reload_token.cancelled() => break,
                change = changes.recv() => {
                    let Some(kind) = change else { break };
                    reload_store.reload(kind).await;
                }
            }
        }
    });
    let result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(cancellation.cancelled_owned())
    .await
    .map_err(ControlError::Serve);
    reload_cancellation.cancel();
    if let Err(error) = reload_task.await {
        tracing::warn!(%error, "配置监听任务异常结束");
    }
    drop(watcher.take());
    result
}

fn watch_configs(
    store: &ConfigStore,
) -> Result<(Option<RecommendedWatcher>, mpsc::Receiver<ConfigKind>)> {
    let paths = store.watched_paths();
    let (sender, receiver) = mpsc::channel(16);
    let callback_paths = paths.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        for (kind, path) in &callback_paths {
            if event.paths.iter().any(|changed| changed == path) {
                match sender.try_send(*kind) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                }
            }
        }
    })?;
    let mut parents = HashSet::new();
    for (_, path) in paths {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        if parents.insert(parent.to_owned()) {
            std::fs::create_dir_all(parent)
                .map_err(|error| ControlError::ReadRuntimeConfig(error.to_string()))?;
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
        }
    }
    Ok((Some(watcher), receiver))
}
