use std::path::PathBuf;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use clap::Parser;
use rand::RngExt as _;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "GateRust QUIC 内网穿透客户端")]
struct Arguments {
    /// 客户端 TOML 配置文件。
    #[arg(short, long, default_value = "client.toml")]
    config: PathBuf,
    /// 生成一个 256-bit URL-safe Base64 分组密钥并退出。
    #[arg(long)]
    generate_key: bool,
}

#[tokio::main]
async fn main() {
    let arguments = Arguments::parse();
    if arguments.generate_key {
        let mut key = [0_u8; 32];
        rand::rng().fill(&mut key);
        println!("{}", URL_SAFE_NO_PAD.encode(key));
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(error) = gaterust_tunnel::run_client(arguments.config).await {
        tracing::error!(%error, "客户端退出");
        std::process::exit(1);
    }
}
