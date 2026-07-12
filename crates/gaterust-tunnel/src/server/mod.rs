mod socks5;
mod stream;
mod udp;

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use quinn::{Endpoint, VarInt};
use subtle::ConstantTimeEq as _;
use tokio::{
    sync::{OwnedSemaphorePermit, RwLock, Semaphore, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Result, TunnelError,
    config::{
        GroupSecret, ServerConfig, ServerQuicConfig, ServerTunnelConfig, TunnelKind,
        validate_group_key,
    },
    identity::validate_device_id,
    protocol::{
        AuthenticationStatus, ClientHello, ControlMessage, HANDSHAKE_TIMEOUT, PROTOCOL_VERSION,
        ServerHello, read_frame, validate_declarations, write_frame,
    },
    runtime::{RegisterError, TunnelRuntime},
    tls,
    watcher::ConfigWatcher,
};

const CLOSE_SHUTDOWN: VarInt = VarInt::from_u32(2);
const MAX_PENDING_AUTHENTICATIONS: usize = 32;

/// 运行隧道服务端，并按配置变化增删公网监听。
///
/// # Errors
///
/// 初始配置、TLS、监听地址或文件监听器初始化失败时返回错误。
pub async fn run_server(config_path: impl AsRef<Path>) -> Result<()> {
    let config_path = config_path.as_ref().to_owned();
    let cancellation = CancellationToken::new();
    let server = run_server_with_shutdown(config_path, cancellation.clone());
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            cancellation.cancel();
            server.await
        }
    }
}

/// 运行隧道服务端，直到取消令牌被触发。
///
/// # Errors
///
/// 初始配置、TLS、监听地址或文件监听器初始化失败时返回错误。
pub async fn run_server_with_shutdown(
    config_path: impl AsRef<Path>,
    cancellation: CancellationToken,
) -> Result<()> {
    run_server_with_runtime(config_path, TunnelRuntime::new(), cancellation).await
}

/// 使用可供控制面查询的运行时句柄启动隧道服务端。
///
/// # Errors
///
/// 初始配置、TLS、监听地址或文件监听器初始化失败时返回错误。
pub async fn run_server_with_runtime(
    config_path: impl AsRef<Path>,
    runtime: TunnelRuntime,
    cancellation: CancellationToken,
) -> Result<()> {
    let config_path = config_path.as_ref().to_owned();
    let initial = ServerConfig::load(&config_path)?;
    let mut watcher = ConfigWatcher::new(&config_path)?;
    let endpoint = tls::server_endpoint(&initial.quic)?;
    let local_address = endpoint.local_addr()?;
    let groups = Arc::new(RwLock::new(initial.credentials()));
    let mut listeners = ListenerManager::new(runtime.clone());
    listeners.apply(&initial.tunnels).await?;
    let accept_task = tokio::spawn(accept_connections(
        endpoint.clone(),
        runtime,
        Arc::clone(&groups),
        cancellation.child_token(),
    ));
    let immutable = initial.quic;
    tracing::info!(address = %local_address, "QUIC 隧道服务端已启动");

    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            changed = watcher.changed() => {
                if !changed {
                    break;
                }
                reload_server(&config_path, &immutable, &groups, &mut listeners).await;
            }
        }
    }

    cancellation.cancel();
    endpoint.close(CLOSE_SHUTDOWN, b"server shutdown");
    listeners.shutdown().await;
    await_task(accept_task, "QUIC 接入任务").await;
    endpoint.wait_idle().await;
    tracing::info!("QUIC 隧道服务端已停止");
    Ok(())
}

async fn reload_server(
    path: &Path,
    immutable: &ServerQuicConfig,
    groups: &RwLock<Vec<(String, GroupSecret)>>,
    listeners: &mut ListenerManager,
) {
    let config = match ServerConfig::load(path) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "新服务端配置无效，继续使用当前配置");
            return;
        }
    };
    if &config.quic != immutable {
        tracing::error!("quic.bind、证书或私钥不支持热更新，本次配置未应用");
        return;
    }
    let credentials = config.credentials();
    if let Err(error) = listeners.apply(&config.tunnels).await {
        tracing::error!(%error, "应用隧道监听配置失败");
        return;
    }
    *groups.write().await = credentials;
    tracing::info!(tunnels = config.tunnels.len(), "服务端配置已热更新");
}

async fn accept_connections(
    endpoint: Endpoint,
    runtime: TunnelRuntime,
    groups: Arc<RwLock<Vec<(String, GroupSecret)>>>,
    cancellation: CancellationToken,
) {
    let ids = Arc::new(AtomicU64::new(1));
    let authentication_permits = Arc::new(Semaphore::new(MAX_PENDING_AUTHENTICATIONS));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            incoming = endpoint.accept() => match incoming {
                Some(incoming) => {
                    let Ok(permit) = Arc::clone(&authentication_permits).try_acquire_owned() else {
                        drop(incoming);
                        tracing::debug!("待认证客户端数量已达上限，拒绝新连接");
                        continue;
                    };
                    let runtime = runtime.clone();
                    let groups = Arc::clone(&groups);
                    let id = ids.fetch_add(1, Ordering::Relaxed);
                    tasks.spawn(async move {
                        match incoming.await {
                            Ok(connection) => {
                                if let Err(error) = authenticate(connection, id, runtime, groups, permit).await {
                                    tracing::warn!(%error, "QUIC 客户端认证或控制通道结束");
                                }
                            }
                            Err(error) => tracing::debug!(%error, "QUIC 握手失败"),
                        }
                    });
                }
                None => break,
            },
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "QUIC 客户端任务异常结束");
                }
            }
        }
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::warn!(%error, "QUIC 客户端任务异常结束");
        }
    }
}

async fn authenticate(
    connection: quinn::Connection,
    id: u64,
    runtime: TunnelRuntime,
    groups: Arc<RwLock<Vec<(String, GroupSecret)>>>,
    authentication_permit: OwnedSemaphorePermit,
) -> Result<()> {
    let remote = connection.remote_address();
    let (mut send, mut receive) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi())
        .await
        .map_err(|_| TunnelError::Timeout("等待认证流"))??;
    let hello: ClientHello = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut receive))
        .await
        .map_err(|_| TunnelError::Timeout("读取认证信息"))??;
    let valid_key =
        std::str::from_utf8(&hello.key).is_ok_and(|key| validate_group_key(key).is_ok());
    let valid_hello = hello.version == PROTOCOL_VERSION
        && valid_key
        && validate_device_id(&hello.device_id).is_ok()
        && validate_declarations(&hello.services).is_ok();
    let group = if valid_hello {
        let groups = groups.read().await;
        groups.iter().find_map(|(name, secret)| {
            bool::from(secret.as_bytes().ct_eq(hello.key.as_slice())).then(|| name.clone())
        })
    } else {
        None
    };
    let Some(group) = group else {
        reject_authentication(
            &connection,
            &mut send,
            AuthenticationStatus::Rejected,
            "认证失败",
        )
        .await?;
        return Err(TunnelError::Protocol("密钥或设备信息无效".into()));
    };
    match runtime
        .register(
            id,
            hello.device_id.clone(),
            group.clone(),
            connection.clone(),
            hello.services,
        )
        .await
    {
        Ok(()) => {}
        Err(RegisterError::DeviceIdConflict) => {
            reject_authentication(
                &connection,
                &mut send,
                AuthenticationStatus::DeviceIdConflict,
                "设备 ID 已在线",
            )
            .await?;
            return Ok(());
        }
        Err(RegisterError::Capacity) => {
            reject_authentication(
                &connection,
                &mut send,
                AuthenticationStatus::ServerBusy,
                "在线客户端数量已达上限",
            )
            .await?;
            return Ok(());
        }
    }
    if let Err(error) = write_frame(
        &mut send,
        &ServerHello {
            status: AuthenticationStatus::Accepted,
            message: String::new(),
        },
    )
    .await
    {
        runtime.unregister(id).await;
        return Err(error);
    }
    drop(authentication_permit);

    let device_id = hello.device_id;
    tracing::info!(group, %device_id, %remote, "内网客户端已上线");
    let result = loop {
        match read_frame::<_, ControlMessage>(&mut receive).await {
            Ok(ControlMessage::UpdateServices(services)) => {
                if let Err(error) = validate_declarations(&services) {
                    break Err(error);
                }
                runtime.update_services(id, services).await;
                tracing::info!(group, %device_id, "客户端服务声明已更新");
            }
            Err(error) => break Err(error),
        }
    };
    runtime.unregister(id).await;
    tracing::info!(group, %device_id, %remote, "内网客户端已下线");
    result
}

async fn reject_authentication(
    connection: &quinn::Connection,
    send: &mut quinn::SendStream,
    status: AuthenticationStatus,
    message: &str,
) -> Result<()> {
    write_frame(
        send,
        &ServerHello {
            status,
            message: message.into(),
        },
    )
    .await?;
    send.finish()
        .map_err(|error| TunnelError::Protocol(format!("结束认证响应流失败: {error}")))?;
    let _ = tokio::time::timeout(HANDSHAKE_TIMEOUT, send.stopped()).await;
    connection.close(VarInt::from_u32(3), b"authentication failed");
    Ok(())
}

struct ListenerHandle {
    config: ServerTunnelConfig,
    cancellation: CancellationToken,
    stopped: oneshot::Receiver<()>,
    task: JoinHandle<()>,
}

struct ListenerManager {
    runtime: TunnelRuntime,
    active: HashMap<String, ListenerHandle>,
    retired: Vec<JoinHandle<()>>,
}

impl ListenerManager {
    fn new(runtime: TunnelRuntime) -> Self {
        Self {
            runtime,
            active: HashMap::new(),
            retired: Vec::new(),
        }
    }

    async fn apply(&mut self, configs: &[ServerTunnelConfig]) -> Result<()> {
        self.reap_retired().await;
        let previous: Vec<_> = self
            .active
            .values()
            .map(|handle| handle.config.clone())
            .collect();
        if let Err(error) = self.apply_once(configs).await {
            if let Err(rollback_error) = self.apply_once(&previous).await {
                return Err(TunnelError::InvalidConfig(format!(
                    "应用监听配置失败: {error}; 回滚也失败: {rollback_error}"
                )));
            }
            return Err(error);
        }
        self.runtime.apply_tunnels(configs).await;
        Ok(())
    }

    async fn apply_once(&mut self, configs: &[ServerTunnelConfig]) -> Result<()> {
        let desired: HashMap<_, _> = configs
            .iter()
            .map(|config| (config.name.clone(), config.clone()))
            .collect();
        let removed: Vec<_> = self
            .active
            .iter()
            .filter(|(name, handle)| desired.get(*name) != Some(&handle.config))
            .map(|(name, _)| name.clone())
            .collect();
        for name in removed {
            self.stop(&name).await;
        }
        for config in configs {
            if !self.active.contains_key(&config.name) {
                let handle = start_listener(config.clone(), self.runtime.clone()).await?;
                tracing::info!(tunnel = %config.name, kind = ?config.kind, address = %config.bind, "公网监听已启动");
                self.active.insert(config.name.clone(), handle);
            }
        }
        Ok(())
    }

    async fn reap_retired(&mut self) {
        let mut index = 0;
        while index < self.retired.len() {
            if self.retired[index].is_finished() {
                let task = self.retired.swap_remove(index);
                if let Err(error) = task.await {
                    tracing::warn!(%error, "已停止的监听任务异常结束");
                }
            } else {
                index += 1;
            }
        }
    }

    async fn stop(&mut self, name: &str) {
        let Some(handle) = self.active.remove(name) else {
            return;
        };
        handle.cancellation.cancel();
        if handle.stopped.await.is_err() {
            tracing::debug!(tunnel = name, "监听任务未发送停止确认");
        }
        tracing::info!(tunnel = name, "公网监听已停止");
        self.retired.push(handle.task);
    }

    async fn shutdown(&mut self) {
        let names: Vec<_> = self.active.keys().cloned().collect();
        for name in names {
            self.stop(&name).await;
        }
        let tasks = std::mem::take(&mut self.retired);
        for mut task in tasks {
            if tokio::time::timeout(Duration::from_secs(10), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }
}

impl Drop for ListenerManager {
    fn drop(&mut self) {
        for handle in self.active.values() {
            handle.cancellation.cancel();
        }
        for task in &self.retired {
            task.abort();
        }
    }
}

async fn start_listener(
    config: ServerTunnelConfig,
    runtime: TunnelRuntime,
) -> Result<ListenerHandle> {
    let cancellation = CancellationToken::new();
    let (stopped_sender, stopped) = oneshot::channel();
    let task = match config.kind {
        TunnelKind::Tcp | TunnelKind::Socks5 => {
            let (listener, permits) = stream::bind(&config).await?;
            let child = cancellation.clone();
            let task_config = config.clone();
            tokio::spawn(stream::run(
                listener,
                permits,
                task_config,
                runtime,
                child,
                stopped_sender,
            ))
        }
        TunnelKind::Udp => {
            let socket = udp::bind(&config).await?;
            let child = cancellation.clone();
            let task_config = config.clone();
            tokio::spawn(udp::run(
                socket,
                task_config,
                runtime,
                child,
                stopped_sender,
            ))
        }
    };
    Ok(ListenerHandle {
        config,
        cancellation,
        stopped,
        task,
    })
}

async fn await_task(mut task: JoinHandle<()>, name: &str) {
    match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, task = name, "后台任务异常结束"),
        Err(_) => {
            tracing::warn!(task = name, "等待后台任务退出超时");
            task.abort();
        }
    }
}
