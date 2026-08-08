# GateRust

GateRust 是一个基于 Rust 的内网穿透与反向代理工具，提供 QUIC 隧道、自动 SSL、Web 控制台和跨平台桌面客户端。

## 核心功能

- 通过单个 QUIC/TLS 端口承载 TCP、UDP 和 SOCKS5 流量。
- 支持分组密钥认证、流量限速、TCP 并发限制和 UDP 会话限制。
- 服务端和客户端均支持配置热更新；TCP 已有连接可自然结束，UDP 监听重载会关闭旧会话，分组密钥变更会立即撤销对应客户端会话。
- 反向代理支持 Host/Path 路由、HTTP(S) 上游、WebSocket 和流式请求体。
- 支持 Let's Encrypt、Google Trust Services，以及 HTTP-01、TLS-ALPN-01、Cloudflare DNS-01 验证。
- Web 控制台提供管理员认证、配置管理、热重载状态和客户端配置生成。

## 快速开始

### 安装服务端

支持运行 systemd 的 Linux 发行版，以及使用 OpenRC 的 Alpine Linux 3.21；支持 x86_64 和 aarch64 架构。复制并执行以下命令，下载安装脚本、授予执行权限并进入交互式安装：

```bash
version=v0.1.1-beta.2
curl -fsSLO "https://github.com/jsryin/GateRust/releases/download/${version}/gaterust.sh"
chmod +x gaterust.sh
sudo ./gaterust.sh
```

安装完成后，使用 `sudo gaterust status` 查看状态，使用 `sudo gaterust logs` 查看日志。

### Windows 客户端

本地生成可直接运行的 Windows `.exe`，不打安装包：

```bash
RC=llvm-rc-21 pnpm --dir client exec tauri build \
    --runner cargo-xwin \
    --target x86_64-pc-windows-msvc \
    --no-bundle \
    -- --locked
```

可执行文件输出位置：`target/x86_64-pc-windows-msvc/release/gaterust-client-desktop.exe`。

## 本地测试

准备好 `config/server.toml`、`config/proxy.toml` 和 `config/web.toml` 后启动服务端：

```bash
RUST_LOG=info cargo run -p gaterust-server -- \
  --enable-web \
  --web-config config/web.toml \
  --enable-tunnel \
  --tunnel-config config/server.toml \
  --enable-proxy \
  --proxy-config config/proxy.toml
```
