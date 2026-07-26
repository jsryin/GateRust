use std::{
    collections::HashMap,
    fmt,
    io::Cursor,
    sync::{Arc, RwLock},
};

use crate::{ProxyError, Result, config::wildcard_matches};
use rustls::{
    ServerConfig,
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};

#[derive(Clone)]
pub(crate) struct CertificateResolver {
    inner: Arc<RwLock<ResolverState>>,
}

#[derive(Default)]
struct ResolverState {
    exact: HashMap<String, ResolverEntry>,
    wildcard: HashMap<String, ResolverEntry>,
}

#[derive(Clone)]
struct ResolverEntry {
    owner: String,
    resolver: Arc<dyn ResolvesServerCert>,
}

impl CertificateResolver {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ResolverState::default())),
        }
    }

    pub(crate) fn server_config(&self) -> Arc<ServerConfig> {
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(self.clone()));
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(config)
    }

    pub(crate) fn replace_direct(
        &self,
        owner: &str,
        domains: &[String],
        resolver: &Arc<DirectResolver>,
    ) {
        let mut state = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 在同一写锁内移除旧域名并安装新解析器，避免配置更新时出现无证书窗口。
        state.exact.retain(|_, entry| entry.owner != owner);
        state.wildcard.retain(|_, entry| entry.owner != owner);
        for domain in domains {
            let entry = ResolverEntry {
                owner: owner.into(),
                resolver: resolver.clone(),
            };
            if let Some(suffix) = domain.strip_prefix("*.") {
                state.wildcard.insert(suffix.into(), entry);
            } else {
                state.exact.insert(domain.clone(), entry);
            }
        }
    }

    pub(crate) fn remove(&self, owner: &str) {
        let mut state = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.exact.retain(|_, entry| entry.owner != owner);
        state.wildcard.retain(|_, entry| entry.owner != owner);
    }

    fn resolver_for(&self, server_name: &str) -> Option<Arc<dyn ResolvesServerCert>> {
        let state = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .exact
            .get(server_name)
            .or_else(|| {
                state
                    .wildcard
                    .iter()
                    .filter(|(suffix, _)| wildcard_matches(server_name, suffix))
                    .max_by_key(|(suffix, _)| suffix.len())
                    .map(|(_, entry)| entry)
            })
            .map(|entry| Arc::clone(&entry.resolver))
    }
}

impl fmt::Debug for CertificateResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CertificateResolver")
            .finish_non_exhaustive()
    }
}

impl ResolvesServerCert for CertificateResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.resolver_for(client_hello.server_name()?)?
            .resolve(client_hello)
    }
}

#[derive(Debug, Default)]
pub(crate) struct DirectResolver {
    certificate: RwLock<Option<Arc<CertifiedKey>>>,
}

impl DirectResolver {
    pub(crate) fn set_pem(&self, certificate_pem: &[u8], private_key_pem: &[u8]) -> Result<()> {
        let certificates = rustls_pemfile::certs(&mut Cursor::new(certificate_pem))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if certificates.is_empty() {
            return Err(ProxyError::Tls("证书链为空".into()));
        }
        let private_key = rustls_pemfile::private_key(&mut Cursor::new(private_key_pem))?
            .ok_or_else(|| ProxyError::Tls("未找到私钥".into()))?;
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)
            .map_err(|error| ProxyError::Tls(format!("私钥格式不受支持: {error}")))?;
        let certified = Arc::new(CertifiedKey::new(certificates, signing_key));
        certified
            .keys_match()
            .map_err(|error| ProxyError::Tls(format!("证书与私钥不匹配: {error}")))?;
        *self
            .certificate
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(certified);
        Ok(())
    }
}

impl ResolvesServerCert for DirectResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.certificate
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_owner_swaps_domain_mappings_atomically() {
        let resolver = CertificateResolver::new();
        let first = Arc::new(DirectResolver::default());
        resolver.replace_direct("site", &["old.example.com".into()], &first);
        assert!(resolver.resolver_for("old.example.com").is_some());

        let replacement = Arc::new(DirectResolver::default());
        resolver.replace_direct("site", &["new.example.com".into()], &replacement);
        assert!(resolver.resolver_for("old.example.com").is_none());
        assert!(resolver.resolver_for("new.example.com").is_some());
    }
}
