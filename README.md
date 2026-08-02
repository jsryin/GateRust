# GateRust

GateRust 是一个基于 Rust 的内网穿透与反向代理工具，提供 QUIC 隧道、自动 SSL、Web 控制台和跨平台桌面客户端。

## 核心功能

- 通过单个 QUIC/TLS 端口承载 TCP、UDP 和 SOCKS5 流量。
- 支持分组密钥认证、流量限速、TCP 并发限制和 UDP 会话限制。
- 服务端和客户端均支持配置热更新；TCP 已有连接可自然结束，UDP 监听重载会关闭旧会话，分组密钥变更会立即撤销对应客户端会话。
- 反向代理支持 Host/Path 路由、HTTP(S) 上游、WebSocket 和流式请求体。
- 支持 Let's Encrypt、Google Trust Services，以及 HTTP-01、TLS-ALPN-01、Cloudflare DNS-01 验证。
- Web 控制台提供管理员认证、配置管理、热重载状态和客户端配置生成。

## Release 二进制

每个版本只发布服务端和命令行客户端的原始可执行文件，覆盖 Linux amd64/arm/arm64、macOS amd64/arm64 和 Windows amd64/arm64。文件名中的版本号不包含标签前导 `v`，Windows 文件带 `.exe` 后缀。

以 Linux amd64 服务端为例：

```bash
version=0.1.1-beta.1
curl -fLO "https://github.com/jsryin/GateRust/releases/download/v${version}/gaterust_server_${version}_linux_amd64"
chmod +x "gaterust_server_${version}_linux_amd64"
```

配置示例和 systemd unit 位于源码仓库的 `config` 与 `scripts` 目录。Web 控制台静态文件需单独构建或部署。

服务端隧道证书默认位于配置指定的路径；使用仓库 systemd 示例部署时建议放在 `/etc/gaterust/tunnel/server.pem`。

旧版本的 `--init-tunnel` 可能生成 `Basic Constraints CA:TRUE` 的证书，新版本会在服务端启动前拒绝将其作为叶证书。可先用以下命令确认：

```bash
sudo openssl x509 -in /etc/gaterust/tunnel/server.pem -noout -ext basicConstraints
```

如果输出包含 `CA:TRUE`，请备份并生成新的服务端叶证书：

```bash
sudo cp -a /etc/gaterust/tunnel/server.pem /etc/gaterust/tunnel/server.pem.ca-true.bak
sudo cp -a /etc/gaterust/tunnel/server-key.pem /etc/gaterust/tunnel/server-key.pem.ca-true.bak
sudo openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 3650 \
  -subj '/CN=gaterust.local' \
  -addext 'subjectAltName=DNS:gaterust.local' \
  -addext 'basicConstraints=critical,CA:FALSE' \
  -addext 'keyUsage=critical,digitalSignature,keyEncipherment' \
  -addext 'extendedKeyUsage=serverAuth' \
  -keyout /etc/gaterust/tunnel/server-key.pem.new \
  -out /etc/gaterust/tunnel/server.pem.new
sudo systemctl stop gaterust.service
sudo install -o root -g gaterust -m 0640 /etc/gaterust/tunnel/server.pem.new /etc/gaterust/tunnel/server.pem
sudo install -o root -g gaterust -m 0640 /etc/gaterust/tunnel/server-key.pem.new /etc/gaterust/tunnel/server-key.pem
sudo systemctl start gaterust.service
```

升级后的桌面客户端检测到本地受管证书为 `CA:TRUE` 时，会重新执行分组密钥证明并安全替换该证书。

## 发布流程

发布 GitHub Tag：

```bash
./scripts/release.mjs 0.2.0
```

默认只创建本地提交和 Tag。确认无误后可手动推送，或者增加 `--push`，将当前分支和 Tag 原子推送到 `origin` 并触发 GitHub Actions：

```bash
./scripts/release.mjs 0.2.0 --push
```

## 桌面客户端

每次点击“获取配置”时，客户端都会通过分组密钥双向证明重新验证服务端证书，再以唯一文件名保存候选证书并建立受信任连接。只有正常 QUIC 认证成功后才提交证书和配置；取消、超时、密钥错误或网络失败会保留原配置及现有会话。整个过程最多持续 60 秒，证书引导协议要求客户端与服务端同时升级到相同版本。

隧道配置中，单条 TCP/SOCKS5 隧道最多允许 512 个并发连接，单条 UDP 隧道最多允许 128 个会话。服务端还使用跨隧道、跨热更新代际的全局数据流、UDP 会话和 16 MiB UDP 排队字节预算。当前 SOCKS5 入口仅支持免认证，因此必须监听 `127.0.0.0/8` 或 `::1`；如需公网使用，应在外层部署经过认证和访问控制的代理。

桌面客户端启动后会自动建立控制会话以获取隧道目录，但不会自动启用上次选择的隧道。“启用”和“停用”操作通过控制通道发送，并在服务端确认最终隧道状态后完成；运行中的选择不会写入客户端配置文件。

服务器地址必须能从客户端所在系统直接访问；`localhost` 只指客户端自己的网络命名空间。例如 Windows 客户端访问 WSL2 NAT 内的 QUIC/UDP 服务时，应使用 Windows 可达的 WSL 地址或启用支持 UDP 的镜像网络，而不能依赖 TCP localhost 转发。

安装前端依赖并启动开发环境：

```bash
pnpm --dir client install --frozen-lockfile
pnpm --dir client dev
```

生成当前平台安装包：

```bash
pnpm --dir client build
```

生成可直接运行的 Windows .exe，不打安装包：
```
RC=llvm-rc-21 pnpm --dir client exec tauri build \
    --runner cargo-xwin \
    --target x86_64-pc-windows-msvc \
    --no-bundle \
    -- --locked
```
安装包输出位置：
target/x86_64-pc-windows-msvc/release/bundle/nsis/GateRust Client_*_x64-setup.exe


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
