use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use bytes::Bytes;
use http::{
    HeaderMap, HeaderValue, Request, Response, StatusCode, Uri,
    header::{CONNECTION, HOST, TE, TRAILER, TRANSFER_ENCODING, UPGRADE},
};
use http_body_util::{BodyExt as _, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinSet,
};

use crate::{
    ProxyError,
    router::{Route, Router},
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type ProxyBody = BoxBody<Bytes, BoxError>;

#[derive(Clone)]
pub(crate) struct ProxyService {
    client: Client<HttpsConnector<HttpConnector>, Incoming>,
    routes: Arc<RwLock<Arc<Router>>>,
    upgrades: Arc<Mutex<JoinSet<()>>>,
}

impl ProxyService {
    pub(crate) fn new(routes: Arc<RwLock<Arc<Router>>>) -> Self {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder(hyper_util::rt::TokioExecutor::new()).build(https);
        Self {
            client,
            routes,
            upgrades: Arc::new(Mutex::new(JoinSet::new())),
        }
    }

    pub(crate) async fn replace_routes(&self, routes: Router) {
        *self.routes.write().await = Arc::new(routes);
    }

    pub(crate) async fn handle(
        &self,
        mut request: Request<Incoming>,
        remote: SocketAddr,
        tls_server_name: Option<&str>,
    ) -> Response<ProxyBody> {
        let Some(host) = request_host(&request) else {
            return response(StatusCode::BAD_REQUEST, "缺少有效 Host 请求头");
        };
        if !tls_host_matches(&host, tls_server_name) {
            return response(StatusCode::MISDIRECTED_REQUEST, "TLS SNI 与 Host 不一致");
        }
        let tls = tls_server_name.is_some();
        let route = {
            let routes = self.routes.read().await;
            routes.find(&host, request.uri().path())
        };
        let Some(route) = route else {
            return response(StatusCode::NOT_FOUND, "未匹配到代理路由");
        };
        if tls && !route.tls {
            return response(StatusCode::MISDIRECTED_REQUEST, "该路由未绑定 TLS 证书");
        }
        let upgrading = request.headers().contains_key(UPGRADE);
        let downstream_upgrade = upgrading.then(|| hyper::upgrade::on(&mut request));
        if let Err(error) = prepare_request(&mut request, &route, remote, tls) {
            tracing::warn!(route = %route.name, %error, "构造上游请求失败");
            return response(StatusCode::BAD_GATEWAY, "上游地址无效");
        }
        match self.client.request(request).await {
            Ok(mut upstream) => {
                if upstream.status() == StatusCode::SWITCHING_PROTOCOLS
                    && let Some(downstream_upgrade) = downstream_upgrade
                {
                    let upstream_upgrade = hyper::upgrade::on(&mut upstream);
                    let mut upgrades = self.upgrades.lock().await;
                    while upgrades.try_join_next().is_some() {}
                    upgrades.spawn(async move {
                        match tokio::try_join!(downstream_upgrade, upstream_upgrade) {
                            Ok((downstream, upstream)) => {
                                let mut downstream = hyper_util::rt::TokioIo::new(downstream);
                                let mut upstream = hyper_util::rt::TokioIo::new(upstream);
                                if let Err(error) =
                                    tokio::io::copy_bidirectional(&mut downstream, &mut upstream)
                                        .await
                                {
                                    tracing::debug!(%error, "升级连接转发结束");
                                }
                            }
                            Err(error) => tracing::debug!(%error, "建立升级连接失败"),
                        }
                    });
                }
                remove_hop_headers(upstream.headers_mut());
                upstream.map(|body| body.map_err(Into::into).boxed())
            }
            Err(error) => {
                tracing::warn!(route = %route.name, %error, "上游请求失败");
                response(StatusCode::BAD_GATEWAY, "上游服务不可用")
            }
        }
    }

    pub(crate) async fn shutdown(&self) {
        self.upgrades.lock().await.shutdown().await;
    }
}

fn prepare_request(
    request: &mut Request<Incoming>,
    route: &Route,
    remote: SocketAddr,
    tls: bool,
) -> Result<(), ProxyError> {
    let original_host = request.headers().get(HOST).cloned();
    let upgrade = request.headers().get(UPGRADE).cloned();
    let path = joined_path(&route.upstream, request.uri());
    *request.uri_mut() = Uri::builder()
        .scheme(
            route
                .upstream
                .scheme()
                .cloned()
                .unwrap_or(http::uri::Scheme::HTTP),
        )
        .authority(
            route
                .upstream
                .authority()
                .cloned()
                .ok_or_else(|| ProxyError::InvalidConfig("上游 URI 缺少 authority".into()))?,
        )
        .path_and_query(path)
        .build()?;
    remove_hop_headers(request.headers_mut());
    if let Some(upgrade) = upgrade {
        request.headers_mut().insert(UPGRADE, upgrade);
        request
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("upgrade"));
    }
    if let Some(authority) = route.upstream.authority() {
        request.headers_mut().insert(
            HOST,
            HeaderValue::from_str(authority.as_str()).map_err(http::Error::from)?,
        );
    }
    set_forwarded(
        request.headers_mut(),
        "x-forwarded-for",
        &remote.ip().to_string(),
    );
    set_forwarded(
        request.headers_mut(),
        "x-forwarded-proto",
        if tls { "https" } else { "http" },
    );
    if let Some(host) = original_host.and_then(|value| value.to_str().ok().map(str::to_owned)) {
        set_forwarded(request.headers_mut(), "x-forwarded-host", &host);
    }
    Ok(())
}

fn joined_path(upstream: &Uri, incoming: &Uri) -> String {
    let base = upstream.path().trim_end_matches('/');
    let incoming_path = incoming.path();
    let mut result = if base.is_empty() {
        incoming_path.to_owned()
    } else if incoming_path == "/" {
        format!("{base}/")
    } else {
        format!("{base}{incoming_path}")
    };
    if let Some(query) = incoming.query() {
        result.push('?');
        result.push_str(query);
    }
    result
}

fn request_host(request: &Request<Incoming>) -> Option<String> {
    let value = request.headers().get(HOST)?.to_str().ok()?;
    let authority: http::uri::Authority = value.parse().ok()?;
    Some(authority.host().trim_end_matches('.').to_ascii_lowercase())
}

fn tls_host_matches(host: &str, server_name: Option<&str>) -> bool {
    server_name.is_none_or(|server_name| server_name.eq_ignore_ascii_case(host))
}

fn set_forwarded(headers: &mut HeaderMap, name: &'static str, value: &str) {
    let Ok(value) = HeaderValue::from_str(value) else {
        return;
    };
    headers.insert(name, value);
}

fn remove_hop_headers(headers: &mut HeaderMap) {
    let connection_tokens: Vec<_> = headers
        .get(CONNECTION)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .filter_map(|name| {
                    http::header::HeaderName::from_bytes(name.trim().as_bytes()).ok()
                })
                .collect()
        })
        .unwrap_or_default();
    for name in connection_tokens {
        headers.remove(name);
    }
    for name in [CONNECTION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE] {
        headers.remove(name);
    }
}

pub(crate) fn response(status: StatusCode, message: &'static str) -> Response<ProxyBody> {
    let body = Full::new(Bytes::from_static(message.as_bytes()))
        .map_err(|never: Infallible| match never {})
        .boxed();
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_host_must_match_sni() {
        assert!(tls_host_matches("example.com", None));
        assert!(tls_host_matches("example.com", Some("EXAMPLE.COM")));
        assert!(!tls_host_matches("api.example.com", Some("example.com")));
    }

    #[test]
    fn joins_upstream_base_path_and_query() {
        let upstream: Uri = "http://127.0.0.1/base".parse().unwrap();
        let incoming: Uri = "/v1/items?page=2".parse().unwrap();
        assert_eq!(joined_path(&upstream, &incoming), "/base/v1/items?page=2");
    }
}
