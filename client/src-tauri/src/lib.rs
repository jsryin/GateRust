mod browser_data;
mod build_identity;
mod commands;
mod instance;

use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gaterust_client::ClientRuntime;
use tauri::{Manager as _, RunEvent, WebviewWindowBuilder};
use tracing_subscriber::EnvFilter;

/// 启动 `GateRust` 桌面客户端并管理隧道运行时生命周期。
///
/// # Errors
///
/// `Tauri` 应用配置无效或桌面运行时无法初始化时返回错误。
pub fn run() -> tauri::Result<()> {
    initialize_logging();
    let application = tauri::Builder::default()
        .setup(|application| {
            let instance = instance::PrimaryInstance::acquire(application)?;
            let browser_data_directory = browser_data::prepare(application)?;
            let config_path = config_path_from_arguments();
            let runtime =
                tauri::async_runtime::block_on(async { ClientRuntime::start(config_path) })?;
            application.manage(runtime);
            create_main_window(application, browser_data_directory)?;
            instance.start(application)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::cancel_login,
            commands::enable_tunnels,
            commands::disable_tunnels,
            commands::get_config,
            commands::get_status,
            commands::login,
        ])
        .build(tauri::generate_context!())?;

    let shutdown_started = Arc::new(AtomicBool::new(false));
    application.run(move |application, event| {
        let RunEvent::ExitRequested { api, .. } = event else {
            return;
        };
        if shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }

        // 系统退出也必须等待隧道任务释放网络资源。
        api.prevent_exit();
        let application = application.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(monitor) = application.try_state::<instance::InstanceMonitor>() {
                monitor.shutdown();
            }
            if let Some(runtime) = application.try_state::<ClientRuntime>()
                && let Err(error) = runtime.shutdown().await
            {
                tracing::error!(%error, "退出时停止客户端运行时失败");
            }
            application.exit(0);
        });
    });
    Ok(())
}

fn create_main_window(
    application: &tauri::App,
    browser_data_directory: Option<PathBuf>,
) -> tauri::Result<()> {
    let config = application
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == "main")
        .cloned()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "缺少 main 窗口配置")
        })?;
    let builder = WebviewWindowBuilder::from_config(application, &config)?;
    let builder = if let Some(directory) = browser_data_directory {
        builder.data_directory(directory)
    } else {
        builder
    };
    builder.build()?;
    Ok(())
}

fn config_path_from_arguments() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GATERUST_CLIENT_CONFIG").filter(|value| !value.is_empty())
    {
        return Some(path.into());
    }

    config_path_from_iter(std::env::args_os().skip(1))
}

fn config_path_from_iter(arguments: impl IntoIterator<Item = OsString>) -> Option<PathBuf> {
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--config" || argument == "-c" {
            return arguments
                .next()
                .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with('-'))
                .map(Into::into);
        }
    }
    None
}

fn initialize_logging() {
    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(_) => EnvFilter::new("info"),
    };
    if let Err(error) = tracing_subscriber::fmt().with_env_filter(filter).try_init() {
        eprintln!("初始化日志失败: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_explicit_config_argument() {
        let arguments = [
            OsString::from("--config"),
            OsString::from("custom/client.toml"),
        ];
        assert_eq!(
            config_path_from_iter(arguments),
            Some(PathBuf::from("custom/client.toml"))
        );
    }

    #[test]
    fn rejects_missing_config_argument_value() {
        let arguments = [OsString::from("-c"), OsString::from("--verbose")];
        assert_eq!(config_path_from_iter(arguments), None);
    }
}
