fn main() {
    if let Err(error) = gaterust_client_desktop::run() {
        eprintln!("GateRust Client 启动失败: {error}");
        std::process::exit(1);
    }
}
