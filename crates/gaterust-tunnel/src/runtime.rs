use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use quinn::{Connection, VarInt};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::{
    config::{ServerTunnelConfig, TunnelKind},
    protocol::ServiceDeclaration,
};

const MAX_ONLINE_CLIENTS: usize = 128;
pub(crate) const ADMINISTRATOR_CLOSE_CODE: u32 = 12;

#[derive(Clone, Default)]
pub struct TunnelRuntime {
    state: Arc<RwLock<RuntimeState>>,
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
    pub waiting_session_ids: Vec<u64>,
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
            .iter()
            .map(|(name, spec)| {
                let owner_session_id = state.owners.get(name).copied();
                let mut waiting_session_ids = state
                    .sessions
                    .iter()
                    .filter_map(|(&id, session)| {
                        (Some(id) != owner_session_id && eligible(session, name, spec))
                            .then_some(id)
                    })
                    .collect::<Vec<_>>();
                waiting_session_ids.sort_unstable();
                RuntimeTunnel {
                    name: name.clone(),
                    owner_session_id,
                    waiting_session_ids,
                }
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
            reconcile(&mut state);
            session.connection
        };
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
                    },
                )
            })
            .collect();
        reconcile(&mut state);
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
        reconcile(&mut state);
        Ok(())
    }

    pub(crate) async fn update_services(&self, id: u64, services: Vec<ServiceDeclaration>) {
        let mut state = self.state.write().await;
        if let Some(session) = state.sessions.get_mut(&id) {
            session.services = service_map(services);
            reconcile(&mut state);
        }
    }

    pub(crate) async fn unregister(&self, id: u64) {
        let mut state = self.state.write().await;
        if state.sessions.remove(&id).is_some() {
            reconcile(&mut state);
        }
    }

    pub(crate) async fn find(&self, tunnel: &str) -> Option<ClientSession> {
        let state = self.state.read().await;
        let id = state.owners.get(tunnel)?;
        state.sessions.get(id).map(|session| ClientSession {
            connection: session.connection.clone(),
        })
    }
}

fn reconcile(state: &mut RuntimeState) {
    let previous = std::mem::take(&mut state.owners);
    for (name, id) in previous {
        let Some(spec) = state.tunnels.get(&name) else {
            continue;
        };
        if state
            .sessions
            .get(&id)
            .is_some_and(|session| eligible(session, &name, spec))
        {
            state.owners.insert(name, id);
        }
    }
    for (name, spec) in &state.tunnels {
        if state.owners.contains_key(name) {
            continue;
        }
        if let Some(id) = state
            .sessions
            .iter()
            .filter_map(|(&id, session)| eligible(session, name, spec).then_some(id))
            .min()
        {
            state.owners.insert(name.clone(), id);
        }
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
