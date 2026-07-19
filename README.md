# GateRust

GateRust 是一个基于 Rust 的内网穿透与反向代理工具，提供 QUIC 隧道、自动 SSL、Web 控制台和跨平台桌面客户端。

## 核心功能

- 通过单个 QUIC/TLS 端口承载 TCP、UDP 和 SOCKS5 流量。
- 支持分组密钥认证、流量限速、TCP 并发限制和 UDP 会话限制。
- 服务端和客户端均支持配置热更新，已有连接不受配置删除影响。
- 反向代理支持 Host/Path 路由、HTTP(S) 上游、WebSocket 和流式请求体。
- 支持 Let's Encrypt、Google Trust Services，以及 HTTP-01、TLS-ALPN-01、Cloudflare DNS-01 验证。
- Web 控制台提供管理员认证、配置管理、热重载状态和客户端配置生成。

## Linux 服务端部署

支持使用 systemd 的 x86_64 和 aarch64 Linux。安装脚本会校验版本、架构和 SHA-256：

```bash
curl -fsSL https://github.com/jsryin/GateRust/releases/latest/download/gaterust.sh | sudo sh
```

交互安装可选择 `tunnel`、`proxy` 和 `web` 模块；无人值守安装示例：

```bash
sudo sh gaterust.sh install \
  --modules tunnel,proxy,web \
  --init-tunnel --init-proxy --enable
```

常用管理命令：

```bash
gaterust start
gaterust restart
gaterust status
gaterust logs
gaterust uninstall --all --yes
```

配置位于 `/etc/gaterust`，运行数据位于 `/var/lib/gaterust`。`--init-tunnel` 生成的自签名证书位于 `/etc/gaterust/tunnel/server.pem`。


可通过 `--config /path/to/client.toml` 指定配置文件。

在 Linux 或 WSL2 使用系统 Wine 构建 Windows 便携版：

```bash
USE_SYSTEM_WINE=true pnpm --dir client exec electron-builder \
  --win portable --x64 --publish never
```

便携版输出到 `client/release/gaterust-client-x64-win.exe`。通过 `RUST_LOG` 调整日志级别。


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
