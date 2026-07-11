# 反向代理与自动 SSL 模块

## 运行模型

代理同时监听 HTTP 和 HTTPS，按规范化后的 `Host` 与路径最长前缀选择上游。精确 Host 优先于 `*.example.com` 通配规则，`/api` 只匹配 `/api` 和 `/api/...`，不会误匹配 `/apix`。请求和响应 body 全程流式转发；HTTP Upgrade（包括 WebSocket）切换为双向字节流。

HTTP 与 HTTPS 共用 `max_connections` 信号量，因此并发连接和握手数量有硬上限。Cloudflare API 响应限制为 1 MiB，TLS 握手限制为 10 秒。模块退出时会取消并等待监听、连接、Upgrade 和证书任务。

代理会移除逐跳头，重写上游 `Host`，并根据当前连接设置 `X-Forwarded-For`、`X-Forwarded-Host`、`X-Forwarded-Proto`，不信任客户端传入的同名头。HTTPS 请求的 SNI 必须与 `Host` 一致。上游支持 `http://` 和 `https://`，HTTPS 始终验证系统 Web PKI 信任链。

## 证书

每个 `certificates` 条目是一张独立证书，路由通过 `certificate` 名称选择证书。配置校验会拒绝不存在的引用，以及证书 SAN 不覆盖路由 Host 的组合。多个证书通过 SNI 共用 443 端口。

- `lets_encrypt` + `http-01`：由 `rustls-acme` 申请；公网 80 端口必须直达本进程。
- `lets_encrypt` + `tls-alpn-01`：由 `rustls-acme` 申请；公网 443 端口必须直达本进程。
- `lets_encrypt` + `cloudflare-dns-01`：由 `instant-acme` 申请，支持通配证书。
- `google_trust_services` + `cloudflare-dns-01`：使用 Google Public CA ACME 和 EAB 凭据，DNS 验证由 Cloudflare API 完成。

Google Cloud Load Balancer 的托管证书不能导出私钥，因此不能绑定到本地 GateRust。这里的 Google 支持是 Google Trust Services Public CA 的 ACME 证书；EAB Key ID 与 HMAC Key 可通过 Google Cloud Public CA 获取。

`production` 默认是 `false`。应先用 staging 验证端口、DNS 和权限，再切换生产 CA，避免触发生产环境速率限制。Cloudflare Token 应只授予目标 Zone 的 `DNS:Edit` 权限。

证书、私钥和 ACME 账户写入 `cache_dir/<certificate-name>`。Unix 下目录权限设为 `0700`、自管文件设为 `0600`，写入采用同目录临时文件加 rename。DNS-01 缓存会校验 CA 目录、联系邮箱和域名集合，配置变化后不会复用旧环境证书。DNS-01 证书每 60 天续签；失败后每小时重试。Cloudflare 请求限制为 30 秒，ACME 步骤限制为 3 分钟。`rustls-acme` 按证书有效期自动安排续签。

## 热重载

模块监听配置文件所在目录，支持控制面采用临时文件加 rename 的原子替换方式。

- 路由增删和上游变化会原子替换路由快照，已有请求与 Upgrade 连接不受影响。
- 证书新增、删除或参数变化会独立重建对应证书任务，不影响其他域名。
- ACME 部署缓存证书或新证书后，SNI 解析器立即使用新证书，无需重启监听。
- 无效配置不会替换当前配置。
- `http_bind`、`https_bind`、`cache_dir`、`max_connections` 是监听级参数，变化需要重启进程。

## 配置

完整示例见 [proxy.example.toml](../config/proxy.example.toml)。所有未知字段都会被拒绝。

- `proxy.http_bind`、`proxy.https_bind`：HTTP 和 HTTPS 监听地址，不能相同。
- `proxy.cache_dir`：证书与账户缓存目录；相对路径以配置文件目录为基准。
- `proxy.max_connections`：HTTP 与 HTTPS 合计连接上限，默认 2048。
- `certificates[].domains`：单张证书 1 至 100 个域名；不同证书不能声明同一域名。
- `certificates[].dns_propagation_seconds`：DNS 写入后等待时间，范围 1 至 600 秒，默认 30。
- `routes[].path_prefix`：以 `/` 开头的路径前缀。
- `routes[].upstream`：包含 scheme 和 authority 的 HTTP(S) URI，可带基础路径但不能带查询参数。
- `routes[].certificate`：可选证书名称；未绑定证书的路由只能通过 HTTP 使用。

## 启动

仅构建和启动代理模块：

```bash
cargo run --release -p gaterust-server \
  --no-default-features --features proxy -- \
  --enable-proxy --proxy-config config/proxy.toml
```

默认 features 同时包含 `proxy` 和 `tunnel`，运行时可同时启用：

```bash
cargo run --release -p gaterust-server -- \
  --enable-tunnel --tunnel-config config/server.toml \
  --enable-proxy --proxy-config config/proxy.toml
```

配置文件包含 API Token、EAB 和账户私钥，不应提交到版本库。
