use std::{collections::HashMap, sync::Arc};

use quinn::Connection;
use tokio::sync::RwLock;

use crate::{config::TunnelKind, protocol::ServiceDeclaration};

#[derive(Clone)]
pub(super) struct ClientSession {
    pub id: u64,
    pub connection: Connection,
    services: Arc<RwLock<HashMap<String, TunnelKind>>>,
}

impl ClientSession {
    pub fn new(id: u64, connection: Connection, services: Vec<ServiceDeclaration>) -> Self {
        Self {
            id,
            connection,
            services: Arc::new(RwLock::new(service_map(services))),
        }
    }

    pub async fn update_services(&self, services: Vec<ServiceDeclaration>) {
        *self.services.write().await = service_map(services);
    }

    async fn provides(&self, service: &str, kind: TunnelKind) -> bool {
        self.services.read().await.get(service) == Some(&kind)
    }
}

#[derive(Default)]
pub(super) struct SessionRegistry {
    sessions: RwLock<HashMap<String, ClientSession>>,
}

impl SessionRegistry {
    pub async fn insert(&self, group: String, session: ClientSession) -> Option<ClientSession> {
        self.sessions.write().await.insert(group, session)
    }

    pub async fn remove(&self, group: &str, id: u64) {
        let mut sessions = self.sessions.write().await;
        if sessions.get(group).is_some_and(|session| session.id == id) {
            sessions.remove(group);
        }
    }

    pub async fn find(
        &self,
        group: &str,
        service: &str,
        kind: TunnelKind,
    ) -> Option<ClientSession> {
        let session = self.sessions.read().await.get(group).cloned();
        match session {
            Some(session) if session.provides(service, kind).await => Some(session),
            _ => None,
        }
    }
}

fn service_map(services: Vec<ServiceDeclaration>) -> HashMap<String, TunnelKind> {
    services
        .into_iter()
        .map(|service| (service.name, service.kind))
        .collect()
}
