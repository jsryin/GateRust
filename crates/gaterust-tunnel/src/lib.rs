//! `GateRust` QUIC 内网穿透核心。

use std::path::Path;

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

pub use client::{
    ClientStatus, ClientTunnel, ClientTunnelState, run_client, run_client_with_shutdown,
    run_client_with_status,
};
pub use config::{
    ClientConfig, ClientServerConfig, ClientServiceConfig, GroupConfig, MAX_CLIENT_SERVICES,
    ServerConfig, ServerQuicConfig, ServerTunnelConfig, TunnelKind, generate_group_key,
};
pub use error::{Result, TunnelError};
pub use runtime::{RuntimeClient, RuntimeTunnel, TunnelRuntime, TunnelRuntimeSnapshot};
pub use server::{run_server, run_server_with_runtime, run_server_with_shutdown};

/// 校验服务端配置及其 TLS 证书和私钥。
///
/// # Errors
///
/// 配置无效、证书或私钥不可读或 TLS 凭据不匹配时返回错误。
pub fn check_server_config(path: impl AsRef<Path>) -> Result<()> {
    let config = ServerConfig::load(path.as_ref())?;
    tls::validate_server_credentials(&config.quic)
}

#[cfg(test)]
mod integration_tests;
