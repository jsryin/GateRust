#[cfg(feature = "web")]
use std::io::Read as _;

use clap::{Args, Parser, Subcommand};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "GateRust 统一服务端")]
struct Arguments {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    server: ServerArguments,
}

#[derive(Args)]
struct ServerArguments {
    /// 启用 Web 中心控制模块。
    #[cfg(feature = "web")]
    #[arg(long)]
    enable_web: bool,
    /// Web 中心控制 TOML 配置文件。
    #[cfg(feature = "web")]
    #[arg(long, default_value = "web.toml")]
    web_config: std::path::PathBuf,
    /// 启用 QUIC 内网穿透模块。
    #[cfg(feature = "tunnel")]
    #[arg(long)]
    enable_tunnel: bool,
    /// QUIC 内网穿透 TOML 配置文件。
    #[cfg(any(feature = "tunnel", feature = "web"))]
    #[arg(long, default_value = "server.toml")]
    tunnel_config: std::path::PathBuf,
    /// 启用反向代理与自动 SSL 模块。
    #[cfg(feature = "proxy")]
    #[arg(long)]
    enable_proxy: bool,
    /// 反向代理 TOML 配置文件。
    #[cfg(any(feature = "proxy", feature = "web"))]
    #[arg(long, default_value = "proxy.toml")]
    proxy_config: std::path::PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// 从标准输入读取管理员密码，输出 Argon2id 哈希后退出。
    #[cfg(feature = "web")]
    HashPassword,
    /// 校验所选模块的配置后退出。
    CheckConfig(ServerArguments),
}

#[tokio::main]
async fn main() {
    let arguments = Arguments::parse();
    if let Some(command) = arguments.command {
        match command {
            #[cfg(feature = "web")]
            Command::HashPassword => hash_password(),
            Command::CheckConfig(arguments) => {
                if let Err(error) = check_config(&arguments) {
                    eprintln!("配置校验失败: {error}");
                    std::process::exit(1);
                }
                println!("配置校验通过");
            }
        }
        return;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(error) = run(arguments.server).await {
        tracing::error!(%error, "服务端退出");
        std::process::exit(1);
    }
}

#[cfg(feature = "web")]
fn hash_password() {
    let mut password = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut password) {
        eprintln!("读取密码失败: {error}");
        std::process::exit(1);
    }
    let password = password.trim_end_matches(['\r', '\n']);
    match gaterust_control::hash_password(password.as_bytes()) {
        Ok(hash) => println!("{hash}"),
        Err(error) => {
            eprintln!("生成密码哈希失败: {error}");
            std::process::exit(1);
        }
    }
}

fn check_config(arguments: &ServerArguments) -> Result<(), String> {
    let enabled = 0_u8;
    #[cfg(feature = "tunnel")]
    let enabled = if arguments.enable_tunnel {
        gaterust_tunnel::check_server_config(&arguments.tunnel_config)
            .map_err(|error| format!("隧道模块: {error}"))?;
        enabled + 1
    } else {
        enabled
    };
    #[cfg(feature = "proxy")]
    let enabled = if arguments.enable_proxy {
        gaterust_proxy::ProxyConfig::load(&arguments.proxy_config)
            .map_err(|error| format!("代理模块: {error}"))?;
        enabled + 1
    } else {
        enabled
    };
    #[cfg(feature = "web")]
    let enabled = if arguments.enable_web {
        gaterust_control::check_config(&arguments.web_config)
            .map_err(|error| format!("Web 控制模块: {error}"))?;
        enabled + 1
    } else {
        enabled
    };
    let _ = arguments;
    if enabled == 0 {
        return Err("至少需要启用一个已编译模块".into());
    }
    Ok(())
}

async fn run(arguments: ServerArguments) -> Result<(), String> {
    let cancellation = CancellationToken::new();
    let mut tasks = JoinSet::new();
    #[cfg(feature = "web")]
    let tunnel_config = arguments.tunnel_config.clone();
    #[cfg(feature = "web")]
    let proxy_config = arguments.proxy_config.clone();
    #[cfg(feature = "tunnel")]
    let tunnel_runtime = arguments
        .enable_tunnel
        .then(gaterust_tunnel::TunnelRuntime::new);
    #[cfg(all(feature = "tunnel", feature = "web"))]
    let control_tunnel_runtime = tunnel_runtime.clone();
    #[cfg(feature = "tunnel")]
    if let Some(runtime) = tunnel_runtime {
        let token = cancellation.child_token();
        tasks.spawn(async move {
            gaterust_tunnel::run_server_with_runtime(arguments.tunnel_config, runtime, token)
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
    #[cfg(feature = "web")]
    if arguments.enable_web {
        let token = cancellation.child_token();
        let options = gaterust_control::ControlOptions {
            tunnel_config,
            #[cfg(feature = "tunnel")]
            tunnel_runtime: control_tunnel_runtime,
            #[cfg(not(feature = "tunnel"))]
            tunnel_runtime: None,
            proxy_config,
            #[cfg(feature = "tunnel")]
            tunnel_enabled: arguments.enable_tunnel,
            #[cfg(not(feature = "tunnel"))]
            tunnel_enabled: false,
            #[cfg(feature = "proxy")]
            proxy_enabled: arguments.enable_proxy,
            #[cfg(not(feature = "proxy"))]
            proxy_enabled: false,
        };
        tasks.spawn(async move {
            gaterust_control::run_control_with_shutdown(&arguments.web_config, options, token)
                .await
                .map_err(|error| format!("Web 控制模块: {error}"))
        });
    }
    if tasks.is_empty() {
        let _ = arguments;
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
