//! `GateRust` 反向代理与自动证书模块。

mod acme;
mod api_model;
mod cache;
mod config;
mod connection;
mod dns;
mod error;
mod listener;
mod proxy;
mod router;
mod runtime;
mod server;
mod tls;
mod watcher;

pub use acme::ManualDnsRecord;
pub use api_model::{AcmeAccountView, DnsAccountView, ProxyConfigView};
pub use config::{
    AcmeAccountConfig, AcmeEnvironment, AcmeProvider, CertificateConfig, CertificateValidation,
    DnsAccountConfig, DnsProvider, KeyAlgorithm, ProxyConfig, ProxyListenerConfig, RouteConfig,
    SecretString,
};
pub use error::{ProxyError, Result};
pub use runtime::{
    CertificateRuntimeStatus, CertificateStatus, ProxyConfigStatus, ProxyRuntime,
    ProxyRuntimeSnapshot,
};
pub use server::{run_proxy, run_proxy_with_runtime, run_proxy_with_shutdown};

#[cfg(test)]
mod integration_tests;
