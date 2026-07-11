use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "GateRust QUIC 内网穿透服务端")]
struct Arguments {
    /// 服务端 TOML 配置文件。
    #[arg(short, long, default_value = "server.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let arguments = Arguments::parse();
    if let Err(error) = gaterust_tunnel::run_server(arguments.config).await {
        tracing::error!(%error, "服务端退出");
        std::process::exit(1);
    }
}
