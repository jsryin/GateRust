use std::sync::Arc;

use dns_orchestrator_provider::{
    CreateDnsRecordRequest, DnsProvider as DnsProviderApi, PaginationParams, ProviderCredentials,
    RecordData, create_provider,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::Semaphore;

use crate::{DnsAccountConfig, DnsProvider, ProxyError, Result};

const DNS_PAGE_SIZE: u32 = 100;
const DNS_RECORD_TTL: u32 = 600;
const GODADDY_API: &str = "https://api.godaddy.com/v1";

pub(crate) enum DnsClient {
    Unified(Arc<dyn DnsProviderApi>),
    GoDaddy(GoDaddyClient),
}

pub(crate) enum DnsRecordHandle {
    Unified {
        domain_id: String,
        record_id: String,
    },
    GoDaddy {
        domain: String,
        name: String,
        value: String,
    },
}

impl DnsClient {
    pub(crate) fn from_config(
        config: &DnsAccountConfig,
        mutation_gate: Option<Arc<Semaphore>>,
    ) -> Result<Self> {
        let credentials = match config.provider {
            DnsProvider::Cloudflare => ProviderCredentials::Cloudflare {
                api_token: required(config.api_token.as_ref(), "api_token")?.into(),
            },
            DnsProvider::Aliyun => ProviderCredentials::Aliyun {
                access_key_id: required(config.access_key.as_ref(), "access_key")?.into(),
                access_key_secret: required(config.secret_key.as_ref(), "secret_key")?.into(),
            },
            DnsProvider::TencentCloud => ProviderCredentials::Dnspod {
                secret_id: required(config.access_key.as_ref(), "access_key")?.into(),
                secret_key: required(config.secret_key.as_ref(), "secret_key")?.into(),
            },
            DnsProvider::GoDaddy => {
                return Ok(Self::GoDaddy(GoDaddyClient::new(
                    required(config.access_key.as_ref(), "access_key")?,
                    required(config.secret_key.as_ref(), "secret_key")?,
                    mutation_gate.unwrap_or_else(|| Arc::new(Semaphore::new(1))),
                )));
            }
        };
        create_provider(credentials)
            .map(Self::Unified)
            .map_err(|error| ProxyError::Dns(error.to_string()))
    }

    pub(crate) async fn validate_credentials(&self) -> Result<()> {
        match self {
            Self::Unified(provider) => credentials_valid(
                provider
                    .validate_credentials()
                    .await
                    .map_err(|error| ProxyError::Dns(error.to_string()))?,
            ),
            Self::GoDaddy(provider) => provider.validate_credentials().await,
        }
    }

    pub(crate) async fn create_txt(&self, fqdn: &str, value: &str) -> Result<DnsRecordHandle> {
        match self {
            Self::Unified(provider) => {
                let (domain_id, zone) = find_unified_zone(provider, fqdn).await?;
                let record = provider
                    .create_record(&CreateDnsRecordRequest {
                        domain_id: domain_id.clone(),
                        name: relative_name(fqdn, &zone)?,
                        ttl: DNS_RECORD_TTL,
                        data: RecordData::TXT { text: value.into() },
                        proxied: Some(false),
                    })
                    .await
                    .map_err(|error| ProxyError::Dns(error.to_string()))?;
                Ok(DnsRecordHandle::Unified {
                    domain_id,
                    record_id: record.id,
                })
            }
            Self::GoDaddy(provider) => provider.create_txt(fqdn, value).await,
        }
    }

    pub(crate) async fn delete_txt(&self, handle: DnsRecordHandle) -> Result<()> {
        match (self, handle) {
            (
                Self::Unified(provider),
                DnsRecordHandle::Unified {
                    domain_id,
                    record_id,
                },
            ) => provider
                .delete_record(&record_id, &domain_id)
                .await
                .map_err(|error| ProxyError::Dns(error.to_string())),
            (
                Self::GoDaddy(provider),
                DnsRecordHandle::GoDaddy {
                    domain,
                    name,
                    value,
                },
            ) => provider.delete_txt(&domain, &name, &value).await,
            _ => Err(ProxyError::Dns("DNS 记录句柄与账户类型不匹配".into())),
        }
    }
}

fn credentials_valid(valid: bool) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ProxyError::Dns("DNS 凭据无效或权限不足".into()))
    }
}

async fn find_unified_zone(
    provider: &Arc<dyn DnsProviderApi>,
    fqdn: &str,
) -> Result<(String, String)> {
    let mut page = 1;
    let mut best: Option<(String, String)> = None;
    loop {
        let response = provider
            .list_domains(&PaginationParams {
                page,
                page_size: DNS_PAGE_SIZE,
            })
            .await
            .map_err(|error| ProxyError::Dns(error.to_string()))?;
        for zone in response.items {
            if domain_belongs_to_zone(fqdn, &zone.name)
                && best
                    .as_ref()
                    .is_none_or(|(_, current)| zone.name.len() > current.len())
            {
                best = Some((zone.id, zone.name));
            }
        }
        if !response.has_more {
            break;
        }
        page = page.saturating_add(1);
    }
    best.ok_or_else(|| ProxyError::Dns(format!("DNS 账户中找不到 {fqdn} 对应的托管域名")))
}

fn domain_belongs_to_zone(fqdn: &str, zone: &str) -> bool {
    fqdn == zone
        || fqdn
            .strip_suffix(zone)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn relative_name(fqdn: &str, zone: &str) -> Result<String> {
    if fqdn == zone {
        return Ok("@".into());
    }
    fqdn.strip_suffix(zone)
        .and_then(|prefix| prefix.strip_suffix('.'))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ProxyError::Dns(format!("记录 {fqdn} 不属于托管域名 {zone}")))
}

fn required<'a>(value: Option<&'a crate::SecretString>, field: &'static str) -> Result<&'a str> {
    value
        .map(crate::SecretString::expose)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::InvalidConfig(format!("DNS 账户缺少 {field}")))
}

pub(crate) struct GoDaddyClient {
    client: Client,
    authorization: String,
    mutation_gate: Arc<Semaphore>,
}

#[derive(Deserialize)]
struct GoDaddyDomain {
    domain: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct GoDaddyRecord {
    data: String,
    ttl: u32,
}

impl GoDaddyClient {
    fn new(access_key: &str, secret_key: &str, mutation_gate: Arc<Semaphore>) -> Self {
        Self {
            client: Client::new(),
            authorization: format!("sso-key {access_key}:{secret_key}"),
            mutation_gate,
        }
    }

    async fn validate_credentials(&self) -> Result<()> {
        self.request::<Vec<GoDaddyDomain>>(
            self.client
                .get(format!("{GODADDY_API}/domains"))
                .query(&[("limit", "1")]),
        )
        .await
        .map(|_| ())
    }

    async fn create_txt(&self, fqdn: &str, value: &str) -> Result<DnsRecordHandle> {
        let domain = self.find_zone(fqdn).await?;
        let name = relative_name(fqdn, &domain)?;
        let _permit = self.acquire_mutation_permit().await?;
        let mut records = self.get_txt(&domain, &name).await?;
        records.push(GoDaddyRecord {
            data: value.into(),
            ttl: DNS_RECORD_TTL,
        });
        self.put_txt(&domain, &name, &records).await?;
        Ok(DnsRecordHandle::GoDaddy {
            domain,
            name,
            value: value.into(),
        })
    }

    async fn delete_txt(&self, domain: &str, name: &str, value: &str) -> Result<()> {
        // GoDaddy 不提供记录 ID，删除前重新读取并只移除本次创建的值。
        let _permit = self.acquire_mutation_permit().await?;
        let mut records = self.get_txt(domain, name).await?;
        let Some(index) = records.iter().rposition(|record| record.data == value) else {
            return Ok(());
        };
        records.remove(index);
        if records.is_empty() {
            self.request_empty(
                self.client
                    .delete(format!("{GODADDY_API}/domains/{domain}/records/TXT/{name}")),
            )
            .await
        } else {
            self.put_txt(domain, name, &records).await
        }
    }

    async fn find_zone(&self, fqdn: &str) -> Result<String> {
        let domains = self
            .request::<Vec<GoDaddyDomain>>(
                self.client
                    .get(format!("{GODADDY_API}/domains"))
                    .query(&[("statuses", "ACTIVE"), ("limit", "1000")]),
            )
            .await?;
        domains
            .into_iter()
            .filter(|domain| domain_belongs_to_zone(fqdn, &domain.domain))
            .max_by_key(|domain| domain.domain.len())
            .map(|domain| domain.domain)
            .ok_or_else(|| ProxyError::Dns(format!("GoDaddy 账户中找不到 {fqdn} 对应的托管域名")))
    }

    async fn acquire_mutation_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.mutation_gate)
            .acquire_owned()
            .await
            .map_err(|_| ProxyError::Dns("GoDaddy DNS 更新通道已关闭".into()))
    }

    async fn get_txt(&self, domain: &str, name: &str) -> Result<Vec<GoDaddyRecord>> {
        let request = self
            .client
            .get(format!("{GODADDY_API}/domains/{domain}/records/TXT/{name}"));
        let response = request
            .header("Authorization", &self.authorization)
            .send()
            .await
            .map_err(|error| dns_http_error(&error))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        parse_response(response).await
    }

    async fn put_txt(&self, domain: &str, name: &str, records: &[GoDaddyRecord]) -> Result<()> {
        self.request_empty(
            self.client
                .put(format!("{GODADDY_API}/domains/{domain}/records/TXT/{name}"))
                .json(records),
        )
        .await
    }

    async fn request<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T> {
        let response = request
            .header("Authorization", &self.authorization)
            .send()
            .await
            .map_err(|error| dns_http_error(&error))?;
        parse_response(response).await
    }

    async fn request_empty(&self, request: reqwest::RequestBuilder) -> Result<()> {
        let response = request
            .header("Authorization", &self.authorization)
            .send()
            .await
            .map_err(|error| dns_http_error(&error))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(response_error(response).await)
        }
    }
}

async fn parse_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    response
        .json()
        .await
        .map_err(|error| ProxyError::Dns(format!("解析 GoDaddy API 响应失败: {error}")))
}

async fn response_error(response: reqwest::Response) -> ProxyError {
    let status = response.status();
    let message = response
        .text()
        .await
        .unwrap_or_else(|_| "无法读取响应内容".into());
    ProxyError::Dns(format!("GoDaddy API 返回 HTTP {status}: {message}"))
}

fn dns_http_error(error: &reqwest::Error) -> ProxyError {
    ProxyError::Dns(format!("GoDaddy API 请求失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_relative_record_name() {
        assert_eq!(
            relative_name("_acme-challenge.www.example.com", "example.com").expect("记录属于域名"),
            "_acme-challenge.www"
        );
        assert!(relative_name("badexample.com", "example.com").is_err());
    }

    #[test]
    fn rejects_negative_credential_validation() {
        assert!(credentials_valid(true).is_ok());
        assert!(credentials_valid(false).is_err());
    }
}
