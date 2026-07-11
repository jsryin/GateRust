# QUIC 内网穿透模块

## 运行模型

服务端在一个 UDP 地址上接受 QUIC 连接。客户端先建立一条长期控制流，提交协议版本、分组名、分组密钥以及服务名和类型。服务端以常量时间比较分组密钥；认证后，同一分组的新客户端会替换旧客户端，避免同名服务出现不确定路由。

公网流量到达后，服务端为每个 TCP/SOCKS5 连接或 UDP 来源会话打开独立 QUIC 双向流。客户端连接内网目标并确认成功后才开始转发。内网目标地址仅保存在客户端配置中，固定 TCP/UDP 目标不会发送给服务端。

UDP 按公网来源地址建立会话，每个会话使用一条带长度帧的 QUIC 流，以保留数据报边界。会话队列、会话总数和空闲时间都有上限；队列满时丢弃新数据报，不进行无界缓存。

SOCKS5 支持无认证的 `CONNECT` 命令以及 IPv4、IPv6 和域名目标。该监听本身没有用户密码认证，默认应绑定环回地址或由防火墙限制可信来源；直接绑定公网会形成开放代理。

## 配置

服务端：

- `quic.bind`：QUIC UDP 监听地址。
- `quic.certificate`、`quic.private_key`：PEM 证书链和私钥。相对路径以配置文件目录为基准。
- `groups[].name`：由 ASCII 字母、数字、`-`、`_` 组成，最长 64 字节。
- `groups[].key`：URL-safe Base64 且不带 padding，解码后必须恰好 32 字节。
- `tunnels[].kind`：`tcp`、`udp` 或 `socks5`。
- `tunnels[].bind`：公网监听地址。TCP 和 SOCKS5 不能使用相同地址；UDP 使用独立地址空间。
- `tunnels[].limit_bps`：可选，按该隧道汇总双向流量的每秒字节数。
- `tunnels[].max_connections`：TCP/SOCKS5 最大并发数，默认 1024。
- `tunnels[].max_udp_sessions`：UDP 最大来源会话数，默认 1024。
- `tunnels[].udp_idle_seconds`：UDP 会话空闲回收时间，默认 60 秒。

客户端：

- `server.address`：服务端 QUIC 地址。
- `server.name`：证书校验使用的 TLS 名称，不能填写未包含在证书 SAN 中的名称。
- `server.ca_certificate`：PEM CA 证书；自签名开发证书可直接作为信任锚。
- `group.name`、`group.key`：必须与服务端同一分组一致。
- `services[].name`、`services[].kind`：必须与服务端隧道一致。
- TCP/UDP 的 `services[].target`：客户端可访问的内网目标，支持 IP 或主机名。
- SOCKS5 不配置固定 `target`，目标来自 SOCKS5 `CONNECT` 请求。

单个服务端最多配置 256 个分组和 1024 条隧道，单客户端最多声明 256 个服务。未知字段会被拒绝，避免拼写错误被静默忽略。

## 热重载

模块监听配置文件所在目录，兼容控制面采用“临时文件 + rename”的原子替换方式。

- 服务端分组密钥变化只影响后续认证；现有 QUIC 会话保持运行。
- 隧道添加、删除、协议、地址、限速或容量变化会重建对应监听；其他监听不受影响。
- 客户端服务和目标变化通过现有控制流更新，不重连 QUIC，已有数据流继续使用建立时的目标。
- 客户端服务器地址、TLS 名称、CA、分组名或密钥变化会重建 QUIC 连接。
- 服务端 QUIC 地址、证书和私钥属于端点级配置，不支持热更新；变化会被拒绝并记录日志，需重启服务端。
- 无效的新配置不会替换当前配置。

## 安全与性能

- TLS 校验始终开启，不提供跳过证书验证的选项。
- 分组密钥使用系统随机源生成，认证比较为常量时间，内存副本在释放时清零。
- 0-RTT 当前未启用，避免认证和服务声明被重放；连接使用 keepalive 和有界空闲时间。
- 数据转发采用 16 KiB 固定缓冲区；限速器在短临界区内预留发送时间，睡眠期间不持有锁。
- 每个公网连接使用独立 QUIC 流，消除 TCP 流之间的队头阻塞。

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

端到端测试会生成临时 TLS 证书，并实际验证 TCP、UDP、SOCKS5 转发，以及客户端服务和服务端监听热删除/恢复时已有 TCP 连接不中断。
