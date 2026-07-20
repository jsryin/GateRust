use std::path::PathBuf;

use clap::Parser;
use gaterust_client::prepare_config_path;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "GateRust QUIC 内网穿透客户端")]
struct Arguments {
    /// 客户端 TOML 配置文件；默认使用当前用户的应用配置目录。
    #[arg(short, long)]
    config: Option<PathBuf>,
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

    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(_) => EnvFilter::new("info"),
    };
    if let Err(error) = tracing_subscriber::fmt().with_env_filter(filter).try_init() {
        eprintln!("初始化日志失败: {error}");
    }
    let result = async {
        let config_path = prepare_config_path(arguments.config)?;
        gaterust_tunnel::run_client(config_path).await?;
        Ok::<_, gaterust_client::ClientError>(())
    }
    .await;
    if let Err(error) = result {
        tracing::error!(%error, "客户端退出");
        std::process::exit(1);
    }
}
