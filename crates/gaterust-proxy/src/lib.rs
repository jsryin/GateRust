//! `GateRust` 反向代理与自动证书模块。

mod acme;
mod cache;
mod cloudflare;
mod config;
mod error;
mod proxy;
mod router;
mod server;
mod tls;
mod watcher;

pub use config::{
    AcmeChallenge, CertificateConfig, CertificateIssuer, ProxyConfig, ProxyListenerConfig,
    RouteConfig,
};
pub use error::{ProxyError, Result};
pub use server::{run_proxy, run_proxy_with_shutdown};

#[cfg(test)]
mod integration_tests;
