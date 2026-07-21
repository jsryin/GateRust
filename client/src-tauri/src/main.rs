// Windows 发布版使用 GUI 子系统，避免启动时创建额外的控制台窗口。
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

fn main() {
    if let Err(error) = gaterust_client_desktop::run() {
        eprintln!("GateRust Client 启动失败: {error}");
        std::process::exit(1);
    }
}
