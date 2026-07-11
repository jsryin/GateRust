# GateRust 开发准则

- 必须遵守本文件中的所有项目规则；除非用户明确要求，否则不得忽略或绕过。
- 本项目使用 Rust 1.97.0、Rust 2024 edition 和 Cargo workspace；服务端基于 Tokio、Axum、quinn、rustls-acme，前端使用 Svelte。
- 作为高级 Rust 开发者处理任务：先理解现有实现和调用链，再做最小化、完整且正确的改动。
- 优先使用标准库、Tokio 生态和已有依赖；新增 crate 前检查维护状态、许可证、MSRV 和资源开销，避免不必要的重型依赖。
- 使用现代、稳定且符合 Rust 惯例的实现；代码保持简洁、类型安全并与现有风格一致，避免过度抽象、无依据优化和 nightly 功能。
- 按职责拆分 crate、模块和文件，保持边界清晰，避免笼统的 `utils` 模块。
- 异步代码不得跨 `.await` 持有锁；后台任务必须可取消、可等待，各类队列和资源必须有明确上限。
- 错误使用结构化类型并在边界添加上下文；生产代码避免 `unwrap()`、`expect()`、`panic!()`，不得忽略 `Result`。
- 改功能要覆盖完整链路，包括配置、运行时、API、Web UI、权限、测试和部署影响。
- 安全相关功能使用成熟实现和安全默认值；不得自行实现密码学，也不得泄露敏感信息。
- 代码注释使用中文，只在逻辑不直观时添加；公共 API 和复杂不变量使用 rustdoc。
- 只在修改 Rust 代码后运行相关检查：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`；feature 改动还需检查对应的单模块组合。
- 修改前端或部署脚本时，运行仓库已定义的相关 lint、测试和构建命令；无法执行时必须在交付说明中明确原因。