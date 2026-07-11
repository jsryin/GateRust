use std::path::PathBuf;

use clap::Parser;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "GateRust 统一服务端")]
struct Arguments {
    /// 启用 QUIC 内网穿透模块。
    #[cfg(feature = "tunnel")]
    #[arg(long)]
    enable_tunnel: bool,
    /// QUIC 内网穿透 TOML 配置文件。
    #[cfg(feature = "tunnel")]
    #[arg(long, default_value = "server.toml")]
    tunnel_config: PathBuf,
    /// 启用反向代理与自动 SSL 模块。
    #[cfg(feature = "proxy")]
    #[arg(long)]
    enable_proxy: bool,
    /// 反向代理 TOML 配置文件。
    #[cfg(feature = "proxy")]
    #[arg(long, default_value = "proxy.toml")]
    proxy_config: PathBuf,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let arguments = Arguments::parse();
    if let Err(error) = run(arguments).await {
        tracing::error!(%error, "服务端退出");
        std::process::exit(1);
    }
}

async fn run(arguments: Arguments) -> Result<(), String> {
    let cancellation = CancellationToken::new();
    let mut tasks = JoinSet::new();
    #[cfg(feature = "tunnel")]
    if arguments.enable_tunnel {
        let token = cancellation.child_token();
        tasks.spawn(async move {
            gaterust_tunnel::run_server_with_shutdown(arguments.tunnel_config, token)
                .await
                .map_err(|error| format!("隧道模块: {error}"))
        });
    }
    #[cfg(feature = "proxy")]
    if arguments.enable_proxy {
        let token = cancellation.child_token();
        tasks.spawn(async move {
            gaterust_proxy::run_proxy_with_shutdown(arguments.proxy_config, token)
                .await
                .map_err(|error| format!("代理模块: {error}"))
        });
    }
    if tasks.is_empty() {
        return Err("至少需要启用一个已编译模块".into());
    }

    let mut failure = None;
    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| format!("监听退出信号失败: {error}"))?;
            cancellation.cancel();
        }
        result = tasks.join_next() => {
            cancellation.cancel();
            match result {
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(error))) => failure = Some(error),
                Some(Err(error)) => failure = Some(format!("模块任务异常结束: {error}")),
                None => return Ok(()),
            }
        }
    }
    while let Some(result) = tasks.join_next().await {
        let error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(error) => Some(format!("模块任务异常结束: {error}")),
        };
        if failure.is_none() {
            failure = error;
        }
    }
    failure.map_or(Ok(()), Err)
}
