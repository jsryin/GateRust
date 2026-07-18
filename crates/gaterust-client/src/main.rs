mod api;
mod app;
mod browser;
mod error;
mod paths;

use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "GateRust QUIC 内网穿透客户端")]
struct Arguments {
    /// 客户端 TOML 配置文件；默认使用当前用户的应用配置目录。
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// 不自动打开本机管理界面。
    #[arg(long)]
    no_open: bool,
    /// 生成一个 32 字符的随机分组密钥并退出。
    #[arg(long)]
    generate_key: bool,
}

#[tokio::main]
async fn main() {
    let arguments = Arguments::parse();
    if arguments.generate_key {
        println!("{}", gaterust_tunnel::generate_group_key());
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let result = async {
        let config_path = paths::config_path(arguments.config)?;
        let created = gaterust_tunnel::ClientConfig::ensure_exists(&config_path)?;
        if created {
            tracing::info!(path = %config_path.display(), "已创建客户端初始配置");
        }
        app::run(config_path, !arguments.no_open).await
    }
    .await;
    if let Err(error) = result {
        tracing::error!(%error, "客户端退出");
        std::process::exit(1);
    }
}
