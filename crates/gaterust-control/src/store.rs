#[cfg(any(feature = "tunnel", feature = "proxy"))]
use std::sync::Mutex;
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
    #[cfg(feature = "tunnel")]
    Tunnel,
    #[cfg(feature = "proxy")]
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
    tunnel_writer: Mutex<()>,
    #[cfg(feature = "tunnel")]
    tunnel_runtime: Option<gaterust_tunnel::TunnelRuntime>,
    #[cfg(feature = "proxy")]
    proxy_path: PathBuf,
    #[cfg(feature = "proxy")]
    proxy_writer: Mutex<()>,
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
                tunnel_writer: Mutex::new(()),
                #[cfg(feature = "tunnel")]
                tunnel_runtime: options.tunnel_runtime.clone(),
                #[cfg(feature = "proxy")]
                proxy_path: absolute_path(&options.proxy_config)?,
                #[cfg(feature = "proxy")]
                proxy_writer: Mutex::new(()),
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
    pub(crate) async fn set_tunnel_quic(
        &self,
        quic: gaterust_tunnel::ServerQuicConfig,
    ) -> Result<gaterust_tunnel::ServerConfig> {
        self.mutate_tunnel(move |config| {
            config.quic = quic;
            Ok(())
        })
        .await
    }

    #[cfg(feature = "tunnel")]
    pub(crate) async fn create_group(
        &self,
        group: gaterust_tunnel::GroupConfig,
    ) -> Result<gaterust_tunnel::ServerConfig> {
        self.mutate_tunnel(move |config| {
            config.groups.push(group);
            Ok(())
        })
        .await
    }

    #[cfg(feature = "tunnel")]
    pub(crate) async fn update_group(
        &self,
        original_name: String,
        group: gaterust_tunnel::GroupConfig,
    ) -> Result<gaterust_tunnel::ServerConfig> {
        self.mutate_tunnel(move |config| {
            let Some(current) = config
                .groups
                .iter_mut()
                .find(|current| current.name == original_name)
            else {
                return Err(resource_not_found("分组", &original_name));
            };
            let renamed = current.name != group.name;
            *current = group;
            if renamed {
                for tunnel in &mut config.tunnels {
                    if tunnel.group == original_name {
                        tunnel.group.clone_from(&current.name);
                    }
                }
            }
            Ok(())
        })
        .await
    }

    #[cfg(feature = "tunnel")]
    pub(crate) async fn delete_group(&self, name: String) -> Result<gaterust_tunnel::ServerConfig> {
        self.mutate_tunnel(move |config| {
            remove_named(&mut config.groups, &name, "分组", |group| &group.name)?;
            config.tunnels.retain(|tunnel| tunnel.group != name);
            Ok(())
        })
        .await
    }

    #[cfg(feature = "tunnel")]
    pub(crate) async fn create_tunnel(
        &self,
        tunnel: gaterust_tunnel::ServerTunnelConfig,
    ) -> Result<gaterust_tunnel::ServerConfig> {
        self.mutate_tunnel(move |config| {
            config.tunnels.push(tunnel);
            Ok(())
        })
        .await
    }

    #[cfg(feature = "tunnel")]
    pub(crate) async fn update_tunnel_config(
        &self,
        original_name: String,
        tunnel: gaterust_tunnel::ServerTunnelConfig,
    ) -> Result<gaterust_tunnel::ServerConfig> {
        self.mutate_tunnel(move |config| {
            replace_named(
                &mut config.tunnels,
                &original_name,
                tunnel,
                "隧道",
                |tunnel| &tunnel.name,
            )
        })
        .await
    }

    #[cfg(feature = "tunnel")]
    pub(crate) async fn delete_tunnel_config(
        &self,
        name: String,
    ) -> Result<gaterust_tunnel::ServerConfig> {
        self.mutate_tunnel(move |config| {
            remove_named(&mut config.tunnels, &name, "隧道", |tunnel| &tunnel.name)
        })
        .await
    }

    #[cfg(feature = "proxy")]
    pub(crate) async fn set_proxy_listener(
        &self,
        listener: gaterust_proxy::ProxyListenerConfig,
    ) -> Result<gaterust_proxy::ProxyConfig> {
        self.mutate_proxy(move |config| {
            config.proxy = listener;
            Ok(())
        })
        .await
    }

    #[cfg(feature = "proxy")]
    pub(crate) async fn create_certificate(
        &self,
        certificate: gaterust_proxy::CertificateConfig,
    ) -> Result<gaterust_proxy::ProxyConfig> {
        self.mutate_proxy(move |config| {
            config.certificates.push(certificate);
            Ok(())
        })
        .await
    }

    #[cfg(feature = "proxy")]
    pub(crate) async fn update_certificate(
        &self,
        original_name: String,
        certificate: gaterust_proxy::CertificateConfig,
    ) -> Result<gaterust_proxy::ProxyConfig> {
        self.mutate_proxy(move |config| {
            let Some(current) = config
                .certificates
                .iter_mut()
                .find(|current| current.name == original_name)
            else {
                return Err(resource_not_found("证书", &original_name));
            };
            let renamed = current.name != certificate.name;
            *current = certificate;
            if renamed {
                for route in &mut config.routes {
                    if route.certificate.as_deref() == Some(&original_name) {
                        route.certificate.clone_from(&Some(current.name.clone()));
                    }
                }
            }
            Ok(())
        })
        .await
    }

    #[cfg(feature = "proxy")]
    pub(crate) async fn delete_certificate(
        &self,
        name: String,
    ) -> Result<gaterust_proxy::ProxyConfig> {
        self.mutate_proxy(move |config| {
            remove_named(&mut config.certificates, &name, "证书", |certificate| {
                &certificate.name
            })?;
            for route in &mut config.routes {
                if route.certificate.as_deref() == Some(&name) {
                    route.certificate = None;
                }
            }
            Ok(())
        })
        .await
    }

    #[cfg(feature = "proxy")]
    pub(crate) async fn create_route(
        &self,
        route: gaterust_proxy::RouteConfig,
    ) -> Result<gaterust_proxy::ProxyConfig> {
        self.mutate_proxy(move |config| {
            config.routes.push(route);
            Ok(())
        })
        .await
    }

    #[cfg(feature = "proxy")]
    pub(crate) async fn update_route(
        &self,
        original_name: String,
        route: gaterust_proxy::RouteConfig,
    ) -> Result<gaterust_proxy::ProxyConfig> {
        self.mutate_proxy(move |config| {
            replace_named(
                &mut config.routes,
                &original_name,
                route,
                "路由",
                |route| &route.name,
            )
        })
        .await
    }

    #[cfg(feature = "proxy")]
    pub(crate) async fn delete_route(&self, name: String) -> Result<gaterust_proxy::ProxyConfig> {
        self.mutate_proxy(move |config| {
            remove_named(&mut config.routes, &name, "路由", |route| &route.name)
        })
        .await
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
            #[cfg(feature = "proxy")]
            ConfigKind::Proxy => {
                let path = self.inner.proxy_path.clone();
                match tokio::task::spawn_blocking(move || load_optional_proxy(&path)).await {
                    Ok(Ok(config)) => self.update_proxy(config).await,
                    Ok(Err(error)) => tracing::warn!(%error, "重新加载代理配置失败"),
                    Err(error) => tracing::warn!(%error, "代理配置加载任务失败"),
                }
            }
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

    #[cfg(feature = "tunnel")]
    async fn mutate_tunnel<F>(&self, mutate: F) -> Result<gaterust_tunnel::ServerConfig>
    where
        F: FnOnce(&mut gaterust_tunnel::ServerConfig) -> Result<()> + Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        let loaded = tokio::task::spawn_blocking(move || {
            // 串行执行“读取最新文件、合并单项、原子写入”，避免旧页面快照覆盖其它改动。
            let _guard = inner
                .tunnel_writer
                .lock()
                .map_err(|_| ControlError::WriteRuntimeConfig("隧道配置写入锁已损坏".into()))?;
            let mut config =
                load_optional_tunnel(&inner.tunnel_path)?.unwrap_or_else(default_tunnel);
            mutate(&mut config)?;
            write_validated(
                &inner.tunnel_path,
                &config,
                gaterust_tunnel::ServerConfig::load,
            )
        })
        .await
        .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))??;
        self.update_tunnel(Some(loaded.clone())).await;
        Ok(loaded)
    }

    #[cfg(feature = "proxy")]
    async fn mutate_proxy<F>(&self, mutate: F) -> Result<gaterust_proxy::ProxyConfig>
    where
        F: FnOnce(&mut gaterust_proxy::ProxyConfig) -> Result<()> + Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        let loaded = tokio::task::spawn_blocking(move || {
            // 文件锁仅存在于阻塞任务内，不跨异步等待持有。
            let _guard = inner
                .proxy_writer
                .lock()
                .map_err(|_| ControlError::WriteRuntimeConfig("代理配置写入锁已损坏".into()))?;
            let mut config = load_optional_proxy(&inner.proxy_path)?.unwrap_or_else(default_proxy);
            mutate(&mut config)?;
            write_validated(
                &inner.proxy_path,
                &config,
                gaterust_proxy::ProxyConfig::load,
            )
        })
        .await
        .map_err(|error| ControlError::WriteRuntimeConfig(error.to_string()))??;
        self.update_proxy(Some(loaded.clone())).await;
        Ok(loaded)
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

#[cfg(feature = "tunnel")]
fn default_tunnel() -> gaterust_tunnel::ServerConfig {
    gaterust_tunnel::ServerConfig {
        quic: gaterust_tunnel::ServerQuicConfig {
            bind: std::net::SocketAddr::from(([0, 0, 0, 0], 2333)),
            certificate: "/etc/gaterust/tunnel/server.pem".into(),
            private_key: "/etc/gaterust/tunnel/server-key.pem".into(),
        },
        groups: Vec::new(),
        tunnels: Vec::new(),
    }
}

#[cfg(feature = "proxy")]
fn default_proxy() -> gaterust_proxy::ProxyConfig {
    gaterust_proxy::ProxyConfig {
        proxy: gaterust_proxy::ProxyListenerConfig {
            http_bind: std::net::SocketAddr::from(([0, 0, 0, 0], 80)),
            https_bind: std::net::SocketAddr::from(([0, 0, 0, 0], 443)),
            cache_dir: "/var/lib/gaterust/proxy/acme".into(),
            max_connections: 2_048,
        },
        certificates: Vec::new(),
        routes: Vec::new(),
    }
}

#[cfg(any(feature = "tunnel", feature = "proxy"))]
fn replace_named<T, F>(
    items: &mut [T],
    original_name: &str,
    replacement: T,
    kind: &'static str,
    name: F,
) -> Result<()>
where
    F: Fn(&T) -> &str,
{
    let Some(current) = items
        .iter_mut()
        .find(|current| name(current) == original_name)
    else {
        return Err(resource_not_found(kind, original_name));
    };
    *current = replacement;
    Ok(())
}

#[cfg(any(feature = "tunnel", feature = "proxy"))]
fn remove_named<T, F>(items: &mut Vec<T>, target: &str, kind: &'static str, name: F) -> Result<()>
where
    F: Fn(&T) -> &str,
{
    let original_len = items.len();
    items.retain(|item| name(item) != target);
    if items.len() == original_len {
        return Err(resource_not_found(kind, target));
    }
    Ok(())
}

#[cfg(any(feature = "tunnel", feature = "proxy"))]
fn resource_not_found(kind: &'static str, name: &str) -> ControlError {
    ControlError::ResourceNotFound {
        kind,
        name: name.into(),
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
mod tunnel_tests {
    use std::{num::NonZeroU64, str::FromStr as _};

    use gaterust_tunnel::{
        GroupConfig, ServerConfig, ServerQuicConfig, ServerTunnelConfig, TunnelKind,
    };

    use super::*;

    #[tokio::test]
    async fn tunnel_mutations_only_replace_target_resource() {
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
        let quic = ServerQuicConfig {
            bind: "127.0.0.1:2333".parse().expect("测试地址有效"),
            certificate: directory.path().join("server.pem"),
            private_key: directory.path().join("server-key.pem"),
        };
        store
            .set_tunnel_quic(quic.clone())
            .await
            .expect("保存 QUIC 配置");
        store
            .create_group(GroupConfig {
                name: "office".into(),
                key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            })
            .await
            .expect("创建分组");
        store
            .create_tunnel(ServerTunnelConfig {
                name: "ssh".into(),
                group: "office".into(),
                kind: TunnelKind::Tcp,
                bind: "127.0.0.1:22022".parse().expect("测试地址有效"),
                limit_bps: NonZeroU64::from_str("1024").ok(),
                max_connections: 8,
                max_udp_sessions: 8,
                udp_idle_seconds: 30,
            })
            .await
            .expect("创建隧道");
        let updated = store
            .update_group(
                "office".into(),
                GroupConfig {
                    name: "home".into(),
                    key: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".into(),
                },
            )
            .await
            .expect("重命名分组");

        assert_eq!(updated.quic, quic);
        assert_eq!(updated.groups.len(), 1);
        assert_eq!(updated.groups[0].name, "home");
        assert_eq!(updated.tunnels.len(), 1);
        assert_eq!(updated.tunnels[0].group, "home");
        let loaded = ServerConfig::load(&path).expect("读取保存的配置");
        assert_eq!(loaded.quic, quic);
        assert_eq!(loaded.tunnels[0].group, "home");
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

#[cfg(all(test, feature = "proxy"))]
mod proxy_tests {
    use gaterust_proxy::{
        AcmeChallenge, CertificateConfig, CertificateIssuer, ProxyConfig, ProxyListenerConfig,
        RouteConfig,
    };

    use super::*;

    #[tokio::test]
    async fn proxy_mutations_only_replace_target_resource() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let path = directory.path().join("proxy.toml");
        let options = crate::ControlOptions {
            tunnel_enabled: false,
            proxy_enabled: true,
            #[cfg(feature = "tunnel")]
            tunnel_config: directory.path().join("server.toml"),
            #[cfg(feature = "tunnel")]
            tunnel_runtime: None,
            proxy_config: path.clone(),
        };
        let store = ConfigStore::new(&options).expect("创建配置存储");
        let listener = ProxyListenerConfig {
            http_bind: "127.0.0.1:8080".parse().expect("测试地址有效"),
            https_bind: "127.0.0.1:8443".parse().expect("测试地址有效"),
            cache_dir: directory.path().join("acme"),
            max_connections: 64,
        };
        store
            .set_proxy_listener(listener.clone())
            .await
            .expect("保存代理监听");
        store
            .create_certificate(certificate("site"))
            .await
            .expect("创建证书");
        store
            .create_route(RouteConfig {
                name: "web".into(),
                host: "example.com".into(),
                path_prefix: "/".into(),
                upstream: "http://127.0.0.1:3000".into(),
                certificate: Some("site".into()),
            })
            .await
            .expect("创建路由");
        let updated = store
            .update_certificate("site".into(), certificate("primary"))
            .await
            .expect("重命名证书");

        assert_eq!(updated.proxy, listener);
        assert_eq!(updated.certificates[0].name, "primary");
        assert_eq!(updated.routes[0].certificate.as_deref(), Some("primary"));

        let updated = store
            .delete_certificate("primary".into())
            .await
            .expect("删除证书");
        assert_eq!(updated.proxy, listener);
        assert!(updated.certificates.is_empty());
        assert_eq!(updated.routes[0].certificate, None);
        let loaded = ProxyConfig::load(&path).expect("读取保存的配置");
        assert_eq!(loaded.proxy, listener);
        assert_eq!(loaded.routes.len(), 1);
    }

    fn certificate(name: &str) -> CertificateConfig {
        CertificateConfig {
            name: name.into(),
            domains: vec!["example.com".into()],
            email: "admin@example.com".into(),
            issuer: CertificateIssuer::LetsEncrypt,
            challenge: AcmeChallenge::TlsAlpn01,
            production: false,
            cloudflare_api_token: None,
            cloudflare_zone_id: None,
            google_eab_key_id: None,
            google_eab_hmac_key: None,
            dns_propagation_seconds: 30,
        }
    }
}
