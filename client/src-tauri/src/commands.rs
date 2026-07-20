use gaterust_client::ClientRuntime;
use gaterust_tunnel::{ClientConfig, ClientStatus};
use serde::Serialize;
use tauri::State;

type CommandResult<T> = std::result::Result<T, String>;

#[derive(Serialize)]
pub(crate) struct ConfigResponse {
    path: String,
    config: ClientConfig,
}

#[derive(Serialize)]
pub(crate) struct StatusResponse {
    state: &'static str,
    message: Option<String>,
    server: Option<String>,
    device_id: Option<String>,
    retry_seconds: Option<u64>,
}

#[tauri::command]
pub(crate) async fn get_config(runtime: State<'_, ClientRuntime>) -> CommandResult<ConfigResponse> {
    let config = runtime.config().await.map_err(|error| error.to_string())?;
    Ok(config_response(&runtime, config))
}

#[tauri::command]
pub(crate) async fn save_config(
    runtime: State<'_, ClientRuntime>,
    config: ClientConfig,
) -> CommandResult<ConfigResponse> {
    let config = runtime
        .save_config(config)
        .await
        .map_err(|error| error.to_string())?;
    Ok(config_response(&runtime, config))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn get_status(runtime: State<'_, ClientRuntime>) -> StatusResponse {
    StatusResponse::from(runtime.status())
}

#[tauri::command]
pub(crate) fn generate_key() -> String {
    gaterust_tunnel::generate_group_key()
}

#[tauri::command]
pub(crate) async fn shutdown(runtime: State<'_, ClientRuntime>) -> CommandResult<()> {
    if let Err(error) = runtime.shutdown().await {
        tracing::error!(%error, "等待客户端运行时退出失败");
    }
    Ok(())
}

fn config_response(runtime: &ClientRuntime, config: ClientConfig) -> ConfigResponse {
    ConfigResponse {
        path: runtime.config_path().display().to_string(),
        config,
    }
}

impl From<ClientStatus> for StatusResponse {
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
}

impl StatusResponse {
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
