//! `GateRust` QUIC 内网穿透核心。

mod client;
mod config;
mod error;
mod protocol;
mod rate_limit;
mod relay;
mod server;
mod tls;
mod watcher;

pub use client::{run_client, run_client_with_shutdown};
pub use config::{ClientConfig, ServerConfig, TunnelKind};
pub use error::{Result, TunnelError};
pub use server::{run_server, run_server_with_shutdown};

#[cfg(test)]
mod integration_tests;
