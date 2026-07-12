//! `GateRust` QUIC 内网穿透核心。

mod client;
mod config;
mod error;
mod identity;
mod protocol;
mod rate_limit;
mod relay;
mod runtime;
mod server;
mod tls;
mod watcher;

pub use client::{run_client, run_client_with_shutdown};
pub use config::{
    ClientConfig, ClientServerConfig, ClientServiceConfig, GroupConfig, ServerConfig,
    ServerQuicConfig, ServerTunnelConfig, TunnelKind, generate_group_key,
};
pub use error::{Result, TunnelError};
pub use runtime::{RuntimeClient, RuntimeTunnel, TunnelRuntime, TunnelRuntimeSnapshot};
pub use server::{run_server, run_server_with_runtime, run_server_with_shutdown};

#[cfg(test)]
mod integration_tests;
