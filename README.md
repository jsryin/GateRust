# GateRust

GateRust 是一个基于 Rust 的内网穿透与反向代理工具，提供 QUIC 隧道、自动 SSL、Web 控制台和跨平台桌面客户端。

## 核心功能

- 通过单个 QUIC/TLS 端口承载 TCP、UDP 和 SOCKS5 流量。
- 支持分组密钥认证、流量限速、TCP 并发限制和 UDP 会话限制。
- 服务端和客户端均支持配置热更新，已有连接不受配置删除影响。
- 反向代理支持 Host/Path 路由、HTTP(S) 上游、WebSocket 和流式请求体。
- 支持 Let's Encrypt、Google Trust Services，以及 HTTP-01、TLS-ALPN-01、Cloudflare DNS-01 验证。
- Web 控制台提供管理员认证、配置管理、热重载状态和客户端配置生成。

配置字段参考 [QUIC 服务端示例](config/server.example.toml) 和 [代理与自动 SSL 示例](config/proxy.example.toml)。

## Linux 部署

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

配置位于 `/etc/gaterust`，运行数据位于 `/var/lib/gaterust`。配置可能包含密钥和 API 凭据，建议使用 `sudoedit` 修改。Web 模块未提供配置时会生成仅监听 `127.0.0.1:8080` 的初始配置和随机管理员密码。

`--init-tunnel` 生成的自签名证书位于 `/etc/gaterust/tunnel/server.pem`，客户端需信任该证书并使用 TLS 服务器名称 `gaterust.local`。重新执行一键安装命令可升级版本，并保留现有模块、配置和数据。

## 桌面客户端

[GitHub Releases](https://github.com/jsryin/GateRust/releases) 提供 Linux AppImage、Windows 安装程序和 macOS DMG，安装包已包含 Rust 隧道后台。

Linux AppImage 使用方式：

```bash
chmod +x gaterust-client-x64-linux.AppImage
./gaterust-client-x64-linux.AppImage
```

客户端配置位置：

- Windows：`%APPDATA%\GateRust\client.toml`
- macOS：`~/Library/Application Support/GateRust/client.toml`
- Linux：`${XDG_CONFIG_HOME:-$HOME/.config}/gaterust/client.toml`

可通过 `--config /path/to/client.toml` 指定配置文件。所有发布文件的校验值记录在 `SHA256SUMS` 中。

## 源码运行

开发环境需要 Rust 1.97.0、pnpm 和 OpenSSL。先生成分组密钥和本地 TLS 证书：

```bash
cargo run -p gaterust-client -- --generate-key
openssl req -x509 -newkey rsa:3072 -nodes -days 365 \
  -keyout certs/server-key.pem -out certs/server.pem \
  -subj '/CN=localhost' -addext 'subjectAltName=DNS:localhost'
cp config/server.example.toml config/server.toml
```

修改示例配置中的密钥和监听地址，然后启动 QUIC 服务端：

```bash
cargo run --release -p gaterust-server -- \
  --enable-tunnel --tunnel-config config/server.toml
```

启动桌面客户端：

```bash
cargo build -p gaterust-client
pnpm --dir client install
pnpm --dir client dev
```

启用 Web 控制台时，先构建前端并基于 [web.example.toml](config/web.example.toml) 创建配置。管理员密码哈希可通过以下命令生成：

```bash
pnpm --dir web install
pnpm --dir web build
cp config/web.example.toml config/web.toml
printf '%s' 'replace-with-a-strong-password' \
  | cargo run -p gaterust-server -- hash-password
cargo run -p gaterust-client -- --generate-key
```

将最后两条命令的输出分别写入 `admin_password_hash` 和 `jwt_secret`，再启动 Web 模块：

```bash
cargo run --release -p gaterust-server -- \
  --enable-web --web-config config/web.toml
```

在 Linux 或 WSL2 使用系统 Wine 构建 Windows 安装包：

```bash
USE_SYSTEM_WINE=true pnpm --dir client exec electron-builder \
  --win nsis --x64 --publish never
```

安装包输出到 `client/release/gaterust-client-x64-win.exe`。通过 `RUST_LOG` 调整日志级别；配置文件包含敏感信息时不得提交到版本库。
