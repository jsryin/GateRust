use std::{net::UdpSocket, sync::Arc};

use quinn::Endpoint;
use tokio::sync::watch;

use crate::{Result, config::ServerQuicConfig, tls};

pub(super) struct QuicEndpoint {
    endpoint: Endpoint,
    config: ServerQuicConfig,
    credentials: watch::Sender<Arc<tls::ServerCredentials>>,
}

pub(super) struct QuicUpdate {
    config: ServerQuicConfig,
    credentials: Arc<tls::ServerCredentials>,
    socket: Option<UdpSocket>,
}

impl QuicEndpoint {
    pub(super) fn bind(config: ServerQuicConfig) -> Result<Self> {
        let credentials = Arc::new(tls::server_credentials(&config)?);
        let endpoint = Endpoint::server((*credentials.server_config).clone(), config.bind)?;
        let (credentials, _) = watch::channel(credentials);
        Ok(Self {
            endpoint,
            config,
            credentials,
        })
    }

    pub(super) fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub(super) fn subscribe_credentials(&self) -> watch::Receiver<Arc<tls::ServerCredentials>> {
        self.credentials.subscribe()
    }

    pub(super) fn prepare(&self, config: &ServerQuicConfig) -> Result<Option<QuicUpdate>> {
        if config == &self.config {
            return Ok(None);
        }
        let credentials = Arc::new(tls::server_credentials(config)?);
        let socket = (config.bind != self.config.bind)
            .then(|| UdpSocket::bind(config.bind))
            .transpose()?;
        Ok(Some(QuicUpdate {
            config: config.clone(),
            credentials,
            socket,
        }))
    }

    pub(super) fn apply(&mut self, update: QuicUpdate) -> Result<()> {
        if let Some(socket) = update.socket {
            // quinn 原生换绑会同步迁移现有连接使用的 socket，避免重启端点。
            self.endpoint.rebind(socket)?;
        }
        self.endpoint
            .set_server_config(Some((*update.credentials.server_config).clone()));
        self.credentials.send_replace(update.credentials);
        self.config = update.config;
        Ok(())
    }
}
