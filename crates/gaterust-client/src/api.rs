use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use gaterust_tunnel::{ClientConfig, ClientStatus};
use serde::Serialize;
use subtle::ConstantTimeEq as _;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::app::API_AUTHORITY;

const MAX_API_BODY_BYTES: usize = 128 * 1_024;
#[derive(Clone)]
struct ApiState {
    config_path: Arc<PathBuf>,
    status: watch::Receiver<ClientStatus>,
    revision: watch::Sender<u64>,
    cancellation: CancellationToken,
    token: Arc<str>,
}

#[derive(Serialize)]
struct SessionResponse {
    token: String,
}

#[derive(Serialize)]
struct ConfigResponse {
    path: String,
    config: ClientConfig,
}

#[derive(Serialize)]
struct StatusResponse {
    state: &'static str,
    message: Option<String>,
    server: Option<String>,
    device_id: Option<String>,
    retry_seconds: Option<u64>,
}

#[derive(Serialize)]
struct KeyResponse {
    key: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

struct ApiError {
    status: StatusCode,
    message: String,
}

pub(crate) fn router(
    config_path: PathBuf,
    status: watch::Receiver<ClientStatus>,
    revision: watch::Sender<u64>,
    cancellation: CancellationToken,
) -> Router {
    let state = ApiState {
        config_path: Arc::new(config_path),
        status,
        revision,
        cancellation,
        token: gaterust_tunnel::generate_group_key().into(),
    };
    let protected = Router::new()
        .route("/config", get(get_config).put(save_config))
        .route("/status", get(get_status))
        .route("/key", post(generate_key))
        .route("/shutdown", post(shutdown))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));
    Router::new()
        .route("/api/health", get(health))
        .route("/api/session", get(session))
        .nest("/api", protected)
        .layer(DefaultBodyLimit::max(MAX_API_BODY_BYTES))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(validate_host))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    ([("X-GateRust-Client", "1")], "gaterust-client")
}

async fn session(State(state): State<ApiState>) -> Json<SessionResponse> {
    Json(SessionResponse {
        token: state.token.to_string(),
    })
}

async fn get_config(State(state): State<ApiState>) -> Result<Json<ConfigResponse>, ApiError> {
    let path = Arc::clone(&state.config_path);
    let config = tokio::task::spawn_blocking(move || ClientConfig::read(path.as_ref()))
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
        .map_err(|error| ApiError::from_tunnel(&error))?;
    Ok(Json(ConfigResponse {
        path: state.config_path.display().to_string(),
        config,
    }))
}

async fn save_config(
    State(state): State<ApiState>,
    Json(config): Json<ClientConfig>,
) -> Result<Json<ConfigResponse>, ApiError> {
    let path = Arc::clone(&state.config_path);
    let config = tokio::task::spawn_blocking(move || {
        config.save(path.as_ref())?;
        Ok::<_, gaterust_tunnel::TunnelError>(config)
    })
    .await
    .map_err(|error| ApiError::internal(error.to_string()))?
    .map_err(|error| ApiError::from_tunnel(&error))?;
    state
        .revision
        .send_modify(|revision| *revision = revision.wrapping_add(1));
    Ok(Json(ConfigResponse {
        path: state.config_path.display().to_string(),
        config,
    }))
}

async fn get_status(State(state): State<ApiState>) -> Json<StatusResponse> {
    Json(StatusResponse::from(state.status.borrow().clone()))
}

async fn generate_key() -> Json<KeyResponse> {
    Json(KeyResponse {
        key: gaterust_tunnel::generate_group_key(),
    })
}

async fn shutdown(State(state): State<ApiState>) -> StatusCode {
    state.cancellation.cancel();
    StatusCode::NO_CONTENT
}

async fn require_token(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let candidate = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let authorized = candidate
        .is_some_and(|candidate| bool::from(candidate.as_bytes().ct_eq(state.token.as_bytes())));
    if authorized {
        next.run(request).await
    } else {
        ApiError::new(StatusCode::UNAUTHORIZED, "本机会话无效").into_response()
    }
}

async fn validate_host(request: Request, next: Next) -> Response {
    let valid = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == API_AUTHORITY);
    if valid {
        next.run(request).await
    } else {
        StatusCode::BAD_REQUEST.into_response()
    }
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; img-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

impl StatusResponse {
    fn from(status: ClientStatus) -> Self {
        match status {
            ClientStatus::Starting => Self::new("starting"),
            ClientStatus::Unconfigured { reason } => Self {
                state: "unconfigured",
                message: Some(reason),
                ..Self::new("unconfigured")
            },
            ClientStatus::Connecting { server } => Self {
                state: "connecting",
                server: Some(server),
                ..Self::new("connecting")
            },
            ClientStatus::Connected { server, device_id } => Self {
                state: "connected",
                server: Some(server),
                device_id: Some(device_id),
                ..Self::new("connected")
            },
            ClientStatus::Reconnecting {
                error,
                retry_seconds,
            } => Self {
                state: "reconnecting",
                message: Some(error),
                retry_seconds: Some(retry_seconds),
                ..Self::new("reconnecting")
            },
            ClientStatus::Stopped { reason } => Self {
                state: "stopped",
                message: reason,
                ..Self::new("stopped")
            },
        }
    }

    const fn new(state: &'static str) -> Self {
        Self {
            state,
            message: None,
            server: None,
            device_id: None,
            retry_seconds: None,
        }
    }
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        let message = message.into();
        tracing::error!(error = %message, "客户端本机 API 操作失败");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "客户端内部错误")
    }

    fn from_tunnel(error: &gaterust_tunnel::TunnelError) -> Self {
        if matches!(error, gaterust_tunnel::TunnelError::InvalidConfig(_)) {
            Self::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
        } else {
            Self::internal(error.to_string())
        }
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
