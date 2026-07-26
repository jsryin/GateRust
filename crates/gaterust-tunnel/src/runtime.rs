use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use quinn::Connection;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::{RwLock, watch};
use zeroize::Zeroize as _;

use crate::{
    client::{ClientTunnel, ClientTunnelState},
    close::ApplicationCloseCode,
    config::{GroupSecret, ServerConfig, ServerTunnelConfig, TunnelKind},
    protocol::ServiceDeclaration,
};

const MAX_ONLINE_CLIENTS: usize = 128;

#[derive(Clone)]
pub struct TunnelRuntime {
    state: Arc<RwLock<RuntimeState>>,
    revision: watch::Sender<u64>,
    config_status: watch::Sender<ConfigApplyState>,
}

#[derive(Serialize)]
pub struct TunnelRuntimeSnapshot {
    pub clients: Vec<RuntimeClient>,
    pub tunnels: Vec<RuntimeTunnel>,
    pub config_status: TunnelConfigStatus,
}

#[derive(Clone, Serialize)]
pub struct TunnelConfigStatus {
    pub revision: u64,
    pub restart_required: bool,
    pub last_apply_error: Option<String>,
}

#[derive(Serialize)]
pub struct RuntimeClient {
    pub session_id: u64,
    pub device_id: String,
    pub group: String,
    pub remote_address: SocketAddr,
    pub connected_at: u64,
}

#[derive(Serialize)]
pub struct RuntimeTunnel {
    pub name: String,
    pub owner_session_id: Option<u64>,
}

#[derive(Clone)]
pub(crate) struct ClientSession {
    pub(crate) connection: Connection,
}

pub(crate) enum RegisterError {
    DeviceIdConflict,
    Capacity,
    CredentialsChanged,
}

pub(crate) struct ServiceUpdate {
    pub(crate) declared_services: usize,
    pub(crate) active_tunnels: usize,
    pub(crate) claimed_tunnels: usize,
    pub(crate) released_tunnels: usize,
    pub(crate) changed: bool,
}

#[derive(Default)]
struct RuntimeState {
    credentials: HashMap<String, [u8; 32]>,
    sessions: HashMap<u64, SessionEntry>,
    tunnels: HashMap<String, TunnelSpec>,
    owners: HashMap<String, u64>,
}

struct SessionEntry {
    connection: Connection,
    device_id: String,
    group: String,
    credential_digest: [u8; 32],
    remote_address: SocketAddr,
    connected_at: u64,
    services: HashMap<String, TunnelKind>,
}

struct TunnelSpec {
    group: String,
    kind: TunnelKind,
    bind: SocketAddr,
    local_port: Option<u16>,
}

#[derive(Clone)]
struct ConfigApplyState {
    fingerprint: Option<[u8; 32]>,
    status: TunnelConfigStatus,
}

impl Default for TunnelRuntime {
    fn default() -> Self {
        let (revision, _) = watch::channel(0);
        let (config_status, _) = watch::channel(ConfigApplyState {
            fingerprint: None,
            status: TunnelConfigStatus {
                revision: 0,
                restart_required: false,
                last_apply_error: None,
            },
        });
        Self {
            state: Arc::new(RwLock::new(RuntimeState::default())),
            revision,
            config_status,
        }
    }
}

impl TunnelRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self) -> TunnelRuntimeSnapshot {
        let state = self.state.read().await;
        let mut clients = state
            .sessions
            .iter()
            .map(|(&session_id, session)| RuntimeClient {
                session_id,
                device_id: session.device_id.clone(),
                group: session.group.clone(),
                remote_address: session.remote_address,
                connected_at: session.connected_at,
            })
            .collect::<Vec<_>>();
        clients.sort_unstable_by_key(|client| client.session_id);

        let mut tunnels = state
            .tunnels
            .keys()
            .map(|name| RuntimeTunnel {
                name: name.clone(),
                owner_session_id: state.owners.get(name).copied(),
            })
            .collect::<Vec<_>>();
        tunnels.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        let config_status = self.config_status.borrow().status.clone();
        TunnelRuntimeSnapshot {
            clients,
            tunnels,
            config_status,
        }
    }

    /// 等待服务端处理指定配置，返回匹配配置的应用结果。
    ///
    /// # Errors
    ///
    /// 配置无法生成稳定指纹时返回错误；超过等待期限时返回 `None`。
    pub async fn wait_for_config(
        &self,
        config: &ServerConfig,
        after_revision: u64,
        timeout: Duration,
    ) -> crate::Result<Option<TunnelConfigStatus>> {
        let fingerprint = config_fingerprint(config)?;
        let mut changes = self.config_status.subscribe();
        let wait = async {
            loop {
                let current = changes.borrow_and_update().clone();
                if current.status.revision != after_revision
                    && current.fingerprint == Some(fingerprint)
                {
                    return Some(current.status);
                }
                if changes.changed().await.is_err() {
                    return None;
                }
            }
        };
        Ok(tokio::time::timeout(timeout, wait).await.unwrap_or(None))
    }

    #[must_use]
    pub fn config_revision(&self) -> u64 {
        self.config_status.borrow().status.revision
    }

    pub async fn disconnect(&self, session_id: u64) -> bool {
        let connection = {
            let mut state = self.state.write().await;
            let Some(session) = state.sessions.remove(&session_id) else {
                return false;
            };
            release_session(&mut state, session_id);
            session.connection
        };
        self.notify();
        connection.close(
            ApplicationCloseCode::AdministratorDisconnect.value(),
            b"disconnected by administrator",
        );
        true
    }

    pub(crate) async fn apply_tunnels(&self, configs: &[ServerTunnelConfig]) {
        let mut state = self.state.write().await;
        state.tunnels = configs
            .iter()
            .map(|config| {
                (
                    config.name.clone(),
                    TunnelSpec {
                        group: config.group.clone(),
                        kind: config.kind,
                        bind: config.bind,
                        local_port: config.client_local_port(),
                    },
                )
            })
            .collect();
        retain_valid_owners(&mut state);
        drop(state);
        self.notify();
    }

    pub(crate) async fn apply_credentials(&self, credentials: &[(String, GroupSecret)]) {
        let credentials = credentials
            .iter()
            .map(|(name, secret)| (name.clone(), credential_digest(secret.as_bytes())))
            .collect::<HashMap<_, _>>();
        let connections = {
            let mut state = self.state.write().await;
            state.credentials = credentials;
            let revoked = state
                .sessions
                .iter()
                .filter(|(_, session)| {
                    state.credentials.get(&session.group) != Some(&session.credential_digest)
                })
                .map(|(&id, _)| id)
                .collect::<Vec<_>>();
            let mut connections = Vec::with_capacity(revoked.len());
            for id in revoked {
                if let Some(session) = state.sessions.remove(&id) {
                    release_session(&mut state, id);
                    connections.push(session.connection);
                }
            }
            connections
        };
        if connections.is_empty() {
            return;
        }
        self.notify();
        for connection in connections {
            connection.close(
                ApplicationCloseCode::CredentialsRevoked.value(),
                b"credentials revoked",
            );
        }
    }

    pub(crate) fn report_config_applied(
        &self,
        config: &ServerConfig,
        restart_required: bool,
    ) -> crate::Result<()> {
        self.publish_config_status(config_fingerprint(config)?, restart_required, None);
        Ok(())
    }

    pub(crate) fn report_config_failed(
        &self,
        config: &ServerConfig,
        restart_required: bool,
        error: String,
    ) -> crate::Result<()> {
        self.publish_config_status(config_fingerprint(config)?, restart_required, Some(error));
        Ok(())
    }

    pub(crate) fn report_config_load_error(&self, error: String) {
        self.config_status.send_modify(|state| {
            state.fingerprint = None;
            state.status.revision = state.status.revision.wrapping_add(1);
            state.status.last_apply_error = Some(error);
        });
    }

    pub(crate) async fn register(
        &self,
        id: u64,
        device_id: String,
        group: String,
        credential_digest: [u8; 32],
        connection: Connection,
        services: Vec<ServiceDeclaration>,
    ) -> Result<(), RegisterError> {
        let mut state = self.state.write().await;
        if state.sessions.len() >= MAX_ONLINE_CLIENTS {
            return Err(RegisterError::Capacity);
        }
        if state.credentials.get(&group) != Some(&credential_digest) {
            return Err(RegisterError::CredentialsChanged);
        }
        if state
            .sessions
            .values()
            .any(|session| session.device_id == device_id)
        {
            return Err(RegisterError::DeviceIdConflict);
        }
        state.sessions.insert(
            id,
            SessionEntry {
                remote_address: connection.remote_address(),
                connection,
                device_id,
                group,
                credential_digest,
                connected_at: unix_timestamp(),
                services: service_map(services),
            },
        );
        claim_available(&mut state, id);
        drop(state);
        self.notify();
        Ok(())
    }

    pub(crate) async fn update_services(
        &self,
        id: u64,
        services: Vec<ServiceDeclaration>,
    ) -> Option<ServiceUpdate> {
        let mut state = self.state.write().await;
        if !state.sessions.contains_key(&id) {
            return None;
        }
        let previously_owned = owned_tunnels(&state, id);
        let services = service_map(services);
        let session = state.sessions.get_mut(&id)?;
        let declarations_changed = session.services != services;
        session.services = services;
        let declared_services = session.services.len();
        retain_valid_owners(&mut state);
        claim_available(&mut state, id);
        let currently_owned = owned_tunnels(&state, id);
        let claimed_tunnels = currently_owned.difference(&previously_owned).count();
        let released_tunnels = previously_owned.difference(&currently_owned).count();
        let changed = declarations_changed || claimed_tunnels != 0 || released_tunnels != 0;
        let update = ServiceUpdate {
            declared_services,
            active_tunnels: currently_owned.len(),
            claimed_tunnels,
            released_tunnels,
            changed,
        };
        drop(state);
        if changed {
            self.notify();
        }
        Some(update)
    }

    pub(crate) async fn unregister(&self, id: u64) {
        let mut state = self.state.write().await;
        if state.sessions.remove(&id).is_none() {
            return;
        }
        release_session(&mut state, id);
        drop(state);
        self.notify();
    }

    pub(crate) async fn find(&self, tunnel: &str) -> Option<ClientSession> {
        let state = self.state.read().await;
        let id = state.owners.get(tunnel)?;
        state.sessions.get(id).map(|session| ClientSession {
            connection: session.connection.clone(),
        })
    }

    pub(crate) async fn catalog(&self, session_id: u64) -> Vec<ClientTunnel> {
        let state = self.state.read().await;
        let Some(session) = state.sessions.get(&session_id) else {
            return Vec::new();
        };
        let mut tunnels = state
            .tunnels
            .iter()
            .filter(|(_, spec)| spec.group == session.group)
            .map(|(name, spec)| {
                let state = match state.owners.get(name) {
                    None => ClientTunnelState::Idle,
                    Some(owner) if *owner == session_id => ClientTunnelState::Enabled,
                    Some(_) => ClientTunnelState::Occupied,
                };
                ClientTunnel {
                    name: name.clone(),
                    kind: spec.kind,
                    server_port: spec.bind.port(),
                    local_port: spec.local_port,
                    state,
                }
            })
            .collect::<Vec<_>>();
        tunnels.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        tunnels
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    fn notify(&self) {
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    fn publish_config_status(
        &self,
        fingerprint: [u8; 32],
        restart_required: bool,
        last_apply_error: Option<String>,
    ) {
        self.config_status.send_modify(|state| {
            state.fingerprint = Some(fingerprint);
            state.status.revision = state.status.revision.wrapping_add(1);
            state.status.restart_required = restart_required;
            state.status.last_apply_error = last_apply_error;
        });
    }
}

pub(crate) fn credential_digest(secret: &[u8]) -> [u8; 32] {
    Sha256::digest(secret).into()
}

fn config_fingerprint(config: &ServerConfig) -> crate::Result<[u8; 32]> {
    let mut serialized = toml::to_string(config).map_err(crate::TunnelError::ConfigFingerprint)?;
    let digest = Sha256::digest(serialized.as_bytes()).into();
    serialized.zeroize();
    Ok(digest)
}

fn retain_valid_owners(state: &mut RuntimeState) {
    let RuntimeState {
        sessions,
        tunnels,
        owners,
        ..
    } = state;
    owners.retain(|name, id| {
        let Some(spec) = tunnels.get(name) else {
            return false;
        };
        sessions
            .get(id)
            .is_some_and(|session| eligible(session, name, spec))
    });
}

fn release_session(state: &mut RuntimeState, session_id: u64) {
    state.owners.retain(|_, owner| *owner != session_id);
}

fn owned_tunnels(state: &RuntimeState, session_id: u64) -> HashSet<String> {
    state
        .owners
        .iter()
        .filter(|&(_, owner)| *owner == session_id)
        .map(|(name, _)| name.clone())
        .collect()
}

fn claim_available(state: &mut RuntimeState, session_id: u64) {
    let Some(session) = state.sessions.get(&session_id) else {
        return;
    };
    let available = state
        .tunnels
        .iter()
        .filter(|&(name, spec)| !state.owners.contains_key(name) && eligible(session, name, spec))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    for name in available {
        state.owners.insert(name, session_id);
    }
}

fn eligible(session: &SessionEntry, tunnel: &str, spec: &TunnelSpec) -> bool {
    session.group == spec.group && session.services.get(tunnel) == Some(&spec.kind)
}

fn service_map(services: Vec<ServiceDeclaration>) -> HashMap<String, TunnelKind> {
    services
        .into_iter()
        .map(|service| (service.name, service.kind))
        .collect()
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::ServerQuicConfig;

    fn config() -> ServerConfig {
        ServerConfig {
            quic: ServerQuicConfig {
                bind: "127.0.0.1:2333".parse().expect("测试地址有效"),
                certificate: PathBuf::from("server.pem"),
                private_key: PathBuf::from("server-key.pem"),
            },
            groups: Vec::new(),
            tunnels: Vec::new(),
        }
    }

    #[tokio::test]
    async fn waits_for_matching_config_application_status() {
        let runtime = TunnelRuntime::new();
        let config = config();
        let revision = runtime.config_revision();
        runtime
            .report_config_applied(&config, true)
            .expect("记录配置状态");

        let status = runtime
            .wait_for_config(&config, revision, Duration::from_millis(50))
            .await
            .expect("生成配置指纹")
            .expect("应匹配配置状态");

        assert!(status.restart_required);
        assert_eq!(status.last_apply_error, None);
    }

    #[tokio::test]
    async fn times_out_for_unseen_config() {
        let runtime = TunnelRuntime::new();
        let mut applied = config();
        runtime
            .report_config_applied(&applied, false)
            .expect("记录配置状态");
        let revision = runtime.config_revision();
        applied.quic.bind = "127.0.0.1:2444".parse().expect("测试地址有效");

        assert!(
            runtime
                .wait_for_config(&applied, revision, Duration::from_millis(10))
                .await
                .expect("生成配置指纹")
                .is_none()
        );
    }

    #[tokio::test]
    async fn load_error_does_not_match_previous_config() {
        let runtime = TunnelRuntime::new();
        let config = config();
        runtime
            .report_config_applied(&config, false)
            .expect("记录配置状态");
        let revision = runtime.config_revision();
        runtime.report_config_load_error("配置文件无效".into());

        assert!(
            runtime
                .wait_for_config(&config, revision, Duration::from_millis(10))
                .await
                .expect("生成配置指纹")
                .is_none()
        );
    }
}
