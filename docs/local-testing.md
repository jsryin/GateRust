可以。把它理解为：**服务端模块在一个进程启动，隧道客户端另开一个进程。**

当前仓库只有 `*.example.toml`，需要先准备好：

```bash
cp config/server.example.toml config/server.toml
cp config/client.example.toml config/client.toml
cp config/proxy.example.toml config/proxy.toml
cp config/web.example.toml config/web.toml
```

配置中的密钥、密码哈希和证书路径要替换掉，示例里的占位值不能直接运行。

**分别启动**

启动 QUIC 隧道服务端：

```bash
cargo run -p gaterust-server -- \
  --enable-tunnel \
  --tunnel-config config/server.toml
```

先构建隧道客户端后台：

```bash
cargo build -p gaterust-client
```

再启动 Electron 桌面界面，配置路径会透传给 Rust 后台：

```bash
pnpm --dir client install
GATERUST_CLIENT_CONFIG=../config/client.toml pnpm --dir client dev
```

启动 Web 控制台：

```bash
cargo run -p gaterust-server -- \
  --enable-web \
  --web-config config/web.toml
```

浏览器访问：

```text
http://127.0.0.1:8080
```

启动反向代理：

```bash
cargo run -p gaterust-server -- \
  --enable-proxy \
  --proxy-config config/proxy.toml
```

注意：示例代理配置使用真实域名和 ACME，不能直接在纯本机环境测试，需要改成本地域名、非特权端口，并去掉证书配置。

**一起启动所有服务端模块**

配置准备好后，服务端只需要一个终端：

```bash
RUST_LOG=info cargo run -p gaterust-server -- \
  --enable-web \
  --web-config config/web.toml \
  --enable-tunnel \
  --tunnel-config config/server.toml \
  --enable-proxy \
  --proxy-config config/proxy.toml
```

在运行服务端的终端按 `Ctrl+C`，统一关闭所有服务端模块。确认进程已经退出：

```bash
pgrep -af gaterust-server
```

如果终端已经关闭但进程仍在运行：

```bash
pkill -TERM -x gaterust-server
```

然后另开一个终端启动桌面客户端：

```bash
RUST_LOG=info GATERUST_CLIENT_CONFIG=../config/client.toml pnpm --dir client dev
```

如果正在开发前端，再开一个终端：

```bash
pnpm --dir web dev
```

访问 `http://127.0.0.1:5173`。如果只是测试已经构建好的页面，不需要运行这个命令，直接访问 Web 模块的 `8080` 端口即可。
