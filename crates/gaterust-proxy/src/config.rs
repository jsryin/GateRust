use std::{
    collections::{HashMap, HashSet},
    fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use http::Uri;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

use crate::{ProxyError, Result};

const DEFAULT_MAX_CONNECTIONS: usize = 2_048;
const LETS_ENCRYPT_PRODUCTION_DIRECTORY: &str = "https://acme-v02.api.letsencrypt.org/directory";
const LETS_ENCRYPT_STAGING_DIRECTORY: &str =
    "https://acme-staging-v02.api.letsencrypt.org/directory";
const GOOGLE_PRODUCTION_DIRECTORY: &str = "https://dv.acme-v02.api.pki.goog/directory";
const GOOGLE_STAGING_DIRECTORY: &str = "https://dv.acme-v02.test-api.pki.goog/directory";

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    pub proxy: ProxyListenerConfig,
    #[serde(default)]
    pub acme_accounts: Vec<AcmeAccountConfig>,
    #[serde(default)]
    pub dns_accounts: Vec<DnsAccountConfig>,
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
pub enum AcmeProvider {
    LetsEncrypt,
    GoogleCloud,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcmeEnvironment {
    Production,
    Staging,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAlgorithm {
    Ec256,
    Rsa2048,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcmeAccountConfig {
    pub id: String,
    pub name: String,
    pub provider: AcmeProvider,
    pub environment: AcmeEnvironment,
    pub email: String,
    pub key_algorithm: KeyAlgorithm,
    pub eab_key_id: Option<String>,
    pub eab_hmac_key: Option<SecretString>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsProvider {
    Cloudflare,
    GoDaddy,
    Aliyun,
    TencentCloud,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DnsAccountConfig {
    pub id: String,
    pub name: String,
    pub provider: DnsProvider,
    pub api_token: Option<SecretString>,
    pub access_key: Option<SecretString>,
    pub secret_key: Option<SecretString>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum CertificateValidation {
    DnsAccount { dns_account_id: String },
    Manual,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CertificateConfig {
    pub id: String,
    pub name: String,
    pub domains: Vec<String>,
    pub acme_account_id: String,
    pub validation: Option<CertificateValidation>,
    #[serde(default)]
    pub auto_renew: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    pub name: String,
    pub host: String,
    #[serde(default = "default_path_prefix")]
    pub path_prefix: String,
    pub upstream: String,
    pub certificate_id: Option<String>,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl ProxyConfig {
    /// 读取并验证代理配置；旧格式只迁移可从原字段确定的数据。
    ///
    /// # Errors
    ///
    /// 文件不可读、TOML 格式错误或字段不满足约束时返回错误。
    pub fn read(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|source| ProxyError::ReadConfig {
            path: path.to_owned(),
            source,
        })?;
        let mut config = match toml::from_str::<Self>(&content) {
            Ok(config) => config,
            Err(current_error) => match toml::from_str::<LegacyProxyConfig>(&content) {
                Ok(legacy) => legacy.migrate()?,
                Err(_) => {
                    return Err(ProxyError::ParseConfig {
                        path: path.to_owned(),
                        source: current_error,
                    });
                }
            },
        };
        config.validate()?;
        Ok(config)
    }

    /// 读取并验证代理配置，相对缓存路径以配置文件目录为基准。
    ///
    /// # Errors
    ///
    /// 文件不可读、TOML 格式错误或字段不满足约束时返回错误。
    pub fn load(path: &Path) -> Result<Self> {
        let mut config = Self::read(path)?;
        if config.proxy.cache_dir.is_relative() {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            config.proxy.cache_dir = parent.join(&config.proxy.cache_dir);
        }
        Ok(config)
    }

    /// 校验完整配置及全部跨资源引用。
    ///
    /// # Errors
    ///
    /// 任意字段或引用不满足约束时返回错误。
    pub fn validate(&mut self) -> Result<()> {
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
        if self.acme_accounts.len() > 64
            || self.dns_accounts.len() > 64
            || self.certificates.len() > 256
            || self.routes.len() > 4_096
        {
            return Err(ProxyError::InvalidConfig(
                "ACME/DNS 账户不能超过 64 个，证书不能超过 256 个，路由不能超过 4096 条".into(),
            ));
        }

        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for account in &self.acme_accounts {
            validate_id("ACME 账户", &account.id)?;
            validate_display_name("ACME 账户", &account.name)?;
            insert_unique(&mut ids, "ACME 账户 ID", &account.id)?;
            insert_unique(&mut names, "ACME 账户名称", &account.name)?;
            account.validate()?;
        }

        ids.clear();
        names.clear();
        for account in &self.dns_accounts {
            validate_id("DNS 账户", &account.id)?;
            validate_display_name("DNS 账户", &account.name)?;
            insert_unique(&mut ids, "DNS 账户 ID", &account.id)?;
            insert_unique(&mut names, "DNS 账户名称", &account.name)?;
            account.validate()?;
        }

        let acme_ids = self
            .acme_accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect::<HashSet<_>>();
        let dns_ids = self
            .dns_accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect::<HashSet<_>>();
        let mut certificate_ids = HashSet::new();
        let mut certificate_names = HashSet::new();
        let mut domain_owners: HashMap<String, String> = HashMap::new();
        for certificate in &mut self.certificates {
            validate_id("证书", &certificate.id)?;
            validate_display_name("证书", &certificate.name)?;
            if !certificate_ids.insert(certificate.id.clone()) {
                return Err(ProxyError::InvalidConfig(format!(
                    "证书 ID 重复: {}",
                    certificate.id
                )));
            }
            if !certificate_names.insert(certificate.name.clone()) {
                return Err(ProxyError::InvalidConfig(format!(
                    "证书名称重复: {}",
                    certificate.name
                )));
            }
            certificate.validate(&acme_ids, &dns_ids, &mut domain_owners)?;
        }

        let mut route_names = HashSet::new();
        let mut route_keys = HashSet::new();
        for route in &mut self.routes {
            validate_display_name("路由", &route.name)?;
            insert_unique(&mut route_names, "路由名称", &route.name)?;
            route.host = normalize_route_host(&route.host)?;
            validate_path_prefix(&route.path_prefix)?;
            validate_upstream(&route.upstream)?;
            if !route_keys.insert((route.host.as_str(), route.path_prefix.as_str())) {
                return Err(ProxyError::InvalidConfig(format!(
                    "Host 与路径前缀重复: {}{}",
                    route.host, route.path_prefix
                )));
            }
            if let Some(certificate_id) = &route.certificate_id {
                if !certificate_ids.contains(certificate_id) {
                    return Err(ProxyError::InvalidConfig(format!(
                        "路由 {} 引用了不存在的证书 {certificate_id}",
                        route.name
                    )));
                }
                if certificate_for_host(&domain_owners, &route.host)
                    != Some(certificate_id.as_str())
                {
                    return Err(ProxyError::InvalidConfig(format!(
                        "证书 {certificate_id} 不包含路由域名 {}",
                        route.host
                    )));
                }
            }
        }
        Ok(())
    }
}

impl AcmeAccountConfig {
    fn validate(&self) -> Result<()> {
        if self.email.is_empty() || self.email.len() > 254 || !self.email.contains('@') {
            return Err(ProxyError::InvalidConfig(format!(
                "ACME 账户 {} 的联系邮箱无效",
                self.name
            )));
        }
        match self.provider {
            AcmeProvider::LetsEncrypt => {
                reject_secret(
                    "ACME 账户",
                    &self.name,
                    "eab_key_id",
                    self.eab_key_id.as_deref(),
                )?;
                reject_secret(
                    "ACME 账户",
                    &self.name,
                    "eab_hmac_key",
                    self.eab_hmac_key.as_ref().map(SecretString::expose),
                )
            }
            AcmeProvider::GoogleCloud => {
                require_secret(
                    "ACME 账户",
                    &self.name,
                    "eab_key_id",
                    self.eab_key_id.as_deref(),
                )?;
                require_secret(
                    "ACME 账户",
                    &self.name,
                    "eab_hmac_key",
                    self.eab_hmac_key.as_ref().map(SecretString::expose),
                )
            }
        }
    }

    #[must_use]
    pub fn directory_url(&self) -> &'static str {
        match (self.provider, self.environment) {
            (AcmeProvider::LetsEncrypt, AcmeEnvironment::Production) => {
                LETS_ENCRYPT_PRODUCTION_DIRECTORY
            }
            (AcmeProvider::LetsEncrypt, AcmeEnvironment::Staging) => LETS_ENCRYPT_STAGING_DIRECTORY,
            (AcmeProvider::GoogleCloud, AcmeEnvironment::Production) => GOOGLE_PRODUCTION_DIRECTORY,
            (AcmeProvider::GoogleCloud, AcmeEnvironment::Staging) => GOOGLE_STAGING_DIRECTORY,
        }
    }
}

impl DnsAccountConfig {
    fn validate(&self) -> Result<()> {
        match self.provider {
            DnsProvider::Cloudflare => {
                require_secret(
                    "DNS 账户",
                    &self.name,
                    "api_token",
                    self.api_token.as_ref().map(SecretString::expose),
                )?;
                reject_secret(
                    "DNS 账户",
                    &self.name,
                    "access_key",
                    self.access_key.as_ref().map(SecretString::expose),
                )?;
                reject_secret(
                    "DNS 账户",
                    &self.name,
                    "secret_key",
                    self.secret_key.as_ref().map(SecretString::expose),
                )
            }
            DnsProvider::GoDaddy | DnsProvider::Aliyun | DnsProvider::TencentCloud => {
                reject_secret(
                    "DNS 账户",
                    &self.name,
                    "api_token",
                    self.api_token.as_ref().map(SecretString::expose),
                )?;
                require_secret(
                    "DNS 账户",
                    &self.name,
                    "access_key",
                    self.access_key.as_ref().map(SecretString::expose),
                )?;
                require_secret(
                    "DNS 账户",
                    &self.name,
                    "secret_key",
                    self.secret_key.as_ref().map(SecretString::expose),
                )
            }
        }
    }
}

impl CertificateConfig {
    fn validate(
        &mut self,
        acme_ids: &HashSet<&str>,
        dns_ids: &HashSet<&str>,
        owners: &mut HashMap<String, String>,
    ) -> Result<()> {
        if !acme_ids.contains(self.acme_account_id.as_str()) {
            return Err(ProxyError::InvalidConfig(format!(
                "证书 {} 引用了不存在的 ACME 账户 {}",
                self.name, self.acme_account_id
            )));
        }
        match &self.validation {
            Some(CertificateValidation::DnsAccount { dns_account_id }) => {
                if !dns_ids.contains(dns_account_id.as_str()) {
                    return Err(ProxyError::InvalidConfig(format!(
                        "证书 {} 引用了不存在的 DNS 账户 {dns_account_id}",
                        self.name
                    )));
                }
            }
            Some(CertificateValidation::Manual) if self.auto_renew => {
                return Err(ProxyError::InvalidConfig(format!(
                    "证书 {} 使用手动解析时不能启用自动续签",
                    self.name
                )));
            }
            Some(_) => {}
            None if self.migration_error.is_some() && !self.auto_renew => {}
            None => {
                return Err(ProxyError::InvalidConfig(format!(
                    "证书 {} 必须选择验证方式",
                    self.name
                )));
            }
        }
        if self.domains.is_empty() || self.domains.len() > 100 {
            return Err(ProxyError::InvalidConfig(format!(
                "证书 {} 的域名数量必须为 1..=100",
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
            if let Some(owner) = owners.insert(domain.clone(), self.id.clone()) {
                return Err(ProxyError::InvalidConfig(format!(
                    "域名 {domain} 同时属于证书 {owner} 和 {}",
                    self.id
                )));
            }
        }
        Ok(())
    }
}

fn certificate_for_host<'a>(owners: &'a HashMap<String, String>, host: &str) -> Option<&'a str> {
    owners.get(host).map(String::as_str).or_else(|| {
        owners
            .iter()
            .filter_map(|(domain, owner)| {
                let suffix = domain.strip_prefix("*.")?;
                wildcard_matches(host, suffix).then_some((suffix.len(), owner.as_str()))
            })
            .max_by_key(|(length, _)| *length)
            .map(|(_, owner)| owner)
    })
}

pub(crate) fn wildcard_matches(host: &str, suffix: &str) -> bool {
    host.strip_suffix(suffix)
        .is_some_and(|prefix| prefix.ends_with('.') && !prefix[..prefix.len() - 1].contains('.'))
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

fn normalize_route_host(value: &str) -> Result<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if value == "localhost" {
        return Ok(value);
    }
    normalize_domain(&value)
}

fn validate_id(kind: &str, id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || matches!(id, "." | "..")
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ProxyError::InvalidConfig(format!(
            "{kind} ID 必须为 1..=128 个 ASCII 字母、数字、-、_ 或 ."
        )));
    }
    Ok(())
}

fn validate_display_name(kind: &str, name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > 128 {
        return Err(ProxyError::InvalidConfig(format!(
            "{kind}名称不能为空且不能超过 128 字节"
        )));
    }
    Ok(())
}

fn insert_unique<'a>(values: &mut HashSet<&'a str>, kind: &str, value: &'a str) -> Result<()> {
    if !values.insert(value) {
        return Err(ProxyError::InvalidConfig(format!("{kind}重复: {value}")));
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

fn require_secret(kind: &str, name: &str, field: &str, value: Option<&str>) -> Result<()> {
    if value.is_none_or(str::is_empty) {
        return Err(ProxyError::InvalidConfig(format!(
            "{kind} {name} 必须配置 {field}"
        )));
    }
    Ok(())
}

fn reject_secret(kind: &str, name: &str, field: &str, value: Option<&str>) -> Result<()> {
    if value.is_some() {
        return Err(ProxyError::InvalidConfig(format!(
            "{kind} {name} 不应配置 {field}"
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProxyConfig {
    proxy: ProxyListenerConfig,
    #[serde(default)]
    certificates: Vec<LegacyCertificateConfig>,
    #[serde(default)]
    routes: Vec<LegacyRouteConfig>,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LegacyIssuer {
    LetsEncrypt,
    GoogleTrustServices,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
enum LegacyChallenge {
    #[serde(rename = "http-01")]
    Http01,
    #[serde(rename = "tls-alpn-01")]
    TlsAlpn01,
    #[serde(rename = "cloudflare-dns-01")]
    CloudflareDns01,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCertificateConfig {
    name: String,
    domains: Vec<String>,
    email: String,
    issuer: LegacyIssuer,
    challenge: LegacyChallenge,
    #[serde(default)]
    production: bool,
    cloudflare_api_token: Option<String>,
    cloudflare_zone_id: Option<String>,
    google_eab_key_id: Option<String>,
    google_eab_hmac_key: Option<String>,
    #[serde(default)]
    dns_propagation_seconds: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRouteConfig {
    name: String,
    host: String,
    #[serde(default = "default_path_prefix")]
    path_prefix: String,
    upstream: String,
    certificate: Option<String>,
}

impl LegacyProxyConfig {
    fn migrate(self) -> Result<ProxyConfig> {
        let mut acme_accounts = Vec::with_capacity(self.certificates.len());
        let mut dns_accounts = Vec::new();
        let mut certificates = Vec::with_capacity(self.certificates.len());
        let certificate_ids = self
            .certificates
            .iter()
            .map(|certificate| (certificate.name.clone(), certificate.name.clone()))
            .collect::<HashMap<_, _>>();

        for legacy in self.certificates {
            let acme_account_id = format!("acme-{}", legacy.name);
            let provider = match legacy.issuer {
                LegacyIssuer::LetsEncrypt => AcmeProvider::LetsEncrypt,
                LegacyIssuer::GoogleTrustServices => AcmeProvider::GoogleCloud,
            };
            acme_accounts.push(AcmeAccountConfig {
                id: acme_account_id.clone(),
                name: format!("{} ACME", legacy.name),
                provider,
                environment: if legacy.production {
                    AcmeEnvironment::Production
                } else {
                    AcmeEnvironment::Staging
                },
                email: legacy.email,
                // 旧版 instant-acme 的账户与证书密钥均固定为 P-256。
                key_algorithm: KeyAlgorithm::Ec256,
                eab_key_id: legacy.google_eab_key_id,
                eab_hmac_key: legacy.google_eab_hmac_key.map(SecretString::new),
            });

            let (validation, migration_error) = match legacy.challenge {
                LegacyChallenge::CloudflareDns01 => {
                    let dns_account_id = format!("dns-{}", legacy.name);
                    dns_accounts.push(DnsAccountConfig {
                        id: dns_account_id.clone(),
                        name: format!("{} Cloudflare", legacy.name),
                        provider: DnsProvider::Cloudflare,
                        api_token: legacy.cloudflare_api_token.map(SecretString::new),
                        access_key: None,
                        secret_key: None,
                    });
                    (
                        Some(CertificateValidation::DnsAccount { dns_account_id }),
                        None,
                    )
                }
                LegacyChallenge::Http01 => (
                    None,
                    Some("旧配置使用 HTTP-01，请重新选择 DNS 账户或手动解析".into()),
                ),
                LegacyChallenge::TlsAlpn01 => (
                    None,
                    Some("旧配置使用 TLS-ALPN-01，请重新选择 DNS 账户或手动解析".into()),
                ),
            };
            let _ = legacy.cloudflare_zone_id;
            let _ = legacy.dns_propagation_seconds;
            certificates.push(CertificateConfig {
                id: legacy.name.clone(),
                name: legacy.name,
                domains: legacy.domains,
                acme_account_id,
                validation,
                auto_renew: false,
                migration_error,
            });
        }
        let routes = self
            .routes
            .into_iter()
            .map(|route| {
                let certificate_id = match route.certificate {
                    Some(name) => Some(certificate_ids.get(&name).cloned().ok_or_else(|| {
                        ProxyError::InvalidConfig(format!(
                            "旧路由 {} 引用了不存在的证书 {name}",
                            route.name
                        ))
                    })?),
                    None => None,
                };
                Ok(RouteConfig {
                    name: route.name,
                    host: route.host,
                    path_prefix: route.path_prefix,
                    upstream: route.upstream,
                    certificate_id,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ProxyConfig {
            proxy: self.proxy,
            acme_accounts,
            dns_accounts,
            certificates,
            routes,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn wildcard_only_matches_one_label() {
        assert!(wildcard_matches("www.example.com", "example.com"));
        assert!(!wildcard_matches("example.com", "example.com"));
        assert!(!wildcard_matches("a.b.example.com", "example.com"));
        assert!(!wildcard_matches("badexample.com", "example.com"));
    }

    #[test]
    fn migrates_legacy_dns_without_inventing_credentials() {
        let mut file = tempfile::NamedTempFile::new().expect("创建临时配置");
        write!(
            file,
            r#"[proxy]
http_bind = "127.0.0.1:80"
https_bind = "127.0.0.1:443"
cache_dir = "cache"

[[certificates]]
name = "site"
domains = ["*.example.com"]
email = "admin@example.com"
issuer = "lets_encrypt"
challenge = "cloudflare-dns-01"
cloudflare_api_token = "token"
cloudflare_zone_id = "zone"

[[routes]]
name = "web"
host = "www.example.com"
upstream = "http://127.0.0.1:3000"
certificate = "site"
"#
        )
        .expect("写入配置");
        let config = ProxyConfig::read(file.path()).expect("迁移旧配置");
        assert_eq!(config.acme_accounts[0].id, "acme-site");
        assert_eq!(config.dns_accounts[0].id, "dns-site");
        assert_eq!(config.certificates[0].id, "site");
        assert_eq!(config.routes[0].certificate_id.as_deref(), Some("site"));
    }

    #[test]
    fn manual_validation_rejects_auto_renew() {
        let mut config = ProxyConfig {
            proxy: ProxyListenerConfig {
                http_bind: "127.0.0.1:80".parse().expect("地址有效"),
                https_bind: "127.0.0.1:443".parse().expect("地址有效"),
                cache_dir: "cache".into(),
                max_connections: 16,
            },
            acme_accounts: vec![AcmeAccountConfig {
                id: "account".into(),
                name: "账户".into(),
                provider: AcmeProvider::LetsEncrypt,
                environment: AcmeEnvironment::Staging,
                email: "admin@example.com".into(),
                key_algorithm: KeyAlgorithm::Ec256,
                eab_key_id: None,
                eab_hmac_key: None,
            }],
            dns_accounts: vec![],
            certificates: vec![CertificateConfig {
                id: "certificate".into(),
                name: "证书".into(),
                domains: vec!["example.com".into()],
                acme_account_id: "account".into(),
                validation: Some(CertificateValidation::Manual),
                auto_renew: true,
                migration_error: None,
            }],
            routes: vec![],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn cache_resource_ids_reject_special_path_components() {
        assert!(validate_id("证书", ".").is_err());
        assert!(validate_id("证书", "..").is_err());
        assert!(validate_id("证书", "site.example").is_ok());
    }
}
