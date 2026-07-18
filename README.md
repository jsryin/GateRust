# GateRust

GateRust 当前实现 QUIC 内网穿透、反向代理与自动 SSL，以及统一管理这些模块的 Web 控制台。服务端通过 Cargo features 裁剪模块，并通过运行时参数选择启动模块。

## 功能

- 单 UDP 端口承载 QUIC/TLS，多路复用 TCP、UDP 和 SOCKS5 数据流。
- 分组使用独立的 32 到 124 字符密钥认证，密钥只在 TLS 加密的控制流中传输。
- 每条隧道可配置双向总流量限速、TCP 并发上限和 UDP 会话上限。
- 服务端可热添加、删除或修改公网监听；客户端可热更新服务及内网目标。
- 删除配置只阻止新连接，已经建立的数据流继续运行到自然结束。
- 控制帧、任务队列、连接数及 UDP 会话均有明确上限。
- Host/Path 反向代理支持 HTTP(S) 上游和 WebSocket，转发 body 保持流式。
- 支持 Let's Encrypt HTTP-01、TLS-ALPN-01、Cloudflare DNS-01 和 Google Trust Services。
- 多张证书按 SNI 选择，自动续期后热更新 TLS 上下文。
- Web 控制台使用 Argon2id 管理员认证和短期 JWT，支持原子更新 TOML、配置热重载状态 SSE 与客户端配置生成。
- 客户端首次启动自动创建用户配置，并通过仅监听回环地址的本机界面管理连接和服务。

详细协议、安全边界和配置字段见 [QUIC 隧道文档](docs/tunnel.md)。
代理配置、证书提供方和热重载语义见 [代理与自动 SSL 文档](docs/proxy.md)。

## Linux 一键部署

支持使用 systemd 的 x86_64 和 aarch64 Linux。安装脚本与 Release 版本绑定，并会校验下载文件的 SHA-256、版本和目标架构：

```bash
curl -fsSL https://github.com/jsryin/GateRust/releases/latest/download/gaterust.sh | sudo sh
```

脚本可交互选择 QUIC、Proxy 和 Web 模块，也支持无人值守安装。交互安装 QUIC 时默认生成自签名证书、私钥和最小可运行配置；交互安装 Proxy 时默认生成监听 `80/443`、尚无路由和证书的最小配置。无人值守安装可分别通过 `--init-tunnel` 和 `--init-proxy` 显式执行相同初始化。客户端需要信任 `/etc/gaterust/tunnel/server.pem`，并使用 TLS 服务器名称 `gaterust.local`。Web 未提供配置时会自动生成仅监听 `127.0.0.1:8080` 的安全初始配置，并一次性显示随机管理员密码；无人值守安装 QUIC 或 Proxy 时，未提供初始化参数或对应正式配置只会安装 `*.example.toml`，不会阻止其他模块启动：

```bash
sudo sh gaterust.sh install --modules tunnel,proxy
sudo sh gaterust.sh install --modules tunnel,web --init-tunnel --enable
sudo sh gaterust.sh install --modules proxy,web --init-proxy --enable
sudo sh gaterust.sh install --modules web --web-config /path/to/web.toml --enable
sudo sh gaterust.sh install --modules tunnel,proxy,web --init-tunnel --init-proxy --enable
```

安装后使用统一管理命令：

```bash
gaterust start
gaterust restart
gaterust status
gaterust logs
gaterust uninstall --modules proxy --yes
gaterust uninstall --all --yes
```

安装、更新、服务启停和卸载属于系统管理操作；已安装的 `gaterust` 会在需要时自动通过 `sudo` 获取管理员权限。状态和日志查询不需要提权。配置位于 `/etc/gaterust`，其中可能包含密钥和 API 凭据。安装 Web 后，经过认证的管理界面可以原子写入 QUIC 和 Proxy 正式配置；也可以使用 `sudoedit` 修改。新增模块配置后执行 `gaterust restart`，管理程序会校验配置并将该模块加入统一服务。运行数据位于 `/var/lib/gaterust`，日志通过 `journalctl -u gaterust.service` 查看。卸载默认删除模块配置及自动生成的证书和私钥；需要保留时显式传入 `--keep-config`。
已安装 Proxy 但尚无正式配置时，可执行 `gaterust install --modules proxy --init-proxy --enable` 生成最小配置并启动，无需先卸载。
重新执行 latest 一键安装命令会升级到脚本所属版本，并保留已安装模块、配置、数据和原服务启用状态。
同版本仅在修复安装时使用 `gaterust install --modules <模块列表> --force` 强制重装；正常安装和升级不需要该参数。

发布前可在一次性、运行 systemd 的 Linux VM 中构建并验证完整安装链路。默认安装全部模块，自动初始化并启动 QUIC、Proxy 和 Web，再验证 TLS 文件、配置、服务状态、Proxy HTTP 监听和 Web 管理界面。Proxy 初始化配置不包含路由和证书，自动 SSL 需要在 Web 中添加真实域名和证书配置：

```bash
sudo apt-get install -y musl-tools python3 curl openssl
./scripts/test-local-release.sh test
./scripts/test-local-release.sh uninstall
```

导入有效配置并验证服务启动时，将参数放在 `--` 之后传给安装器：

```bash
./scripts/test-local-release.sh test -- \
  --modules web --web-config /path/to/web.toml --enable
```

## 客户端下载

Release 提供 Linux x86_64/aarch64、Windows x86_64，以及 macOS Intel/Apple Silicon 客户端。客户端以带平台后缀的独立可执行文件发布，不需要随程序安装 `client.toml`。Linux 和 macOS 下载后需要添加执行权限：

```bash
chmod +x gaterust-client-x86_64-linux-musl
./gaterust-client-x86_64-linux-musl
```

Windows 客户端文件名为 `gaterust-client-x86_64-windows.exe`，双击即可启动。程序会自动打开 `http://127.0.0.1:47823/`，在本机界面中填写服务器地址、分组密钥、TLS 和本地服务。首次启动生成的配置位于：

- Windows：`%APPDATA%\GateRust\client.toml`
- macOS：`~/Library/Application Support/GateRust/client.toml`
- Linux：`${XDG_CONFIG_HOME:-$HOME/.config}/gaterust/client.toml`

重复打开程序只会唤回已有界面，不会启动重复连接。自动化或无桌面环境仍可使用 Release 中的 `client.example.toml`，并显式指定配置：

```bash
./gaterust-client-x86_64-linux-musl --no-open --config /path/to/client.toml
```

所有客户端文件均记录在 Release 的 `SHA256SUMS` 中。

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

基于 [server.example.toml](config/server.example.toml) 创建服务端配置后启动。客户端首次运行会自动创建初始配置并打开本机界面；客户端只需填写服务端分配的分组密钥和地址。使用公共 CA 证书时可省略 `name` 和 `ca_certificate`，TLS 名称会从服务器地址推导：

```bash
cargo run --release -p gaterust-server -- \
  --enable-tunnel --tunnel-config config/server.toml
cargo run --release -p gaterust-client
```

客户端也支持 `--config config/client.toml` 使用指定路径；文件不存在时会自动创建。首次启动时还会在配置文件旁生成 `client.toml.device-id`，用于在共享同一分组密钥时稳定区分设备。

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
