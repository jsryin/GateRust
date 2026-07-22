use gaterust_client::ClientRuntime;
use gaterust_tunnel::{ClientConfig, ClientStatus, ClientTunnel};
use serde::Serialize;
use tauri::State;

type CommandResult<T> = std::result::Result<T, String>;

#[derive(Serialize)]
pub(crate) struct StatusResponse {
    state: &'static str,
    message: Option<String>,
    server: Option<String>,
    device_id: Option<String>,
    retry_seconds: Option<u64>,
    tunnels: Vec<ClientTunnel>,
}

#[tauri::command]
pub(crate) async fn get_config(runtime: State<'_, ClientRuntime>) -> CommandResult<ClientConfig> {
    runtime.config().await.map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn login(
    runtime: State<'_, ClientRuntime>,
    server_address: String,
    key: String,
) -> CommandResult<ClientConfig> {
    runtime
        .login(server_address, key)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn connect_tunnels(
    runtime: State<'_, ClientRuntime>,
    names: Vec<String>,
) -> CommandResult<()> {
    runtime
        .connect_tunnels(names)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn disconnect_tunnels(runtime: State<'_, ClientRuntime>) -> CommandResult<()> {
    runtime
        .disconnect_tunnels()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn get_status(runtime: State<'_, ClientRuntime>) -> StatusResponse {
    StatusResponse::from(runtime.status())
}

#[tauri::command]
pub(crate) async fn shutdown(runtime: State<'_, ClientRuntime>) -> CommandResult<()> {
    if let Err(error) = runtime.shutdown().await {
        tracing::error!(%error, "等待客户端运行时退出失败");
    }
    Ok(())
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
            ClientStatus::Connected {
                server,
                device_id,
                tunnels,
            } => Self {
                state: "connected",
                server: Some(server),
                device_id: Some(device_id),
                tunnels,
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
            tunnels: Vec::new(),
        }
    }
}
