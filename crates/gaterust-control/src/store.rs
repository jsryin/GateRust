use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use rand::RngExt as _;
use serde::Serialize;
use tokio::sync::{RwLock, watch};

use crate::{ControlError, Result};

#[derive(Clone, Serialize)]
pub(crate) struct ConfigSnapshot {
    #[cfg(feature = "tunnel")]
    pub tunnel: Option<gaterust_tunnel::ServerConfig>,
    #[cfg(feature = "proxy")]
    pub proxy: Option<gaterust_proxy::ProxyConfig>,
}

#[derive(Clone, Serialize)]
pub(crate) struct Dashboard {
    pub revision: u64,
    pub tunnel_enabled: bool,
    pub proxy_enabled: bool,
    pub groups: usize,
    pub tunnels: usize,
    pub certificates: usize,
    pub routes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigKind {
    Tunnel,
    Proxy,
}

#[derive(Clone)]
pub(crate) struct ConfigStore {
    inner: Arc<StoreInner>,
}

struct StoreInner {
    snapshot: RwLock<ConfigSnapshot>,
    revision: AtomicU64,
    dashboard: watch::Sender<Dashboard>,
    tunnel_enabled: bool,
    proxy_enabled: bool,
    #[cfg(feature = "tunnel")]
    tunnel_path: PathBuf,
    #[cfg(feature = "tunnel")]
    tunnel_runtime: Option<gaterust_tunnel::TunnelRuntime>,
    #[cfg(feature = "proxy")]
    proxy_path: PathBuf,
}

impl ConfigStore {
    pub(crate) fn new(options: &crate::ControlOptions) -> Result<Self> {
        #[cfg(feature = "tunnel")]
        let tunnel = load_optional_tunnel(&options.tunnel_config)?;
        #[cfg(feature = "proxy")]
        let proxy = load_optional_proxy(&options.proxy_config)?;
        let snapshot = ConfigSnapshot {
            #[cfg(feature = "tunnel")]
            tunnel,
            #[cfg(feature = "proxy")]
            proxy,
        };
        let initial = dashboard_for(&snapshot, 1, options.tunnel_enabled, options.proxy_enabled);
        let (dashboard, _) = watch::channel(initial);
        Ok(Self {
            inner: Arc::new(StoreInner {
                snapshot: RwLock::new(snapshot),
                revision: AtomicU64::new(1),
                dashboard,
                tunnel_enabled: options.tunnel_enabled,
                proxy_enabled: options.proxy_enabled,
                #[cfg(feature = "tunnel")]
                tunnel_path: absolute_path(&options.tunnel_config)?,
                #[cfg(feature = "tunnel")]
                tunnel_runtime: options.tunnel_runtime.clone(),
                #[cfg(feature = "proxy")]
                proxy_path: absolute_path(&options.proxy_config)?,
            }),
        })
    }

    pub(crate) async fn snapshot(&self) -> ConfigSnapshot {
        self.inner.snapshot.read().await.clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Dashboard> {
        self.inner.dashboard.subscribe()
    }

    #[cfg(feature = "tunnel")]
    pub(crate) fn tunnel_runtime(&self) -> Option<&gaterust_tunnel::TunnelRuntime> {
        self.inner.tunnel_runtime.as_ref()
    }

    pub(crate) fn watched_paths(&self) -> Vec<(ConfigKind, PathBuf)> {
        vec![
            #[cfg(feature = "tunnel")]
            (ConfigKind::Tunnel, self.inner.tunnel_path.clone()),
            #[cfg(feature = "proxy")]
            (ConfigKind::Proxy, self.inner.proxy_path.clone()),
        ]
    }

    #[cfg(feature = "tunnel")]
    pub(crate) async fn save_tunnel(
        &self,
        config: gaterust_tunnel::ServerConfig,
    ) -> Result<gaterust_tunnel::ServerConfig> {
        let path = self.inner.tunnel_path.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            write_validated(&path, &config, gaterust_tunnel::ServerConfig::load)
        })
        .await
        .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))??;
        self.update_tunnel(Some(loaded.clone())).await;
        Ok(loaded)
    }

    #[cfg(feature = "proxy")]
    pub(crate) async fn save_proxy(
        &self,
        config: gaterust_proxy::ProxyConfig,
    ) -> Result<gaterust_proxy::ProxyConfig> {
        let path = self.inner.proxy_path.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            write_validated(&path, &config, gaterust_proxy::ProxyConfig::load)
        })
        .await
        .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))??;
        self.update_proxy(Some(loaded.clone())).await;
        Ok(loaded)
    }

    pub(crate) async fn reload(&self, kind: ConfigKind) {
        match kind {
            #[cfg(feature = "tunnel")]
            ConfigKind::Tunnel => {
                let path = self.inner.tunnel_path.clone();
                match tokio::task::spawn_blocking(move || load_optional_tunnel(&path)).await {
                    Ok(Ok(config)) => self.update_tunnel(config).await,
                    Ok(Err(error)) => tracing::warn!(%error, "重新加载隧道配置失败"),
                    Err(error) => tracing::warn!(%error, "隧道配置加载任务失败"),
                }
            }
            #[cfg(not(feature = "tunnel"))]
            ConfigKind::Tunnel => {}
            #[cfg(feature = "proxy")]
            ConfigKind::Proxy => {
                let path = self.inner.proxy_path.clone();
                match tokio::task::spawn_blocking(move || load_optional_proxy(&path)).await {
                    Ok(Ok(config)) => self.update_proxy(config).await,
                    Ok(Err(error)) => tracing::warn!(%error, "重新加载代理配置失败"),
                    Err(error) => tracing::warn!(%error, "代理配置加载任务失败"),
                }
            }
            #[cfg(not(feature = "proxy"))]
            ConfigKind::Proxy => {}
        }
    }

    #[cfg(feature = "tunnel")]
    async fn update_tunnel(&self, config: Option<gaterust_tunnel::ServerConfig>) {
        self.inner.snapshot.write().await.tunnel = config;
        self.publish().await;
    }

    #[cfg(feature = "proxy")]
    async fn update_proxy(&self, config: Option<gaterust_proxy::ProxyConfig>) {
        self.inner.snapshot.write().await.proxy = config;
        self.publish().await;
    }

    async fn publish(&self) {
        let revision = self.inner.revision.fetch_add(1, Ordering::Relaxed) + 1;
        let snapshot = self.inner.snapshot.read().await;
        self.inner.dashboard.send_replace(dashboard_for(
            &snapshot,
            revision,
            self.inner.tunnel_enabled,
            self.inner.proxy_enabled,
        ));
    }
}

fn dashboard_for(
    snapshot: &ConfigSnapshot,
    revision: u64,
    tunnel_enabled: bool,
    proxy_enabled: bool,
) -> Dashboard {
    Dashboard {
        revision,
        tunnel_enabled,
        proxy_enabled,
        #[cfg(feature = "tunnel")]
        groups: snapshot
            .tunnel
            .as_ref()
            .map_or(0, |config| config.groups.len()),
        #[cfg(not(feature = "tunnel"))]
        groups: 0,
        #[cfg(feature = "tunnel")]
        tunnels: snapshot
            .tunnel
            .as_ref()
            .map_or(0, |config| config.tunnels.len()),
        #[cfg(not(feature = "tunnel"))]
        tunnels: 0,
        #[cfg(feature = "proxy")]
        certificates: snapshot
            .proxy
            .as_ref()
            .map_or(0, |config| config.certificates.len()),
        #[cfg(not(feature = "proxy"))]
        certificates: 0,
        #[cfg(feature = "proxy")]
        routes: snapshot
            .proxy
            .as_ref()
            .map_or(0, |config| config.routes.len()),
        #[cfg(not(feature = "proxy"))]
        routes: 0,
    }
}

#[cfg(feature = "tunnel")]
fn load_optional_tunnel(path: &Path) -> Result<Option<gaterust_tunnel::ServerConfig>> {
    match gaterust_tunnel::ServerConfig::load(path) {
        Ok(config) => Ok(Some(config)),
        Err(gaterust_tunnel::TunnelError::ReadConfig { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(ControlError::ReadRuntimeConfig(error.to_string())),
    }
}

#[cfg(feature = "proxy")]
fn load_optional_proxy(path: &Path) -> Result<Option<gaterust_proxy::ProxyConfig>> {
    match gaterust_proxy::ProxyConfig::load(path) {
        Ok(config) => Ok(Some(config)),
        Err(gaterust_proxy::ProxyError::ReadConfig { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(ControlError::ReadRuntimeConfig(error.to_string())),
    }
}

fn write_validated<T, E, F>(path: &Path, config: &T, load: F) -> Result<T>
where
    T: Serialize,
    E: std::fmt::Display,
    F: FnOnce(&Path) -> std::result::Result<T, E>,
{
    let content = toml::to_string_pretty(config)
        .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))?;
    let mut random = [0_u8; 8];
    rand::rng().fill(&mut random);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let temporary = parent.join(format!(
        ".{file_name}.{:016x}.tmp",
        u64::from_ne_bytes(random)
    ));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))?;
        file.write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))?;
        let loaded = load(&temporary)
            .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))?;
        Ok(loaded)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| ControlError::ReadRuntimeConfig(error.to_string()))
}

#[cfg(all(test, feature = "tunnel"))]
mod tests {
    use std::{num::NonZeroU64, str::FromStr as _};

    use gaterust_tunnel::{
        GroupConfig, ServerConfig, ServerQuicConfig, ServerTunnelConfig, TunnelKind,
    };

    use super::*;

    #[tokio::test]
    async fn saves_valid_config_atomically() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let path = directory.path().join("server.toml");
        let options = crate::ControlOptions {
            tunnel_enabled: true,
            proxy_enabled: false,
            tunnel_config: path.clone(),
            tunnel_runtime: None,
            #[cfg(feature = "proxy")]
            proxy_config: directory.path().join("proxy.toml"),
        };
        let store = ConfigStore::new(&options).expect("创建配置存储");
        let config = ServerConfig {
            quic: ServerQuicConfig {
                bind: "127.0.0.1:2333".parse().expect("测试地址有效"),
                certificate: "server.pem".into(),
                private_key: "server-key.pem".into(),
            },
            groups: vec![GroupConfig {
                name: "office".into(),
                key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            }],
            tunnels: vec![ServerTunnelConfig {
                name: "ssh".into(),
                group: "office".into(),
                kind: TunnelKind::Tcp,
                bind: "127.0.0.1:22022".parse().expect("测试地址有效"),
                limit_bps: NonZeroU64::from_str("1024").ok(),
                max_connections: 8,
                max_udp_sessions: 8,
                udp_idle_seconds: 30,
            }],
        };
        store.save_tunnel(config).await.expect("保存有效配置");
        let loaded = ServerConfig::load(&path).expect("读取保存的配置");
        assert_eq!(loaded.tunnels.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(path)
                    .expect("读取权限")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
