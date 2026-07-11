# GateRust

GateRust 当前实现 QUIC 内网穿透、反向代理与自动 SSL，以及统一管理这些模块的 Web 控制台。服务端通过 Cargo features 裁剪模块，并通过运行时参数选择启动模块。

## 功能

- 单 UDP 端口承载 QUIC/TLS，多路复用 TCP、UDP 和 SOCKS5 数据流。
- 分组使用独立 256-bit 密钥认证，密钥只在 TLS 加密的控制流中传输。
- 每条隧道可配置双向总流量限速、TCP 并发上限和 UDP 会话上限。
- 服务端可热添加、删除或修改公网监听；客户端可热更新服务及内网目标。
- 删除配置只阻止新连接，已经建立的数据流继续运行到自然结束。
- 控制帧、任务队列、连接数及 UDP 会话均有明确上限。
- Host/Path 反向代理支持 HTTP(S) 上游和 WebSocket，转发 body 保持流式。
- 支持 Let's Encrypt HTTP-01、TLS-ALPN-01、Cloudflare DNS-01 和 Google Trust Services。
- 多张证书按 SNI 选择，自动续期后热更新 TLS 上下文。
- Web 控制台使用 Argon2id 管理员认证和短期 JWT，支持原子更新 TOML、配置热重载状态 SSE 与客户端配置生成。

详细协议、安全边界和配置字段见 [QUIC 隧道文档](docs/tunnel.md)。
代理配置、证书提供方和热重载语义见 [代理与自动 SSL 文档](docs/proxy.md)。

## 快速开始

生成分组密钥：

```bash
cargo run -p gaterust-client -- --generate-key
```

准备由受信 CA 签发、用途包含 `serverAuth` 的 TLS 证书。开发环境可生成本地证书：

```bash
openssl req -x509 -newkey rsa:3072 -nodes -days 365 \
  -keyout certs/server-key.pem -out certs/server.pem \
  -subj '/CN=localhost' -addext 'subjectAltName=DNS:localhost'
```

基于 [server.example.toml](config/server.example.toml) 和 [client.example.toml](config/client.example.toml) 创建配置后启动：

```bash
cargo run --release -p gaterust-server -- \
  --enable-tunnel --tunnel-config config/server.toml
cargo run --release -p gaterust-client -- --config config/client.toml
```

通过 `RUST_LOG` 调整日志级别，例如 `RUST_LOG=gaterust_tunnel=debug`。配置文件可能包含密钥，不应提交到版本库；仓库已忽略常用运行时配置路径。

## Web 中心控制

先构建静态 SPA，并基于示例创建控制平面配置：

```bash
cd web
pnpm install
pnpm build
cd ..
cp config/web.example.toml config/web.toml
chmod 600 config/web.toml
printf '%s' 'replace-with-a-strong-password' \
  | cargo run -p gaterust-server -- hash-password
cargo run -p gaterust-client -- --generate-key
```

将最后两条命令的输出分别写入 `admin_password_hash` 和 `jwt_secret`，然后启动需要的模块：

```bash
cargo run --release -p gaterust-server -- \
  --enable-web --web-config config/web.toml \
  --enable-tunnel --tunnel-config config/server.toml \
  --enable-proxy --proxy-config config/proxy.toml
```

默认示例监听 `http://127.0.0.1:8080`。独立部署 `web/dist` 时，通过构建环境变量 `VITE_API_BASE` 指向 API 地址，并将 Pages 的完整 Origin 加入 `allowed_origins`。Bearer Token 只保存在浏览器 `sessionStorage` 中。
