use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use instant_acme::AccountCredentials;
use serde::{Deserialize, Serialize};

use crate::{CertificateConfig, ProxyError, Result, tls::DirectResolver};

const DNS_RENEW_INTERVAL: Duration = Duration::from_hours(24 * 60);

pub(crate) struct CertificateCache {
    directory: PathBuf,
}

#[derive(Deserialize, Serialize)]
struct CachedAccount {
    directory_url: String,
    email: String,
    credentials: AccountCredentials,
}

#[derive(Deserialize, Serialize)]
struct CertificateMetadata {
    directory_url: String,
    domains: Vec<String>,
}

impl CertificateMetadata {
    fn new(config: &CertificateConfig) -> Self {
        Self {
            directory_url: config.directory_url().into(),
            domains: config.domains.clone(),
        }
    }

    fn matches(&self, config: &CertificateConfig) -> bool {
        self.directory_url == config.directory_url() && self.domains == config.domains
    }
}

impl CertificateCache {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub(crate) async fn load_account(
        &self,
        config: &CertificateConfig,
    ) -> Result<Option<AccountCredentials>> {
        let content = match tokio::fs::read(self.directory.join("account-v1.json")).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let cached: CachedAccount = serde_json::from_slice(&content)?;
        if cached.directory_url == config.directory_url() && cached.email == config.email {
            Ok(Some(cached.credentials))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn store_account(
        &self,
        config: &CertificateConfig,
        credentials: AccountCredentials,
    ) -> Result<()> {
        let cached = CachedAccount {
            directory_url: config.directory_url().into(),
            email: config.email.clone(),
            credentials,
        };
        atomic_write(
            &self.directory.join("account-v1.json"),
            &serde_json::to_vec(&cached)?,
        )
        .await
    }

    pub(crate) async fn load_certificate(
        &self,
        config: &CertificateConfig,
        resolver: &DirectResolver,
    ) -> Result<bool> {
        let metadata = match tokio::fs::read(self.directory.join("certificate-v1.json")).await {
            Ok(content) => serde_json::from_slice::<CertificateMetadata>(&content)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if !metadata.matches(config) {
            return Ok(false);
        }
        let certificate = read_optional(&self.directory.join("certificate.pem")).await?;
        let private_key = read_optional(&self.directory.join("private-key.pem")).await?;
        match (certificate, private_key) {
            (Some(certificate), Some(private_key)) => {
                resolver.set_pem(&certificate, &private_key)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(crate) async fn store_certificate(
        &self,
        config: &CertificateConfig,
        certificate: &str,
        private_key: &str,
    ) -> Result<()> {
        atomic_write(
            &self.directory.join("certificate.pem"),
            certificate.as_bytes(),
        )
        .await?;
        atomic_write(
            &self.directory.join("private-key.pem"),
            private_key.as_bytes(),
        )
        .await?;
        let renew_at = SystemTime::now()
            .checked_add(DNS_RENEW_INTERVAL)
            .unwrap_or(SystemTime::now())
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        atomic_write(&self.directory.join("renew-at"), renew_at.as_bytes()).await?;
        let metadata = CertificateMetadata::new(config);
        atomic_write(
            &self.directory.join("certificate-v1.json"),
            &serde_json::to_vec(&metadata)?,
        )
        .await
    }

    pub(crate) async fn renewal_delay(&self) -> Option<Duration> {
        let value = tokio::fs::read_to_string(self.directory.join("renew-at"))
            .await
            .ok()?;
        let renew_at = UNIX_EPOCH.checked_add(Duration::from_secs(value.trim().parse().ok()?))?;
        renew_at.duration_since(SystemTime::now()).ok()
    }
}

pub(crate) async fn prepare_private_directory(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path).await?;
    set_private_directory_permissions(path).await
}

async fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| ProxyError::InvalidConfig("缓存文件缺少父目录".into()))?;
    prepare_private_directory(parent).await?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    tokio::fs::write(&temporary, content).await?;
    set_private_file_permissions(&temporary).await?;
    tokio::fs::rename(&temporary, path).await?;
    Ok(())
}

#[cfg(unix)]
async fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
async fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcmeChallenge, CertificateIssuer};

    fn config(production: bool, domains: &[&str]) -> CertificateConfig {
        CertificateConfig {
            name: "site".into(),
            domains: domains.iter().map(|domain| (*domain).into()).collect(),
            email: "admin@example.com".into(),
            issuer: CertificateIssuer::LetsEncrypt,
            challenge: AcmeChallenge::CloudflareDns01,
            production,
            cloudflare_api_token: Some("token".into()),
            cloudflare_zone_id: Some("zone".into()),
            google_eab_key_id: None,
            google_eab_hmac_key: None,
            dns_propagation_seconds: 30,
        }
    }

    #[test]
    fn certificate_identity_includes_directory_and_domains() {
        let staging = config(false, &["example.com"]);
        let metadata = CertificateMetadata::new(&staging);
        assert!(metadata.matches(&staging));
        assert!(!metadata.matches(&config(true, &["example.com"])));
        assert!(!metadata.matches(&config(false, &["www.example.com"])));
    }
}
