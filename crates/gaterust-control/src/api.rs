use std::{convert::Infallible, net::SocketAddr};

use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post, put},
};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::WatchStream;
use tower_http::cors::CorsLayer;

use crate::{
    auth::{AuthService, LoginError},
    config::WebConfig,
    store::ConfigStore,
};

const MAX_API_BODY_BYTES: usize = 512 * 1_024;

#[derive(Clone)]
struct ApiState {
    auth: AuthService,
    store: ConfigStore,
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
    expires_at: u64,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

#[cfg(feature = "tunnel")]
#[derive(Deserialize)]
struct ClientConfigRequest {
    group: String,
    server_address: String,
    server_name: String,
    ca_certificate: String,
    services: Vec<gaterust_tunnel::ClientServiceConfig>,
}

#[cfg(feature = "tunnel")]
#[derive(Serialize)]
struct ClientConfigResponse {
    toml: String,
}

pub(crate) fn router(config: &WebConfig, auth: AuthService, store: ConfigStore) -> Router {
    let state = ApiState { auth, store };
    let protected = Router::new()
        .route("/config", get(get_config))
        .route("/events", get(events))
        .route("/auth/session", get(session))
        .merge(module_routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    let mut cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if !config.allowed_origins.is_empty() {
        let origins = config
            .allowed_origins
            .iter()
            .filter_map(|origin| origin.parse().ok())
            .collect::<Vec<_>>();
        cors = cors.allow_origin(origins);
    }
    Router::new()
        .route("/api/auth/login", post(login))
        .nest("/api", protected)
        .layer(DefaultBodyLimit::max(MAX_API_BODY_BYTES))
        .layer(cors)
        .with_state(state)
}

fn module_routes() -> Router<ApiState> {
    let router = Router::new();
    #[cfg(feature = "tunnel")]
    let router = router
        .route("/config/tunnel", put(save_tunnel))
        .route("/groups/key", post(generate_key))
        .route("/client-config", post(generate_client_config));
    #[cfg(feature = "proxy")]
    let router = router.route("/config/proxy", put(save_proxy));
    router
}

async fn login(
    State(state): State<ApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    match state
        .auth
        .login(address.ip(), request.username, request.password)
        .await
    {
        Ok((token, expires_at)) => Ok(Json(LoginResponse { token, expires_at })),
        Err(LoginError::InvalidCredentials) => {
            Err(ApiError::new(StatusCode::UNAUTHORIZED, "用户名或密码错误"))
        }
        Err(LoginError::RateLimited) => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "登录尝试过于频繁，请稍后重试",
        )),
        Err(LoginError::Internal) => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "认证服务暂时不可用",
        )),
    }
}

async fn require_auth(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if token.is_some_and(|token| state.auth.verify_token(token)) {
        return next.run(request).await;
    }
    ApiError::new(StatusCode::UNAUTHORIZED, "登录已失效").into_response()
}

async fn session() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn get_config(State(state): State<ApiState>) -> Json<crate::store::ConfigSnapshot> {
    Json(state.store.snapshot().await)
}

async fn events(
    State(state): State<ApiState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let stream = WatchStream::new(state.store.subscribe()).map(|dashboard| {
        let data = serde_json::to_string(&dashboard).unwrap_or_else(|_| "{}".into());
        Ok(Event::default().event("dashboard").data(data))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(feature = "tunnel")]
async fn save_tunnel(
    State(state): State<ApiState>,
    Json(config): Json<gaterust_tunnel::ServerConfig>,
) -> Result<Json<gaterust_tunnel::ServerConfig>, ApiError> {
    state
        .store
        .save_tunnel(config)
        .await
        .map(Json)
        .map_err(|error| ApiError::from_control(&error))
}

#[cfg(feature = "proxy")]
async fn save_proxy(
    State(state): State<ApiState>,
    Json(config): Json<gaterust_proxy::ProxyConfig>,
) -> Result<Json<gaterust_proxy::ProxyConfig>, ApiError> {
    state
        .store
        .save_proxy(config)
        .await
        .map(Json)
        .map_err(|error| ApiError::from_control(&error))
}

#[cfg(feature = "tunnel")]
async fn generate_key() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "key": gaterust_tunnel::generate_group_key() }))
}

#[cfg(feature = "tunnel")]
async fn generate_client_config(
    State(state): State<ApiState>,
    Json(request): Json<ClientConfigRequest>,
) -> Result<Json<ClientConfigResponse>, ApiError> {
    let snapshot = state.store.snapshot().await;
    let tunnel = snapshot
        .tunnel
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "尚未配置隧道模块"))?;
    let group = tunnel
        .groups
        .into_iter()
        .find(|group| group.name == request.group)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "分组不存在"))?;
    let config = gaterust_tunnel::ClientConfig {
        server: gaterust_tunnel::ClientServerConfig {
            address: request.server_address,
            name: request.server_name,
            ca_certificate: request.ca_certificate.into(),
        },
        group: gaterust_tunnel::ClientGroupConfig {
            name: group.name,
            key: group.key,
        },
        services: request.services,
    };
    config
        .validate()
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, error.to_string()))?;
    let content = toml::to_string_pretty(&config)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "生成客户端配置失败"))?;
    Ok(Json(ClientConfigResponse { toml: content }))
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn from_control(error: &crate::ControlError) -> Self {
        tracing::warn!(%error, "Web UI 配置操作失败");
        Self::new(StatusCode::BAD_REQUEST, error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}
