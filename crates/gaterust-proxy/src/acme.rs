use std::{future::Future, path::Path, sync::Arc, time::Duration};

use acme2_eab::{
    Account, AccountBuilder, AuthorizationStatus, Challenge, ChallengeStatus, Csr,
    DirectoryBuilder, Order, OrderBuilder, OrderStatus, gen_ec_p256_private_key,
    gen_rsa_private_key,
    openssl::pkey::{PKey, Private},
};
use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use futures_util::{StreamExt as _, stream};
use hickory_resolver::TokioResolver;
use hickory_resolver::proto::rr::RData;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::{
    AcmeAccountConfig, AcmeProvider, CertificateConfig, CertificateValidation, DnsAccountConfig,
    KeyAlgorithm, ProxyError, Result,
    cache::{AccountCache, CertificateCache},
    dns::{DnsClient, DnsRecordHandle},
};

const ACME_OPERATION_TIMEOUT: Duration = Duration::from_mins(3);
const DNS_PROPAGATION_TIMEOUT: Duration = Duration::from_mins(5);
const DNS_POLL_INTERVAL: Duration = Duration::from_secs(5);
const ACME_POLL_INTERVAL: Duration = Duration::from_secs(5);
const ACME_POLL_ATTEMPTS: usize = 36;
const DNS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);
const DNS_CLEANUP_CONCURRENCY: usize = 8;
const DNS_LOOKUP_CONCURRENCY: usize = 16;

pub(crate) struct IssuanceRequest {
    pub(crate) certificate: CertificateConfig,
    pub(crate) account: AcmeAccountConfig,
    pub(crate) dns_account: Option<DnsAccountConfig>,
    pub(crate) account_gate: Arc<Semaphore>,
    pub(crate) dns_gate: Option<Arc<Semaphore>>,
}

pub(crate) struct IssuedCertificate {
    pub(crate) certificate_pem: Vec<u8>,
    pub(crate) private_key_pem: Vec<u8>,
    pub(crate) expires_at: u64,
}

pub(crate) struct ManualOrder {
    request: IssuanceRequest,
    order: Order,
    challenges: Vec<Challenge>,
    pub(crate) records: Vec<ManualDnsRecord>,
}

#[derive(Clone, serde::Serialize)]
pub struct ManualDnsRecord {
    pub name: String,
    pub value: String,
}

pub(crate) enum IssueResult {
    Issued(IssuedCertificate),
    Waiting(Box<ManualOrder>),
}

pub(crate) async fn begin_issue(
    request: IssuanceRequest,
    cache_root: &Path,
    cancellation: &CancellationToken,
) -> Result<IssueResult> {
    let account = load_account(
        &request.account,
        cache_root,
        cancellation,
        &request.account_gate,
    )
    .await?;
    let mut builder = OrderBuilder::new(account);
    for domain in &request.certificate.domains {
        builder.add_dns_identifier(domain.clone());
    }
    let order = bounded(cancellation, "创建 ACME 订单", builder.build()).await??;
    let (challenges, records) = collect_challenges(&order, cancellation).await?;

    match &request.certificate.validation {
        Some(CertificateValidation::DnsAccount { .. }) => {
            let dns_config = request
                .dns_account
                .as_ref()
                .ok_or_else(|| ProxyError::InvalidConfig("证书引用的 DNS 账户不存在".into()))?;
            let dns = DnsClient::from_config(dns_config, request.dns_gate.clone())?;
            authorize_automatically(&dns, &challenges, &records, cancellation).await?;
            finalize(request, order, cache_root, cancellation)
                .await
                .map(IssueResult::Issued)
        }
        Some(CertificateValidation::Manual) => Ok(IssueResult::Waiting(Box::new(ManualOrder {
            request,
            order,
            challenges,
            records,
        }))),
        None => Err(ProxyError::InvalidConfig("待迁移证书不能发起申请".into())),
    }
}

pub(crate) async fn continue_manual(
    pending: ManualOrder,
    cache_root: &Path,
    cancellation: &CancellationToken,
) -> Result<IssuedCertificate> {
    validate_challenges(&pending.challenges, cancellation).await?;
    finalize(pending.request, pending.order, cache_root, cancellation).await
}

pub(crate) async fn verify_manual(
    pending: &ManualOrder,
    cancellation: &CancellationToken,
) -> Result<()> {
    verify_dns_records(&pending.records, cancellation).await
}

async fn collect_challenges(
    order: &Order,
    cancellation: &CancellationToken,
) -> Result<(Vec<Challenge>, Vec<ManualDnsRecord>)> {
    let authorizations = bounded(cancellation, "读取域名授权", order.authorizations()).await??;
    let mut challenges = Vec::with_capacity(authorizations.len());
    let mut records = Vec::with_capacity(authorizations.len());
    for authorization in authorizations {
        match authorization.status {
            AuthorizationStatus::Valid => continue,
            AuthorizationStatus::Pending => {}
            status => {
                return Err(ProxyError::Acme(format!(
                    "域名 {} 的授权状态异常: {status:?}",
                    authorization.identifier.value
                )));
            }
        }
        let challenge = authorization
            .get_challenge("dns-01")
            .ok_or_else(|| ProxyError::Acme("CA 未提供 DNS-01 挑战".into()))?;
        let value = challenge
            .key_authorization_encoded()?
            .ok_or_else(|| ProxyError::Acme("DNS-01 挑战缺少 token".into()))?;
        let domain = authorization
            .identifier
            .value
            .strip_prefix("*.")
            .unwrap_or(&authorization.identifier.value);
        records.push(ManualDnsRecord {
            name: format!("_acme-challenge.{domain}"),
            value,
        });
        challenges.push(challenge);
    }
    Ok((challenges, records))
}

async fn authorize_automatically(
    dns: &DnsClient,
    challenges: &[Challenge],
    records: &[ManualDnsRecord],
    cancellation: &CancellationToken,
) -> Result<()> {
    let mut handles = Vec::with_capacity(records.len());
    let result = async {
        for record in records {
            handles.push(
                bounded(
                    cancellation,
                    "创建 DNS-01 记录",
                    dns.create_txt(&record.name, &record.value),
                )
                .await??,
            );
        }
        wait_for_dns_records(records, cancellation).await?;
        validate_challenges(challenges, cancellation).await
    }
    .await;
    cleanup_records(dns, handles).await;
    result
}

async fn validate_challenges(
    challenges: &[Challenge],
    cancellation: &CancellationToken,
) -> Result<()> {
    for challenge in challenges {
        let challenge = bounded(cancellation, "提交 DNS-01 挑战", challenge.validate()).await??;
        let challenge = bounded(
            cancellation,
            "等待 DNS-01 验证",
            challenge.wait_done(ACME_POLL_INTERVAL, ACME_POLL_ATTEMPTS),
        )
        .await??;
        if challenge.status != ChallengeStatus::Valid {
            return Err(ProxyError::Acme(format!(
                "DNS-01 验证失败: {:?}",
                challenge.error
            )));
        }
    }
    Ok(())
}

async fn finalize(
    request: IssuanceRequest,
    order: Order,
    cache_root: &Path,
    cancellation: &CancellationToken,
) -> Result<IssuedCertificate> {
    let order = bounded(
        cancellation,
        "等待 ACME 订单就绪",
        order.wait_ready(ACME_POLL_INTERVAL, ACME_POLL_ATTEMPTS),
    )
    .await??;
    if order.status != OrderStatus::Ready {
        return Err(ProxyError::Acme(format!(
            "ACME 订单未就绪: {:?}",
            order.status
        )));
    }
    let private_key = generate_key(request.account.key_algorithm)?;
    let order = bounded(
        cancellation,
        "提交证书请求",
        order.finalize(Csr::Automatic(private_key.clone())),
    )
    .await??;
    let order = bounded(
        cancellation,
        "等待证书签发",
        order.wait_done(ACME_POLL_INTERVAL, ACME_POLL_ATTEMPTS),
    )
    .await??;
    if order.status != OrderStatus::Valid {
        return Err(ProxyError::Acme(format!(
            "ACME 订单签发失败: {:?}",
            order.error
        )));
    }
    let certificates = bounded(cancellation, "下载证书链", order.certificate())
        .await??
        .ok_or_else(|| ProxyError::Acme("ACME 订单未返回证书链".into()))?;
    let mut certificate_pem = Vec::new();
    for certificate in certificates {
        certificate_pem.extend_from_slice(&certificate.to_pem()?);
    }
    let private_key_pem = private_key.private_key_to_pem_pkcs8()?;
    let cache = CertificateCache::new(cache_root, &request.certificate.id);
    let expires_at = cache
        .store_certificate(&request.certificate, &certificate_pem, &private_key_pem)
        .await?;
    Ok(IssuedCertificate {
        certificate_pem,
        private_key_pem,
        expires_at,
    })
}

async fn load_account(
    config: &AcmeAccountConfig,
    cache_root: &Path,
    cancellation: &CancellationToken,
    account_gate: &Arc<Semaphore>,
) -> Result<Arc<Account>> {
    let registration_permit = tokio::select! {
        () = cancellation.cancelled() => {
            return Err(ProxyError::Acme("等待 ACME 账户初始化时申请已取消".into()));
        }
        permit = Arc::clone(account_gate).acquire_owned() => {
            permit.map_err(|_| ProxyError::Acme("ACME 账户初始化通道已关闭".into()))?
        },
    };
    let cache = AccountCache::new(cache_root, &config.id);
    let cached = cache.load_private_key(config).await?;
    let (private_key, registered) = if let Some((pem, registered)) = cached {
        (PKey::private_key_from_pem(&pem)?, registered)
    } else {
        let private_key = generate_key(config.key_algorithm)?;
        cache
            .store_private_key(config, &private_key.private_key_to_pem_pkcs8()?, false)
            .await?;
        (private_key, false)
    };
    if registered {
        // 已注册账户只读取同一把稳定密钥，后续网络查询可以安全并发。
        drop(registration_permit);
    }
    let directory = bounded(
        cancellation,
        "读取 ACME 目录",
        DirectoryBuilder::new(config.directory_url().into()).build(),
    )
    .await??;
    let mut builder = AccountBuilder::new(directory);
    builder
        .private_key(private_key.clone())
        .contact(vec![format!("mailto:{}", config.email)])
        .terms_of_service_agreed(true);
    if registered {
        builder.only_return_existing(true);
    } else if config.provider == AcmeProvider::GoogleCloud {
        let key_id = config.eab_key_id.clone().ok_or_else(|| {
            ProxyError::InvalidConfig(format!("ACME 账户 {} 缺少 EAB Key ID", config.name))
        })?;
        let encoded = config.eab_hmac_key.as_ref().ok_or_else(|| {
            ProxyError::InvalidConfig(format!("ACME 账户 {} 缺少 EAB HMAC Key", config.name))
        })?;
        let key = decode_eab_hmac(encoded.expose())?;
        builder.external_account_binding(key_id, PKey::hmac(&key)?);
    }
    let account = bounded(cancellation, "创建或恢复 ACME 账户", builder.build()).await??;
    if !registered {
        cache
            .store_private_key(config, &private_key.private_key_to_pem_pkcs8()?, true)
            .await?;
    }
    Ok(account)
}

fn decode_eab_hmac(encoded: &str) -> Result<Zeroizing<Vec<u8>>> {
    URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| URL_SAFE.decode(encoded))
        .map(Zeroizing::new)
        .map_err(|_| ProxyError::InvalidConfig("Google EAB HMAC Key 不是 Base64URL".into()))
}

fn generate_key(algorithm: KeyAlgorithm) -> Result<PKey<Private>> {
    match algorithm {
        KeyAlgorithm::Ec256 => gen_ec_p256_private_key(),
        KeyAlgorithm::Rsa2048 => gen_rsa_private_key(2_048),
    }
    .map_err(Into::into)
}

async fn wait_for_dns_records(
    records: &[ManualDnsRecord],
    cancellation: &CancellationToken,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + DNS_PROPAGATION_TIMEOUT;
    loop {
        let visible = tokio::select! {
            () = cancellation.cancelled() => {
                return Err(ProxyError::Acme("证书申请已取消".into()));
            }
            result = dns_records_visible(records) => result?,
        };
        if visible {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ProxyError::Dns("等待 DNS TXT 记录传播超时".into()));
        }
        tokio::select! {
            () = cancellation.cancelled() => return Err(ProxyError::Acme("证书申请已取消".into())),
            () = tokio::time::sleep(DNS_POLL_INTERVAL) => {}
        }
    }
}

async fn verify_dns_records(
    records: &[ManualDnsRecord],
    cancellation: &CancellationToken,
) -> Result<()> {
    tokio::select! {
        () = cancellation.cancelled() => Err(ProxyError::Acme("证书申请已取消".into())),
        result = dns_records_visible(records) => {
            if result? {
                Ok(())
            } else {
                Err(ProxyError::Dns("公共 DNS 尚未解析到全部 TXT 挑战值".into()))
            }
        }
    }
}

async fn dns_records_visible(records: &[ManualDnsRecord]) -> Result<bool> {
    let resolver = TokioResolver::builder_tokio()
        .map_err(|error| ProxyError::Dns(format!("初始化 DNS 解析器失败: {error}")))?
        .build()
        .map_err(|error| ProxyError::Dns(format!("初始化 DNS 解析器失败: {error}")))?;
    let mut lookups = stream::iter(records.to_vec())
        .map(|record| {
            let resolver = resolver.clone();
            async move {
                let Ok(lookup) = resolver.txt_lookup(format!("{}.", record.name)).await else {
                    return false;
                };
                lookup.answers().iter().any(|answer| {
                    let RData::TXT(txt) = &answer.data else {
                        return false;
                    };
                    txt.txt_data
                        .iter()
                        .flat_map(|part| part.iter().copied())
                        .eq(record.value.bytes())
                })
            }
        })
        .buffer_unordered(DNS_LOOKUP_CONCURRENCY);
    while let Some(visible) = lookups.next().await {
        if !visible {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn cleanup_records(dns: &DnsClient, handles: Vec<DnsRecordHandle>) {
    let cleanup =
        stream::iter(handles).for_each_concurrent(DNS_CLEANUP_CONCURRENCY, |handle| async move {
            if let Err(error) = dns.delete_txt(handle).await {
                tracing::warn!(%error, "清理 DNS-01 记录失败");
            }
        });
    if tokio::time::timeout(DNS_CLEANUP_TIMEOUT, cleanup)
        .await
        .is_err()
    {
        tracing::warn!("清理 DNS-01 记录超时");
    }
}

async fn bounded<T>(
    cancellation: &CancellationToken,
    operation: &'static str,
    future: impl Future<Output = T>,
) -> Result<T> {
    tokio::select! {
        () = cancellation.cancelled() => Err(ProxyError::Acme(format!("{operation}已取消"))),
        result = tokio::time::timeout(ACME_OPERATION_TIMEOUT, future) => {
            result.map_err(|_| ProxyError::Acme(format!("{operation}超时")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_padded_and_unpadded_eab_keys() {
        for encoded in ["SGVsbG8", "SGVsbG8="] {
            assert_eq!(
                decode_eab_hmac(encoded).expect("EAB 密钥有效").as_slice(),
                b"Hello"
            );
        }
    }
}
