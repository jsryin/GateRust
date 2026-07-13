**本文档已过时**

### 项目定位
Rust 单二进制工具，实现**内网穿透（QUIC）** + **反向代理 + 自动 SSL**。  
**Web UI 是唯一控制中心**（登录后管理所有配置、状态、控制）。  
配置用 TOML 文件（Web UI 读写），无数据库。  
目标：**高性能 + 极低资源（128MB 服务器）+ 模块化 + 安全**。

### 技术栈（现代 + 轻量）
- **核心**：Tokio + Axum（控制平面） + quinn（QUIC） + rustls-acme（SSL）。
- **配置**：TOML（serde） + notify（文件监听）。
- **前端**：Svelte 静态 SPA（嵌入或 Cloudflare Pages）。
- **其他**：clap、tracing、rand（密钥生成）、tower。
- **为什么轻量**：单进程共享 runtime、无重型依赖、musl static binary、QUIC 低开销。

### 架构（单进程 + 松耦合）
```
unified-server（一个 binary）
├── Control Plane（Axum）
│   ├── Web UI（Svelte）
│   ├── 登录（Argon2 + JWT）
│   ├── TOML 读写 + 验证
│   └── 状态聚合 + SSE
├── Tunnel Module（QUIC）
│   └── 分组 + 密钥 + TCP/UDP/SOCKS5 + 限速
└── Proxy Module
    └── 反向代理 + rustls-acme + DNS-01
```

**关键逻辑**（必须清晰）：
- **配置流**：Web UI 修改 → 原子写入 TOML → notify 触发 → Control Plane 更新共享 `Arc<RwLock<Config>>` → channel 通知对应模块**热重载**（不中断现有连接）。
- **状态流**：各模块通过 channel 上报实时数据（在线客户端、流量、证书状态）→ Control Plane 聚合 → SSE 推送到 Web UI。
- **模块协作**：Tunnel 暴露本地端口 → Proxy 可直接路由到这些端口，实现一站式暴露。
- **模块化**：Cargo features（`tunnel`、`proxy`）+ 运行时 flag（`--enable-tunnel` 等）。sh 脚本可只启动其中一个模块。

### QUIC 内网穿透模块（参考 rathole 结论）
**使用 QUIC（quinn）**，不 fork rathole 整个项目（避免依赖膨胀和 QUIC 不兼容）。

**结论**：**参考 rathole 优秀设计模式**，自己实现 QUIC 版：
- 学习其**分组/服务声明、Token 认证、控制通道 + 数据转发**逻辑。
- 自己用 quinn 实现传输层（更现代、低开销）。
- 优点：保持二进制极小（类似 rathole ~500KiB 级别）、完全可控、与 Web UI 深度集成。

**功能**：
- **分组**：每个分组独立密钥（Web UI 随机生成）。
- **隧道类型**：TCP、UDP、SOCKS5。
- **限速**：每个隧道独立 `limit_bps`（token bucket，轻量实现）。
- **客户端**：服务器地址 + 分组密钥连接。
- **实现要点**：服务端主 QUIC 端点；客户端建连后用初始 stream 发送分组密钥认证；后续每个隧道用独立 stream 转发；支持热添加/删除隧道。

**性能与资源**：QUIC 内置多路复用 + 0-RTT，单 UDP 端口；Rust 异步零拷贝，转发效率高，内存占用极低。

### 反向代理 + 自动 SSL 模块（已补充新需求）
- **SSL**：rustls-acme 自动申请/续签。
  - Let's Encrypt（HTTP-01 / TLS-ALPN-01）。
  - Cloudflare DNS-01（Web UI 配置 Token）。
  - Google Cloud 免费证书支持。
- **反向代理**：hyper + tower 实现 host/path 路由（轻量）。**支持选择之前已自动申请的某个 SSL 证书**（可反代到公网 IP 或本地端口），Web UI 中可直接选择已管理的证书绑定到域名规则。
- **自动续期更新**：当选中的 SSL 证书自动续期成功后（rustls-acme 通知或文件变更检测），**自动热更新对应反向代理的 TLS 配置**（通过 channel 通知 Proxy Module 重新加载该域名的证书上下文，无需重启服务）。
- **关键**：Web UI 配置域名规则后自动处理证书 + 应用；支持自动续签定时任务 + 续期后自动刷新代理 SSL。

### Web UI（中心控制）
- Svelte 静态 SPA（支持 Cloudflare Pages 独立部署，调用后端 API）。
- 功能：登录 → 分组/隧道管理（含限速）→ 域名/SSL 管理（DNS Provider + 选择已有证书）→ 实时仪表盘（SSE）→ 一键生成客户端配置。
- **关键**：UI 只负责配置与展示，不直接处理网络逻辑。

### 客户端
独立 Rust binary（跨平台），支持分组密钥连接。

### 部署与安全（已补充 sh 新功能）
- **部署**：musl static binary + `deploy.sh`。
  - 支持一键**卸载某个模块**（干净卸载：停止对应 systemd 服务、移除相关 unit 文件、清理模块相关配置/日志，按需保留主 binary）。
  - 支持一键**开启/关闭开机自启**（systemd enable/disable）。
- **安全**：QUIC/TLS 全链路加密、分组密钥认证、Axum 输入验证 + rate limit、操作审计日志、内存安全（Rust）。
- **资源保障**：单进程 + 轻量 crate + 按需模块 + 最小日志，128MB 服务器轻松运行（预计全开 < 70MB RSS）。

### 开发路线图（逻辑递进）
1. 基础框架（Axum + TOML + 共享 Config + CLI flag）。
2. Control Plane + Web UI 骨架 + 配置热重载机制。
3. Tunnel Module（QUIC + 分组 + 密钥 + 隧道类型 + 限速，与 Control Plane 集成）。
4. Proxy Module（rustls-acme + DNS-01 + 选择已有证书 + 续期自动更新代理 SSL，与 Tunnel 联动）。
5. 客户端 + deploy.sh（含卸载 + 自启开关） + 测试 + 文档。

**总结关键优势**：
- **高性能 + 极低资源**：QUIC + Tokio + 单进程 + 轻量实现。
- **模块化**：features + flag + sh 脚本（支持单模块 + 干净卸载 + 自启控制）。
- **安全**：现代加密 + 认证 + 验证。
- **逻辑清晰**：TOML 单源 + channel 双向协作 + Web UI 中心控制 + 证书续期自动刷新代理。
- **无冗余**：不 fork rathole，仅参考设计；所有功能围绕 Web UI + TOML 展开；Proxy 支持选择已有 SSL 并自动跟进续期。