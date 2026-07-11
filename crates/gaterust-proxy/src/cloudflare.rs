use bytes::Bytes;
use http::{Method, Request, StatusCode, header};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{ProxyError, Result};

const API_ROOT: &str = "https://api.cloudflare.com/client/v4";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct CloudflareClient {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    token: String,
    zone_id: String,
}

#[derive(Serialize)]
struct CreateRecord<'a> {
    r#type: &'static str,
    name: &'a str,
    content: &'a str,
    ttl: u32,
}

#[derive(Deserialize)]
struct ApiResponse<T> {
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    result: Option<T>,
}

#[derive(Deserialize)]
struct ApiError {
    code: u64,
    message: String,
}

#[derive(Deserialize)]
struct CreatedRecord {
    id: String,
}

impl CloudflareClient {
    pub(crate) fn new(token: String, zone_id: String) -> Self {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_only()
            .enable_http1()
            .build();
        Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            token,
            zone_id,
        }
    }

    pub(crate) async fn create_txt(&self, name: &str, value: &str) -> Result<String> {
        let body = serde_json::to_vec(&CreateRecord {
            r#type: "TXT",
            name,
            content: value,
            ttl: 60,
        })?;
        let path = format!("{API_ROOT}/zones/{}/dns_records", self.zone_id);
        let response: ApiResponse<CreatedRecord> = self
            .request(Method::POST, &path, Full::new(Bytes::from(body)))
            .await?;
        response
            .result
            .map(|record| record.id)
            .ok_or_else(|| ProxyError::Cloudflare("创建 TXT 记录未返回记录 ID".into()))
    }

    pub(crate) async fn delete_record(&self, record_id: &str) -> Result<()> {
        let path = format!("{API_ROOT}/zones/{}/dns_records/{record_id}", self.zone_id);
        let _: ApiResponse<serde_json::Value> = self
            .request(Method::DELETE, &path, Full::new(Bytes::new()))
            .await?;
        Ok(())
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        uri: &str,
        body: Full<Bytes>,
    ) -> Result<ApiResponse<T>> {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)?;
        let response = tokio::time::timeout(REQUEST_TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| ProxyError::Cloudflare("API 请求超时".into()))?
            .map_err(|error| ProxyError::Cloudflare(format!("API 请求失败: {error}")))?;
        let status = response.status();
        let bytes = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|error| ProxyError::Cloudflare(format!("读取 API 响应失败: {error}")))?
            .to_bytes();
        let response: ApiResponse<T> = serde_json::from_slice(&bytes).map_err(|error| {
            ProxyError::Cloudflare(format!("解析 API 响应失败（HTTP {status}）: {error}"))
        })?;
        if status != StatusCode::OK || !response.success {
            let errors = response
                .errors
                .iter()
                .map(|error| format!("{}: {}", error.code, error.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ProxyError::Cloudflare(format!(
                "API 返回 HTTP {status}: {errors}"
            )));
        }
        Ok(response)
    }
}
