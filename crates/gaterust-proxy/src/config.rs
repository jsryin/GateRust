use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
};

use http::Uri;
use serde::{Deserialize, Serialize};

use crate::{ProxyError, Result};

const DEFAULT_MAX_CONNECTIONS: usize = 2_048;
const DEFAULT_DNS_PROPAGATION_SECONDS: u64 = 30;
const GOOGLE_PRODUCTION_DIRECTORY: &str = "https://dv.acme-v02.api.pki.goog/directory";
const GOOGLE_STAGING_DIRECTORY: &str = "https://dv.acme-v02.test-api.pki.goog/directory";

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    pub proxy: ProxyListenerConfig,
    #[serde(default)]
    pub certificates: Vec<CertificateConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyListenerConfig {
    pub http_bind: SocketAddr,
    pub https_bind: SocketAddr,
    pub cache_dir: PathBuf,
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificateIssuer {
    LetsEncrypt,
    GoogleTrustServices,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AcmeChallenge {
    #[serde(rename = "http-01")]
    Http01,
    #[serde(rename = "tls-alpn-01")]
    TlsAlpn01,
    #[serde(rename = "cloudflare-dns-01")]
    CloudflareDns01,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CertificateConfig {
    pub name: String,
    pub domains: Vec<String>,
    pub email: String,
    pub issuer: CertificateIssuer,
    pub challenge: AcmeChallenge,
    #[serde(default)]
    pub production: bool,
    pub cloudflare_api_token: Option<String>,
    pub cloudflare_zone_id: Option<String>,
    pub google_eab_key_id: Option<String>,
    pub google_eab_hmac_key: Option<String>,
    #[serde(default = "default_dns_propagation_seconds")]
    pub dns_propagation_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    pub name: String,
    pub host: String,
    #[serde(default = "default_path_prefix")]
    pub path_prefix: String,
    pub upstream: String,
    pub certificate: Option<String>,
}

impl ProxyConfig {
    /// 读取并验证代理配置，相对缓存路径以配置文件目录为基准。
    ///
    /// # Errors
    ///
    /// 文件不可读、TOML 格式错误或字段不满足约束时返回错误。
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|source| ProxyError::ReadConfig {
            path: path.to_owned(),
            source,
        })?;
        let mut config: Self =
            toml::from_str(&content).map_err(|source| ProxyError::ParseConfig {
                path: path.to_owned(),
                source,
            })?;
        if config.proxy.cache_dir.is_relative() {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            config.proxy.cache_dir = parent.join(&config.proxy.cache_dir);
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&mut self) -> Result<()> {
        if self.proxy.http_bind == self.proxy.https_bind {
            return Err(ProxyError::InvalidConfig(
                "HTTP 与 HTTPS 不能监听同一地址".into(),
            ));
        }
        if self.proxy.max_connections == 0 {
            return Err(ProxyError::InvalidConfig(
                "proxy.max_connections 必须大于 0".into(),
            ));
        }
        if self.certificates.len() > 256 || self.routes.len() > 4_096 {
            return Err(ProxyError::InvalidConfig(
                "证书不能超过 256 个，路由不能超过 4096 条".into(),
            ));
        }

        let mut certificate_names = HashSet::new();
        let mut domain_owners: HashMap<String, String> = HashMap::new();
        for certificate in &mut self.certificates {
            validate_name("证书", &certificate.name)?;
            if !certificate_names.insert(certificate.name.clone()) {
                return Err(ProxyError::InvalidConfig(format!(
                    "证书名称重复: {}",
                    certificate.name
                )));
            }
            certificate.validate(&mut domain_owners)?;
        }

        let mut route_names = HashSet::new();
        let mut route_keys = HashSet::new();
        for route in &mut self.routes {
            validate_name("路由", &route.name)?;
            if !route_names.insert(route.name.as_str()) {
                return Err(ProxyError::InvalidConfig(format!(
                    "路由名称重复: {}",
                    route.name
                )));
            }
            route.host = normalize_domain(&route.host)?;
            validate_path_prefix(&route.path_prefix)?;
            validate_upstream(&route.upstream)?;
            if !route_keys.insert((route.host.as_str(), route.path_prefix.as_str())) {
                return Err(ProxyError::InvalidConfig(format!(
                    "Host 与路径前缀重复: {}{}",
                    route.host, route.path_prefix
                )));
            }
            if let Some(certificate) = &route.certificate {
                if !certificate_names.contains(certificate) {
                    return Err(ProxyError::InvalidConfig(format!(
                        "路由 {} 引用了不存在的证书 {certificate}",
                        route.name
                    )));
                }
                if certificate_for_host(&domain_owners, &route.host) != Some(certificate.as_str()) {
                    return Err(ProxyError::InvalidConfig(format!(
                        "证书 {certificate} 不包含路由域名 {}",
                        route.host
                    )));
                }
            }
        }
        Ok(())
    }
}

impl CertificateConfig {
    fn validate(&mut self, owners: &mut HashMap<String, String>) -> Result<()> {
        if self.domains.is_empty() || self.domains.len() > 100 {
            return Err(ProxyError::InvalidConfig(format!(
                "证书 {} 的域名数量必须为 1..=100",
                self.name
            )));
        }
        if self.email.is_empty() || self.email.len() > 254 || !self.email.contains('@') {
            return Err(ProxyError::InvalidConfig(format!(
                "证书 {} 的联系邮箱无效",
                self.name
            )));
        }
        if self.dns_propagation_seconds == 0 || self.dns_propagation_seconds > 600 {
            return Err(ProxyError::InvalidConfig(format!(
                "证书 {} 的 DNS 传播等待时间必须为 1..=600 秒",
                self.name
            )));
        }
        let mut own_domains = HashSet::new();
        for domain in &mut self.domains {
            *domain = normalize_domain(domain)?;
            if !own_domains.insert(domain.as_str()) {
                return Err(ProxyError::InvalidConfig(format!(
                    "证书 {} 包含重复域名 {domain}",
                    self.name
                )));
            }
            if let Some(owner) = owners.insert(domain.clone(), self.name.clone()) {
                return Err(ProxyError::InvalidConfig(format!(
                    "域名 {domain} 同时属于证书 {owner} 和 {}",
                    self.name
                )));
            }
        }
        match (self.issuer, self.challenge) {
            (CertificateIssuer::LetsEncrypt, AcmeChallenge::Http01 | AcmeChallenge::TlsAlpn01) => {
                reject_present(
                    self,
                    "cloudflare_api_token",
                    self.cloudflare_api_token.as_ref(),
                )?;
                reject_present(self, "cloudflare_zone_id", self.cloudflare_zone_id.as_ref())?;
                reject_present(self, "google_eab_key_id", self.google_eab_key_id.as_ref())?;
                reject_present(
                    self,
                    "google_eab_hmac_key",
                    self.google_eab_hmac_key.as_ref(),
                )?;
            }
            (_, AcmeChallenge::CloudflareDns01) => {
                require_present(
                    self,
                    "cloudflare_api_token",
                    self.cloudflare_api_token.as_ref(),
                )?;
                require_present(self, "cloudflare_zone_id", self.cloudflare_zone_id.as_ref())?;
                if self.issuer == CertificateIssuer::GoogleTrustServices {
                    require_present(self, "google_eab_key_id", self.google_eab_key_id.as_ref())?;
                    require_present(
                        self,
                        "google_eab_hmac_key",
                        self.google_eab_hmac_key.as_ref(),
                    )?;
                } else {
                    reject_present(self, "google_eab_key_id", self.google_eab_key_id.as_ref())?;
                    reject_present(
                        self,
                        "google_eab_hmac_key",
                        self.google_eab_hmac_key.as_ref(),
                    )?;
                }
            }
            (CertificateIssuer::GoogleTrustServices, _) => {
                return Err(ProxyError::InvalidConfig(format!(
                    "证书 {} 使用 Google Trust Services 时必须选择 cloudflare-dns-01",
                    self.name
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn directory_url(&self) -> &'static str {
        match (self.issuer, self.production) {
            (CertificateIssuer::LetsEncrypt, true) => instant_acme::LetsEncrypt::Production.url(),
            (CertificateIssuer::LetsEncrypt, false) => instant_acme::LetsEncrypt::Staging.url(),
            (CertificateIssuer::GoogleTrustServices, true) => GOOGLE_PRODUCTION_DIRECTORY,
            (CertificateIssuer::GoogleTrustServices, false) => GOOGLE_STAGING_DIRECTORY,
        }
    }
}

fn certificate_for_host<'a>(owners: &'a HashMap<String, String>, host: &str) -> Option<&'a str> {
    owners.get(host).map(String::as_str).or_else(|| {
        owners
            .iter()
            .filter_map(|(domain, owner)| {
                let suffix = domain.strip_prefix("*.")?;
                let covered = host == domain
                    || (host.len() > suffix.len()
                        && host.ends_with(suffix)
                        && host.as_bytes()[host.len() - suffix.len() - 1] == b'.');
                covered.then_some((suffix.len(), owner.as_str()))
            })
            .max_by_key(|(length, _)| *length)
            .map(|(_, owner)| owner)
    })
}

fn normalize_domain(value: &str) -> Result<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    let plain = value.strip_prefix("*.").unwrap_or(&value);
    if plain.is_empty()
        || plain.len() > 253
        || !plain.contains('.')
        || plain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(ProxyError::InvalidConfig(format!("域名无效: {value}")));
    }
    Ok(value)
}

fn validate_name(kind: &str, name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ProxyError::InvalidConfig(format!(
            "{kind}名称必须为 1..=64 个 ASCII 字母、数字、- 或 _"
        )));
    }
    Ok(())
}

fn validate_path_prefix(value: &str) -> Result<()> {
    if !value.starts_with('/') || value.contains(['?', '#']) {
        return Err(ProxyError::InvalidConfig(format!(
            "路径前缀必须以 / 开头且不能包含查询或片段: {value}"
        )));
    }
    Ok(())
}

fn validate_upstream(value: &str) -> Result<()> {
    let uri: Uri = value
        .parse()
        .map_err(|_| ProxyError::InvalidConfig(format!("上游 URI 无效: {value}")))?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.authority().is_none() {
        return Err(ProxyError::InvalidConfig(format!(
            "上游 URI 必须包含 http/https scheme 和 authority: {value}"
        )));
    }
    if uri.query().is_some() {
        return Err(ProxyError::InvalidConfig(format!(
            "上游 URI 不能包含查询参数: {value}"
        )));
    }
    Ok(())
}

fn require_present(config: &CertificateConfig, field: &str, value: Option<&String>) -> Result<()> {
    if value.is_none_or(String::is_empty) {
        return Err(ProxyError::InvalidConfig(format!(
            "证书 {} 必须配置 {field}",
            config.name
        )));
    }
    Ok(())
}

fn reject_present(config: &CertificateConfig, field: &str, value: Option<&String>) -> Result<()> {
    if value.is_some() {
        return Err(ProxyError::InvalidConfig(format!(
            "证书 {} 不应配置 {field}",
            config.name
        )));
    }
    Ok(())
}

fn default_path_prefix() -> String {
    "/".into()
}

const fn default_max_connections() -> usize {
    DEFAULT_MAX_CONNECTIONS
}

const fn default_dns_propagation_seconds() -> u64 {
    DEFAULT_DNS_PROPAGATION_SECONDS
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::AcmeChallenge;

    #[derive(Deserialize)]
    struct ChallengeConfig {
        challenge: AcmeChallenge,
    }

    #[test]
    fn parses_documented_acme_challenge_names() {
        let cases = [
            ("http-01", AcmeChallenge::Http01),
            ("tls-alpn-01", AcmeChallenge::TlsAlpn01),
            ("cloudflare-dns-01", AcmeChallenge::CloudflareDns01),
        ];
        for (name, expected) in cases {
            let config: ChallengeConfig = toml::from_str(&format!("challenge = \"{name}\""))
                .expect("文档中的 ACME 验证名称应有效");
            assert_eq!(config.challenge, expected);
        }
    }
}
