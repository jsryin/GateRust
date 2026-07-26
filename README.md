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

## 桌面客户端

首次点击“获取配置”时，如果客户端配置目录没有可用的 `server.pem`，客户端会通过分组密钥双向证明验证服务端，并将证书保存为 `server.pem` 后重新建立受信任连接。获取期间可主动取消，整个过程最多持续 60 秒；取消、超时或密钥错误时不会保存候选配置，也不会在后台继续获取。证书引导协议要求客户端与服务端同时升级到相同版本。

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
target/x86_64-pc-windows-msvc/release/bundle/nsis/GateRust Client_0.1.0_x64-setup.exe


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
