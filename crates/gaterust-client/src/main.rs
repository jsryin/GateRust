use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "GateRust QUIC 内网穿透客户端")]
struct Arguments {
    /// 客户端 TOML 配置文件。
    #[arg(short, long, default_value = "client.toml")]
    config: PathBuf,
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
    if let Err(error) = gaterust_tunnel::run_client(arguments.config).await {
        tracing::error!(%error, "客户端退出");
        std::process::exit(1);
    }
}
