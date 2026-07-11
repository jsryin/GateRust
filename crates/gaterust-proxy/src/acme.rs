use std::{collections::HashMap, future::Future, path::PathBuf, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt as _;
use instant_acme::{
    Account, AuthorizationStatus, ChallengeType, ExternalAccountKey, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use rustls_acme::{AcmeConfig, EventOk, UseChallenge, caches::DirCache};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    AcmeChallenge, CertificateConfig, CertificateIssuer, ProxyError, Result,
    cache::{CertificateCache, prepare_private_directory},
    cloudflare::CloudflareClient,
    tls::{CertificateResolver, DirectResolver},
};

const DNS_RETRY_INTERVAL: Duration = Duration::from_hours(1);
const ACME_OPERATION_TIMEOUT: Duration = Duration::from_mins(3);
const TASK_STOP_TIMEOUT: Duration = Duration::from_secs(35);

pub(crate) struct CertificateManager {
    cache_dir: PathBuf,
    resolver: CertificateResolver,
    tasks: HashMap<String, CertificateTask>,
}

struct CertificateTask {
    config: CertificateConfig,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

impl CertificateManager {
    pub(crate) fn new(cache_dir: PathBuf, resolver: CertificateResolver) -> Self {
        Self {
            cache_dir,
            resolver,
            tasks: HashMap::new(),
        }
    }

    pub(crate) async fn apply(&mut self, configs: &[CertificateConfig]) {
        let desired: HashMap<_, _> = configs
            .iter()
            .map(|config| (config.name.as_str(), config))
            .collect();
        let removed: Vec<_> = self
            .tasks
            .iter()
            .filter(|(name, task)| {
                desired
                    .get(name.as_str())
                    .is_none_or(|next| **next != task.config)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in &removed {
            if let Some(task) = self.tasks.get(name) {
                task.cancellation.cancel();
            }
        }
        for name in removed {
            self.stop(&name).await;
        }
        for config in configs {
            if !self.tasks.contains_key(&config.name) {
                self.start(config.clone());
            }
        }
    }

    fn start(&mut self, config: CertificateConfig) {
        let cancellation = CancellationToken::new();
        let task = match config.challenge {
            AcmeChallenge::Http01 | AcmeChallenge::TlsAlpn01 => {
                self.start_rustls_acme(config.clone(), cancellation.clone())
            }
            AcmeChallenge::CloudflareDns01 => {
                self.start_dns_acme(config.clone(), cancellation.clone())
            }
        };
        tracing::info!(certificate = %config.name, domains = ?config.domains, "证书管理任务已启动");
        self.tasks.insert(
            config.name.clone(),
            CertificateTask {
                config,
                cancellation,
                task,
            },
        );
    }

    fn start_rustls_acme(
        &self,
        config: CertificateConfig,
        cancellation: CancellationToken,
    ) -> JoinHandle<()> {
        let challenge = match config.challenge {
            AcmeChallenge::Http01 => UseChallenge::Http01,
            AcmeChallenge::TlsAlpn01 => UseChallenge::TlsAlpn01,
            AcmeChallenge::CloudflareDns01 => unreachable!(),
        };
        let cache = self.cache_dir.join(&config.name);
        let mut state = AcmeConfig::new(&config.domains)
            .contact_push(format!("mailto:{}", config.email))
            .cache(DirCache::new(cache.clone()))
            .directory_lets_encrypt(config.production)
            .challenge_type(challenge)
            .state();
        self.resolver
            .install_acme(&config.name, &config.domains, state.resolver());
        tokio::spawn(async move {
            if let Err(error) = prepare_private_directory(&cache).await {
                tracing::error!(certificate = %config.name, %error, "保护 ACME 缓存目录失败");
                return;
            }
            loop {
                tokio::select! {
                    () = cancellation.cancelled() => break,
                    event = state.next() => match event {
                        Some(Ok(EventOk::DeployedCachedCert | EventOk::DeployedNewCert)) => {
                            tracing::info!(certificate = %config.name, "TLS 证书已部署并热更新");
                        }
                        Some(Ok(EventOk::CertCacheStore | EventOk::AccountCacheStore)) => {}
                        Some(Err(error)) => {
                            tracing::error!(certificate = %config.name, %error, "ACME 证书任务失败，将按退避策略重试");
                        }
                        None => break,
                    }
                }
            }
        })
    }

    fn start_dns_acme(
        &self,
        config: CertificateConfig,
        cancellation: CancellationToken,
    ) -> JoinHandle<()> {
        let resolver = Arc::new(DirectResolver::default());
        self.resolver
            .install_direct(&config.name, &config.domains, &resolver);
        let cache = CertificateCache::new(self.cache_dir.join(&config.name));
        tokio::spawn(async move {
            let mut needs_certificate = match cache.load_certificate(&config, &resolver).await {
                Ok(loaded) => !loaded,
                Err(error) => {
                    tracing::warn!(certificate = %config.name, %error, "读取 DNS-01 缓存证书失败");
                    true
                }
            };
            loop {
                let delay = if needs_certificate {
                    Duration::ZERO
                } else {
                    cache.renewal_delay().await.unwrap_or(Duration::ZERO)
                };
                tokio::select! {
                    () = cancellation.cancelled() => break,
                    () = tokio::time::sleep(delay) => {}
                }
                match issue_dns_certificate(&config, &cache, &cancellation).await {
                    Ok((certificate, private_key)) => {
                        match deploy_dns_certificate(
                            &config,
                            &cache,
                            &resolver,
                            &certificate,
                            &private_key,
                        )
                        .await
                        {
                            Ok(()) => {
                                needs_certificate = false;
                                tracing::info!(certificate = %config.name, "DNS-01 证书已签发并热更新");
                                continue;
                            }
                            Err(error) => {
                                tracing::error!(certificate = %config.name, %error, "部署 DNS-01 证书失败，一小时后重试");
                            }
                        }
                    }
                    Err(error) => {
                        if cancellation.is_cancelled() {
                            break;
                        }
                        tracing::error!(certificate = %config.name, %error, "DNS-01 签发失败，一小时后重试");
                    }
                }
                if !sleep_or_cancel(&cancellation, DNS_RETRY_INTERVAL).await {
                    break;
                }
            }
        })
    }

    async fn stop(&mut self, name: &str) {
        let Some(mut handle) = self.tasks.remove(name) else {
            return;
        };
        self.resolver.remove(name);
        handle.cancellation.cancel();
        if tokio::time::timeout(TASK_STOP_TIMEOUT, &mut handle.task)
            .await
            .is_err()
        {
            handle.task.abort();
        }
        tracing::info!(certificate = name, "证书管理任务已停止");
    }

    pub(crate) async fn shutdown(&mut self) {
        for task in self.tasks.values() {
            task.cancellation.cancel();
        }
        let names: Vec<_> = self.tasks.keys().cloned().collect();
        for name in names {
            self.stop(&name).await;
        }
    }
}

impl Drop for CertificateManager {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            task.cancellation.cancel();
            task.task.abort();
        }
    }
}

async fn issue_dns_certificate(
    config: &CertificateConfig,
    cache: &CertificateCache,
    cancellation: &CancellationToken,
) -> Result<(String, String)> {
    let account = load_or_create_account(config, cache, cancellation).await?;
    let identifiers: Vec<_> = config
        .domains
        .iter()
        .map(|domain| Identifier::Dns(domain.clone()))
        .collect();
    let mut order = bounded_acme(
        cancellation,
        "创建订单",
        account.new_order(&NewOrder::new(&identifiers)),
    )
    .await?
    .map_err(acme_error)?;
    let cloudflare = CloudflareClient::new(
        config.cloudflare_api_token.clone().unwrap_or_default(),
        config.cloudflare_zone_id.clone().unwrap_or_default(),
    );
    let mut records = Vec::with_capacity(config.domains.len());
    let authorization_result = authorize_dns(
        &mut order,
        &cloudflare,
        config.dns_propagation_seconds,
        &mut records,
        cancellation,
    )
    .await;
    let ready_result = match authorization_result {
        Ok(()) => bounded_acme(
            cancellation,
            "等待订单就绪",
            order.poll_ready(&RetryPolicy::default().timeout(Duration::from_mins(2))),
        )
        .await
        .and_then(|result| result.map_err(acme_error))
        .and_then(|status| {
            if status == OrderStatus::Ready {
                Ok(())
            } else {
                Err(ProxyError::Acme(format!("订单状态异常: {status:?}")))
            }
        }),
        Err(error) => Err(error),
    };
    cleanup_records(&cloudflare, records).await;
    ready_result?;
    let private_key = bounded_acme(cancellation, "提交证书请求", order.finalize())
        .await?
        .map_err(acme_error)?;
    let certificate = bounded_acme(
        cancellation,
        "下载证书",
        order.poll_certificate(&RetryPolicy::default().timeout(Duration::from_mins(2))),
    )
    .await?
    .map_err(acme_error)?;
    Ok((certificate, private_key))
}

async fn authorize_dns(
    order: &mut instant_acme::Order,
    cloudflare: &CloudflareClient,
    propagation_seconds: u64,
    records: &mut Vec<String>,
    cancellation: &CancellationToken,
) -> Result<()> {
    let mut authorizations = order.authorizations();
    while let Some(authorization) =
        bounded_acme(cancellation, "读取域名授权", authorizations.next()).await?
    {
        let mut authorization = authorization.map_err(acme_error)?;
        match authorization.status {
            AuthorizationStatus::Valid => continue,
            AuthorizationStatus::Pending => {}
            status => {
                return Err(ProxyError::Acme(format!("域名授权状态异常: {status:?}")));
            }
        }
        let mut challenge = authorization
            .challenge(ChallengeType::Dns01)
            .ok_or_else(|| ProxyError::Acme("CA 未提供 DNS-01 挑战".into()))?;
        let domain = challenge.identifier().to_string();
        let domain = domain.strip_prefix("*.").unwrap_or(&domain);
        let record_name = format!("_acme-challenge.{domain}");
        let value = challenge.key_authorization().dns_value();
        let record = cloudflare.create_txt(&record_name, &value).await?;
        records.push(record);
        tokio::select! {
            () = cancellation.cancelled() => {
                return Err(ProxyError::Acme("DNS-01 签发已取消".into()));
            }
            () = tokio::time::sleep(Duration::from_secs(propagation_seconds)) => {}
        }
        bounded_acme(cancellation, "提交 DNS-01 挑战", challenge.set_ready())
            .await?
            .map_err(acme_error)?;
    }
    Ok(())
}

async fn load_or_create_account(
    config: &CertificateConfig,
    cache: &CertificateCache,
    cancellation: &CancellationToken,
) -> Result<Account> {
    if let Some(credentials) = cache.load_account(config).await? {
        return bounded_acme(
            cancellation,
            "恢复 ACME 账户",
            Account::builder()
                .map_err(acme_error)?
                .from_credentials(credentials),
        )
        .await?
        .map_err(acme_error);
    }
    let contact = format!("mailto:{}", config.email);
    let contacts = [contact.as_str()];
    let external = if config.issuer == CertificateIssuer::GoogleTrustServices {
        let key = config.google_eab_hmac_key.as_deref().unwrap_or_default();
        let key = URL_SAFE_NO_PAD.decode(key).map_err(|_| {
            ProxyError::InvalidConfig("Google EAB HMAC key 不是 URL-safe Base64".into())
        })?;
        Some(ExternalAccountKey::new(
            config.google_eab_key_id.clone().unwrap_or_default(),
            &key,
        ))
    } else {
        None
    };
    let (account, credentials) = bounded_acme(
        cancellation,
        "创建 ACME 账户",
        Account::builder().map_err(acme_error)?.create(
            &NewAccount {
                contact: &contacts,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            config.directory_url().into(),
            external.as_ref(),
        ),
    )
    .await?
    .map_err(acme_error)?;
    cache.store_account(config, credentials).await?;
    Ok(account)
}

async fn cleanup_records(cloudflare: &CloudflareClient, records: Vec<String>) {
    futures_util::stream::iter(records)
        .for_each_concurrent(8, |record| async move {
            if let Err(error) = cloudflare.delete_record(&record).await {
                tracing::warn!(%error, record, "清理 Cloudflare DNS 挑战记录失败");
            }
        })
        .await;
}

async fn deploy_dns_certificate(
    config: &CertificateConfig,
    cache: &CertificateCache,
    resolver: &DirectResolver,
    certificate: &str,
    private_key: &str,
) -> Result<()> {
    resolver.set_pem(certificate.as_bytes(), private_key.as_bytes())?;
    cache
        .store_certificate(config, certificate, private_key)
        .await
}

async fn sleep_or_cancel(cancellation: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        () = cancellation.cancelled() => false,
        () = tokio::time::sleep(duration) => true,
    }
}

async fn bounded_acme<T, F>(
    cancellation: &CancellationToken,
    operation: &'static str,
    future: F,
) -> Result<T>
where
    F: Future<Output = T>,
{
    tokio::select! {
        () = cancellation.cancelled() => {
            Err(ProxyError::Acme(format!("{operation}已取消")))
        }
        result = tokio::time::timeout(ACME_OPERATION_TIMEOUT, future) => {
            result.map_err(|_| ProxyError::Acme(format!("{operation}超时")))
        }
    }
}

fn acme_error(error: impl std::fmt::Display) -> ProxyError {
    ProxyError::Acme(error.to_string())
}
