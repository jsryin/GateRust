use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use quinn::{Connection, VarInt};
use serde::Serialize;
use tokio::sync::{RwLock, watch};

use crate::{
    client::{ClientTunnel, ClientTunnelState},
    config::{ServerTunnelConfig, TunnelKind},
    protocol::ServiceDeclaration,
};

const MAX_ONLINE_CLIENTS: usize = 128;
pub(crate) const ADMINISTRATOR_CLOSE_CODE: u32 = 12;

#[derive(Clone)]
pub struct TunnelRuntime {
    state: Arc<RwLock<RuntimeState>>,
    revision: watch::Sender<u64>,
}

#[derive(Serialize)]
pub struct TunnelRuntimeSnapshot {
    pub clients: Vec<RuntimeClient>,
    pub tunnels: Vec<RuntimeTunnel>,
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
}

#[derive(Default)]
struct RuntimeState {
    sessions: HashMap<u64, SessionEntry>,
    tunnels: HashMap<String, TunnelSpec>,
    owners: HashMap<String, u64>,
}

struct SessionEntry {
    connection: Connection,
    device_id: String,
    group: String,
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

impl Default for TunnelRuntime {
    fn default() -> Self {
        let (revision, _) = watch::channel(0);
        Self {
            state: Arc::new(RwLock::new(RuntimeState::default())),
            revision,
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
        TunnelRuntimeSnapshot { clients, tunnels }
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
            VarInt::from_u32(ADMINISTRATOR_CLOSE_CODE),
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

    pub(crate) async fn register(
        &self,
        id: u64,
        device_id: String,
        group: String,
        connection: Connection,
        services: Vec<ServiceDeclaration>,
    ) -> Result<(), RegisterError> {
        let mut state = self.state.write().await;
        if state.sessions.len() >= MAX_ONLINE_CLIENTS {
            return Err(RegisterError::Capacity);
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
                connected_at: unix_timestamp(),
                services: service_map(services),
            },
        );
        claim_available(&mut state, id);
        drop(state);
        self.notify();
        Ok(())
    }

    pub(crate) async fn update_services(&self, id: u64, services: Vec<ServiceDeclaration>) {
        let mut state = self.state.write().await;
        let Some(session) = state.sessions.get_mut(&id) else {
            return;
        };
        session.services = service_map(services);
        retain_valid_owners(&mut state);
        claim_available(&mut state, id);
        drop(state);
        self.notify();
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
                    Some(owner) if *owner == session_id => ClientTunnelState::Connected,
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
}

fn retain_valid_owners(state: &mut RuntimeState) {
    let RuntimeState {
        sessions,
        tunnels,
        owners,
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
