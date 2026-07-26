use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use x509_parser::parse_x509_certificate;

use crate::{AcmeAccountConfig, CertificateConfig, KeyAlgorithm, ProxyError, Result};

static TEMPORARY_FILE_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) struct AccountCache {
    directory: PathBuf,
}

pub(crate) struct CertificateCache {
    directory: PathBuf,
    legacy_directory: PathBuf,
}

pub(crate) struct CachedCertificate {
    pub(crate) certificate: Vec<u8>,
    pub(crate) private_key: Vec<u8>,
    pub(crate) expires_at: u64,
}

#[derive(Deserialize, Serialize)]
struct AccountMetadata {
    directory_url: String,
    key_algorithm: KeyAlgorithm,
    registered: bool,
}

#[derive(Deserialize, Serialize)]
struct CertificateMetadata {
    acme_account_id: String,
    domains: Vec<String>,
    expires_at: u64,
}

#[derive(Deserialize)]
struct LegacyCertificateMetadata {
    domains: Vec<String>,
}

impl AccountCache {
    pub(crate) fn new(root: &Path, account_id: &str) -> Self {
        Self {
            directory: root.join("accounts").join(account_id),
        }
    }

    pub(crate) async fn load_private_key(
        &self,
        config: &AcmeAccountConfig,
    ) -> Result<Option<(Vec<u8>, bool)>> {
        let metadata = match read_optional(&self.directory.join("metadata-v1.json")).await? {
            Some(content) => serde_json::from_slice::<AccountMetadata>(&content)?,
            None => return Ok(None),
        };
        if metadata.directory_url != config.directory_url()
            || metadata.key_algorithm != config.key_algorithm
        {
            return Ok(None);
        }
        let key = read_optional(&self.directory.join("private-key.pem")).await?;
        Ok(key.map(|key| (key, metadata.registered)))
    }

    pub(crate) async fn store_private_key(
        &self,
        config: &AcmeAccountConfig,
        private_key: &[u8],
        registered: bool,
    ) -> Result<()> {
        atomic_write(&self.directory.join("private-key.pem"), private_key).await?;
        let metadata = AccountMetadata {
            directory_url: config.directory_url().into(),
            key_algorithm: config.key_algorithm,
            registered,
        };
        atomic_write(
            &self.directory.join("metadata-v1.json"),
            &serde_json::to_vec(&metadata)?,
        )
        .await
    }
}

impl CertificateMetadata {
    fn new(config: &CertificateConfig, expires_at: u64) -> Self {
        Self {
            acme_account_id: config.acme_account_id.clone(),
            domains: config.domains.clone(),
            expires_at,
        }
    }

    fn matches(&self, config: &CertificateConfig) -> bool {
        self.acme_account_id == config.acme_account_id && self.domains == config.domains
    }
}

impl CertificateCache {
    pub(crate) fn new(root: &Path, certificate_id: &str) -> Self {
        Self {
            directory: root.join("certificates").join(certificate_id),
            legacy_directory: root.join(certificate_id),
        }
    }

    pub(crate) async fn load_certificate(
        &self,
        config: &CertificateConfig,
    ) -> Result<Option<CachedCertificate>> {
        if let Some(certificate) = self.load_current(config).await? {
            return Ok(Some(certificate));
        }
        self.load_legacy(config).await
    }

    async fn load_current(&self, config: &CertificateConfig) -> Result<Option<CachedCertificate>> {
        let metadata = match read_optional(&self.directory.join("metadata-v2.json")).await? {
            Some(content) => serde_json::from_slice::<CertificateMetadata>(&content)?,
            None => return Ok(None),
        };
        if !metadata.matches(config) {
            return Ok(None);
        }
        self.read_pair(Some(metadata.expires_at), &self.directory)
            .await
    }

    async fn load_legacy(&self, config: &CertificateConfig) -> Result<Option<CachedCertificate>> {
        let metadata =
            match read_optional(&self.legacy_directory.join("certificate-v1.json")).await? {
                Some(content) => serde_json::from_slice::<LegacyCertificateMetadata>(&content)?,
                None => return Ok(None),
            };
        if metadata.domains != config.domains {
            return Ok(None);
        }
        self.read_pair(None, &self.legacy_directory).await
    }

    async fn read_pair(
        &self,
        stored_expiry: Option<u64>,
        directory: &Path,
    ) -> Result<Option<CachedCertificate>> {
        let certificate = read_optional(&directory.join("certificate.pem")).await?;
        let private_key = read_optional(&directory.join("private-key.pem")).await?;
        match (certificate, private_key) {
            (Some(certificate), Some(private_key)) => {
                let expires_at = parse_certificate_expiry(&certificate)?;
                if stored_expiry.is_some_and(|stored| stored != expires_at) {
                    return Err(ProxyError::Tls("证书缓存中的过期时间与叶证书不一致".into()));
                }
                Ok(Some(CachedCertificate {
                    certificate,
                    private_key,
                    expires_at,
                }))
            }
            _ => Ok(None),
        }
    }

    pub(crate) async fn store_certificate(
        &self,
        config: &CertificateConfig,
        certificate: &[u8],
        private_key: &[u8],
    ) -> Result<u64> {
        let expires_at = parse_certificate_expiry(certificate)?;
        atomic_write(&self.directory.join("certificate.pem"), certificate).await?;
        atomic_write(&self.directory.join("private-key.pem"), private_key).await?;
        let metadata = CertificateMetadata::new(config, expires_at);
        // 元数据最后提交，加载方不会把未完整写入的一对证书和私钥视为有效缓存。
        atomic_write(
            &self.directory.join("metadata-v2.json"),
            &serde_json::to_vec(&metadata)?,
        )
        .await?;
        Ok(expires_at)
    }
}

pub(crate) fn parse_certificate_expiry(certificate_pem: &[u8]) -> Result<u64> {
    let certificate = rustls_pemfile::certs(&mut std::io::Cursor::new(certificate_pem))
        .next()
        .transpose()?
        .ok_or_else(|| ProxyError::Tls("证书链为空".into()))?;
    let (_, certificate) = parse_x509_certificate(certificate.as_ref())
        .map_err(|error| ProxyError::Tls(format!("解析叶证书失败: {error}")))?;
    u64::try_from(certificate.validity().not_after.timestamp())
        .map_err(|_| ProxyError::Tls("叶证书过期时间早于 Unix epoch".into()))
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
    let temporary_id = TEMPORARY_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp-{}-{temporary_id}", std::process::id()));
    let result = async {
        tokio::fs::write(&temporary, content).await?;
        set_private_file_permissions(&temporary).await?;
        tokio::fs::rename(&temporary, path).await?;
        Ok::<(), ProxyError>(())
    }
    .await;
    if result.is_err() {
        match tokio::fs::remove_file(&temporary).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %temporary.display(), %error, "清理缓存临时文件失败");
            }
        }
    }
    result
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
