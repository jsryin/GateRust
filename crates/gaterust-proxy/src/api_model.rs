use serde::Serialize;

use crate::{
    AcmeEnvironment, AcmeProvider, CertificateConfig, DnsProvider, KeyAlgorithm, ProxyConfig,
    ProxyListenerConfig, RouteConfig,
};

#[derive(Clone, Serialize)]
pub struct ProxyConfigView {
    pub proxy: ProxyListenerConfig,
    pub acme_accounts: Vec<AcmeAccountView>,
    pub dns_accounts: Vec<DnsAccountView>,
    pub certificates: Vec<CertificateConfig>,
    pub routes: Vec<RouteConfig>,
}

#[derive(Clone, Serialize)]
pub struct AcmeAccountView {
    pub id: String,
    pub name: String,
    pub provider: AcmeProvider,
    pub environment: AcmeEnvironment,
    pub email: String,
    pub key_algorithm: KeyAlgorithm,
    pub eab_key_id: Option<String>,
    pub eab_hmac_key_configured: bool,
}

#[derive(Clone, Serialize)]
pub struct DnsAccountView {
    pub id: String,
    pub name: String,
    pub provider: DnsProvider,
    pub api_token_configured: bool,
    pub access_key_configured: bool,
    pub secret_key_configured: bool,
}

impl From<&ProxyConfig> for ProxyConfigView {
    fn from(config: &ProxyConfig) -> Self {
        Self {
            proxy: config.proxy.clone(),
            acme_accounts: config
                .acme_accounts
                .iter()
                .map(|account| AcmeAccountView {
                    id: account.id.clone(),
                    name: account.name.clone(),
                    provider: account.provider,
                    environment: account.environment,
                    email: account.email.clone(),
                    key_algorithm: account.key_algorithm,
                    eab_key_id: account.eab_key_id.clone(),
                    eab_hmac_key_configured: account.eab_hmac_key.is_some(),
                })
                .collect(),
            dns_accounts: config
                .dns_accounts
                .iter()
                .map(|account| DnsAccountView {
                    id: account.id.clone(),
                    name: account.name.clone(),
                    provider: account.provider,
                    api_token_configured: account.api_token.is_some(),
                    access_key_configured: account.access_key.is_some(),
                    secret_key_configured: account.secret_key.is_some(),
                })
                .collect(),
            certificates: config.certificates.clone(),
            routes: config.routes.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf};

    use crate::{
        AcmeAccountConfig, AcmeEnvironment, AcmeProvider, DnsAccountConfig, DnsProvider,
        KeyAlgorithm, SecretString,
    };

    use super::*;

    #[test]
    fn serialized_view_never_contains_secrets() {
        let config = ProxyConfig {
            proxy: ProxyListenerConfig {
                http_bind: SocketAddr::from(([127, 0, 0, 1], 80)),
                https_bind: SocketAddr::from(([127, 0, 0, 1], 443)),
                cache_dir: PathBuf::from("cache"),
                max_connections: 16,
            },
            acme_accounts: vec![AcmeAccountConfig {
                id: "acme".into(),
                name: "Google".into(),
                provider: AcmeProvider::GoogleCloud,
                environment: AcmeEnvironment::Staging,
                email: "admin@example.com".into(),
                key_algorithm: KeyAlgorithm::Ec256,
                eab_key_id: Some("key-id".into()),
                eab_hmac_key: Some(SecretString::new("eab-secret".into())),
            }],
            dns_accounts: vec![DnsAccountConfig {
                id: "dns".into(),
                name: "Cloudflare".into(),
                provider: DnsProvider::Cloudflare,
                api_token: Some(SecretString::new("dns-secret".into())),
                access_key: None,
                secret_key: None,
            }],
            certificates: Vec::new(),
            routes: Vec::new(),
        };
        let json = serde_json::to_string(&ProxyConfigView::from(&config)).expect("序列化视图");
        assert!(!json.contains("eab-secret"));
        assert!(!json.contains("dns-secret"));
        assert!(json.contains("eab_hmac_key_configured"));
        assert!(json.contains("api_token_configured"));
    }
}
