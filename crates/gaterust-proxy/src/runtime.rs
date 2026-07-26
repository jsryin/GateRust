use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tokio::{
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    AcmeAccountConfig, CertificateConfig, CertificateValidation, DnsAccountConfig, ProxyConfig,
    ProxyError, ProxyListenerConfig, Result,
    acme::{
        IssuanceRequest, IssueResult, IssuedCertificate, ManualDnsRecord, ManualOrder, begin_issue,
        continue_manual, verify_manual,
    },
    cache::CertificateCache,
    dns::DnsClient,
    tls::{CertificateResolver, DirectResolver},
};

const COMMAND_CAPACITY: usize = 64;
const RENEW_BEFORE: Duration = Duration::from_hours(336);
const SCHEDULER_INTERVAL: Duration = Duration::from_mins(1);
const MAX_CONCURRENT_ISSUANCE: usize = 4;
const MAX_CONCURRENT_DNS_TESTS: usize = 4;
const DNS_TEST_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_BASE: Duration = Duration::from_mins(5);
const RETRY_MAX: Duration = Duration::from_hours(6);

#[derive(Clone)]
pub struct ProxyRuntime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    commands: mpsc::Sender<Command>,
    receiver: Mutex<Option<mpsc::Receiver<Command>>>,
    snapshot: watch::Sender<ProxyRuntimeSnapshot>,
}

#[derive(Clone, Serialize)]
pub struct ProxyRuntimeSnapshot {
    pub certificates: Vec<CertificateRuntimeStatus>,
    pub config_status: ProxyConfigStatus,
}

#[derive(Clone, Serialize)]
pub struct ProxyConfigStatus {
    pub revision: u64,
    pub restart_required: bool,
    pub last_apply_error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct CertificateRuntimeStatus {
    pub certificate_id: String,
    pub status: CertificateStatus,
    pub expires_at: Option<u64>,
    pub last_error: Option<String>,
    pub manual_records: Vec<ManualDnsRecord>,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificateStatus {
    Idle,
    Issuing,
    WaitingDns,
    Valid,
    Renewing,
    Failed,
    Expired,
}

enum Command {
    Apply {
        config: ProxyConfig,
        restart_required: bool,
        response: oneshot::Sender<Result<ProxyConfigStatus>>,
    },
    Issue {
        certificate_id: String,
        response: oneshot::Sender<Result<()>>,
    },
    Continue {
        certificate_id: String,
        response: oneshot::Sender<Result<()>>,
    },
    TestDns {
        account: DnsAccountConfig,
        response: oneshot::Sender<Result<()>>,
    },
}

impl Default for ProxyRuntime {
    fn default() -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (snapshot, _) = watch::channel(ProxyRuntimeSnapshot {
            certificates: Vec::new(),
            config_status: ProxyConfigStatus {
                revision: 0,
                restart_required: false,
                last_apply_error: None,
            },
        });
        Self {
            inner: Arc::new(RuntimeInner {
                commands,
                receiver: Mutex::new(Some(receiver)),
                snapshot,
            }),
        }
    }
}

impl ProxyRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn snapshot(&self) -> ProxyRuntimeSnapshot {
        self.inner.snapshot.borrow().clone()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<ProxyRuntimeSnapshot> {
        self.inner.snapshot.subscribe()
    }

    /// 应用已通过校验的代理配置，并等待证书运行时完成缓存装载。
    ///
    /// # Errors
    ///
    /// 运行时未启动、命令队列关闭或缓存证书无效时返回错误。
    pub async fn apply_config(
        &self,
        config: ProxyConfig,
        restart_required: bool,
    ) -> Result<ProxyConfigStatus> {
        let (response, result) = oneshot::channel();
        self.send(Command::Apply {
            config,
            restart_required,
            response,
        })
        .await?;
        result.await.map_err(runtime_closed)?
    }

    /// 发起指定证书的首次申请或重新申请。
    ///
    /// # Errors
    ///
    /// 证书不存在、仍在申请或配置待迁移时返回错误。
    pub async fn issue_certificate(&self, certificate_id: String) -> Result<()> {
        let (response, result) = oneshot::channel();
        self.send(Command::Issue {
            certificate_id,
            response,
        })
        .await?;
        result.await.map_err(runtime_closed)?
    }

    /// 继续提交已完成手动 DNS 解析的订单。
    ///
    /// # Errors
    ///
    /// 不存在待处理订单或证书仍在操作时返回错误。
    pub async fn continue_certificate(&self, certificate_id: String) -> Result<()> {
        let (response, result) = oneshot::channel();
        self.send(Command::Continue {
            certificate_id,
            response,
        })
        .await?;
        result.await.map_err(runtime_closed)?
    }

    /// 使用当前凭据调用 DNS 服务商的轻量接口。
    ///
    /// # Errors
    ///
    /// 凭据无效或服务商 API 不可用时返回错误。
    pub async fn test_dns_account(&self, account: DnsAccountConfig) -> Result<()> {
        let (response, result) = oneshot::channel();
        self.send(Command::TestDns { account, response }).await?;
        result.await.map_err(runtime_closed)?
    }

    async fn send(&self, command: Command) -> Result<()> {
        self.inner
            .commands
            .send(command)
            .await
            .map_err(|_| ProxyError::Runtime("代理运行时未启动".into()))
    }

    pub(crate) async fn run_manager(
        &self,
        resolver: CertificateResolver,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let receiver = self
            .inner
            .receiver
            .lock()
            .map_err(|_| ProxyError::Runtime("代理命令接收锁已损坏".into()))?
            .take()
            .ok_or_else(|| ProxyError::Runtime("代理运行时只能启动一次".into()))?;
        CertificateManager::new(resolver, self.inner.snapshot.clone())
            .run(receiver, cancellation)
            .await;
        Ok(())
    }
}

fn runtime_closed(_: oneshot::error::RecvError) -> ProxyError {
    ProxyError::Runtime("代理运行时已停止".into())
}

struct CertificateManager {
    resolver: CertificateResolver,
    snapshot: watch::Sender<ProxyRuntimeSnapshot>,
    config: Option<ProxyConfig>,
    resolvers: HashMap<String, Arc<DirectResolver>>,
    statuses: HashMap<String, CertificateRuntimeStatus>,
    active: HashMap<String, ActiveTask>,
    pending: HashMap<String, ManualOrder>,
    tasks: JoinSet<TaskOutput>,
    dns_tasks: JoinSet<()>,
    retry: HashMap<String, RetryState>,
    account_gates: HashMap<String, Arc<Semaphore>>,
    dns_gates: HashMap<String, Arc<Semaphore>>,
    generation: u64,
    config_revision: u64,
    issuance_permits: Arc<Semaphore>,
    dns_test_permits: Arc<Semaphore>,
}

struct ActiveTask {
    generation: u64,
    cancellation: CancellationToken,
}

struct RetryState {
    attempts: u32,
    retry_at: u64,
}

struct TaskOutput {
    certificate_id: String,
    generation: u64,
    result: TaskResult,
}

enum TaskResult {
    Begin(Result<IssueResult>),
    Continued(Result<IssuedCertificate>),
    ManualNotReady(Box<ManualOrder>, ProxyError),
}

impl CertificateManager {
    fn new(resolver: CertificateResolver, snapshot: watch::Sender<ProxyRuntimeSnapshot>) -> Self {
        Self {
            resolver,
            snapshot,
            config: None,
            resolvers: HashMap::new(),
            statuses: HashMap::new(),
            active: HashMap::new(),
            pending: HashMap::new(),
            tasks: JoinSet::new(),
            dns_tasks: JoinSet::new(),
            retry: HashMap::new(),
            account_gates: HashMap::new(),
            dns_gates: HashMap::new(),
            generation: 0,
            config_revision: 0,
            issuance_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_ISSUANCE)),
            dns_test_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_DNS_TESTS)),
        }
    }

    async fn run(mut self, mut commands: mpsc::Receiver<Command>, cancellation: CancellationToken) {
        let mut scheduler = tokio::time::interval(SCHEDULER_INTERVAL);
        scheduler.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = cancellation.cancelled() => break,
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    self.handle_command(command).await;
                }
                result = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    if let Some(result) = result {
                        match result {
                            Ok(output) => self.handle_task(output),
                            Err(error) => tracing::warn!(%error, "证书签发任务异常结束"),
                        }
                    }
                }
                result = self.dns_tasks.join_next(), if !self.dns_tasks.is_empty() => {
                    if let Some(Err(error)) = result {
                        tracing::warn!(%error, "DNS 凭据测试任务异常结束");
                    }
                }
                _ = scheduler.tick() => self.schedule_due(),
            }
        }
        for task in self.active.values() {
            task.cancellation.cancel();
        }
        self.tasks.shutdown().await;
        self.dns_tasks.shutdown().await;
    }

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::Apply {
                config,
                restart_required,
                response,
            } => {
                let result = self.apply_config(config, restart_required).await;
                if response.send(result).is_err() {
                    tracing::debug!("配置应用请求方已断开");
                }
            }
            Command::Issue {
                certificate_id,
                response,
            } => {
                let result = self.start_issue(&certificate_id, false);
                if response.send(result).is_err() {
                    tracing::debug!("证书申请请求方已断开");
                }
            }
            Command::Continue {
                certificate_id,
                response,
            } => {
                let result = self.start_continue(&certificate_id);
                if response.send(result).is_err() {
                    tracing::debug!("手动验证请求方已断开");
                }
            }
            Command::TestDns { account, response } => {
                let Ok(permit) = Arc::clone(&self.dns_test_permits).try_acquire_owned() else {
                    if response
                        .send(Err(ProxyError::Runtime(
                            "DNS 凭据测试任务已达到并发上限".into(),
                        )))
                        .is_err()
                    {
                        tracing::debug!("DNS 凭据测试请求方已断开");
                    }
                    return;
                };
                self.dns_tasks.spawn(async move {
                    let _permit = permit;
                    let result = match DnsClient::from_config(&account, None) {
                        Ok(client) => {
                            tokio::time::timeout(DNS_TEST_TIMEOUT, client.validate_credentials())
                                .await
                                .unwrap_or_else(|_| Err(ProxyError::Dns("DNS 凭据测试超时".into())))
                        }
                        Err(error) => Err(error),
                    };
                    if response.send(result).is_err() {
                        tracing::debug!("DNS 凭据测试请求方已断开");
                    }
                });
            }
        }
    }

    async fn apply_config(
        &mut self,
        config: ProxyConfig,
        restart_required: bool,
    ) -> Result<ProxyConfigStatus> {
        let reload = certificates_requiring_reload(self.config.as_ref(), &config);
        let desired_ids = config
            .certificates
            .iter()
            .map(|certificate| certificate.id.as_str())
            .collect::<HashSet<_>>();
        let removed = self
            .resolvers
            .keys()
            .filter(|id| !desired_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        for certificate_id in reload.iter().chain(&removed) {
            if let Some(active) = self.active.remove(certificate_id) {
                active.cancellation.cancel();
            }
            self.pending.remove(certificate_id);
            self.retry.remove(certificate_id);
        }

        // 先完整装载新解析器，线上旧证书会持续服务到原子替换完成。
        let mut replacements = Vec::with_capacity(reload.len());
        for certificate in config
            .certificates
            .iter()
            .filter(|certificate| reload.contains(&certificate.id))
        {
            let (direct, status) =
                load_certificate_runtime(&config.proxy.cache_dir, certificate).await;
            replacements.push((certificate, direct, status));
        }

        for certificate_id in removed {
            self.resolver.remove(&certificate_id);
            self.resolvers.remove(&certificate_id);
            self.statuses.remove(&certificate_id);
        }
        for (certificate, direct, status) in replacements {
            self.resolver
                .replace_direct(&certificate.id, &certificate.domains, &direct);
            self.resolvers.insert(certificate.id.clone(), direct);
            self.statuses.insert(certificate.id.clone(), status);
        }

        if let Some(current) = &self.config {
            for certificate in &config.certificates {
                if current
                    .certificates
                    .iter()
                    .find(|item| item.id == certificate.id)
                    .is_some_and(|item| item.auto_renew != certificate.auto_renew)
                {
                    self.retry.remove(&certificate.id);
                }
            }
        }
        self.config = Some(config);
        self.config_revision = self.config_revision.wrapping_add(1);
        self.publish_config(restart_required, None);
        self.schedule_due();
        Ok(self.snapshot.borrow().config_status.clone())
    }

    fn start_issue(&mut self, certificate_id: &str, renewal: bool) -> Result<()> {
        if self.active.contains_key(certificate_id) {
            return Err(ProxyError::Runtime("该证书正在申请中".into()));
        }
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| ProxyError::Runtime("代理配置尚未应用".into()))?;
        let certificate = config
            .certificates
            .iter()
            .find(|certificate| certificate.id == certificate_id)
            .cloned()
            .ok_or_else(|| ProxyError::Runtime("证书不存在".into()))?;
        if certificate.validation.is_none() {
            return Err(ProxyError::Runtime(
                certificate
                    .migration_error
                    .clone()
                    .unwrap_or_else(|| "证书尚未选择验证方式".into()),
            ));
        }
        let account = config
            .acme_accounts
            .iter()
            .find(|account| account.id == certificate.acme_account_id)
            .cloned()
            .ok_or_else(|| ProxyError::Runtime("ACME 账户不存在".into()))?;
        let dns_account = match &certificate.validation {
            Some(CertificateValidation::DnsAccount { dns_account_id }) => Some(
                config
                    .dns_accounts
                    .iter()
                    .find(|account| account.id == *dns_account_id)
                    .cloned()
                    .ok_or_else(|| ProxyError::Runtime("DNS 账户不存在".into()))?,
            ),
            Some(CertificateValidation::Manual) | None => None,
        };
        let cache_root = config.proxy.cache_dir.clone();
        let account_gate = gate_for(&mut self.account_gates, &account.id);
        let dns_gate = dns_account
            .as_ref()
            .map(|account| gate_for(&mut self.dns_gates, &account.id));
        let request = IssuanceRequest {
            certificate: certificate.clone(),
            account,
            dns_account,
            account_gate,
            dns_gate,
        };
        let permit = Arc::clone(&self.issuance_permits)
            .try_acquire_owned()
            .map_err(|_| ProxyError::Runtime("证书签发任务已达到并发上限".into()))?;
        self.pending.remove(certificate_id);
        let cancellation = CancellationToken::new();
        let task_token = cancellation.clone();
        let generation = self.next_generation();
        let id = certificate.id.clone();
        self.tasks.spawn(async move {
            let _permit = permit;
            let result = begin_issue(request, &cache_root, &task_token).await;
            TaskOutput {
                certificate_id: id,
                generation,
                result: TaskResult::Begin(result),
            }
        });
        self.active.insert(
            certificate.id.clone(),
            ActiveTask {
                generation,
                cancellation,
            },
        );
        self.statuses
            .entry(certificate.id.clone())
            .and_modify(|status| {
                status.status = if renewal {
                    CertificateStatus::Renewing
                } else {
                    CertificateStatus::Issuing
                };
                status.last_error = None;
                status.manual_records.clear();
            });
        self.publish();
        Ok(())
    }

    fn start_continue(&mut self, certificate_id: &str) -> Result<()> {
        if self.active.contains_key(certificate_id) {
            return Err(ProxyError::Runtime("该证书正在申请中".into()));
        }
        let cache_root = self
            .config
            .as_ref()
            .map(|config| config.proxy.cache_dir.clone())
            .ok_or_else(|| ProxyError::Runtime("代理配置尚未应用".into()))?;
        let cancellation = CancellationToken::new();
        let permit = Arc::clone(&self.issuance_permits)
            .try_acquire_owned()
            .map_err(|_| ProxyError::Runtime("证书签发任务已达到并发上限".into()))?;
        let pending = self
            .pending
            .remove(certificate_id)
            .ok_or_else(|| ProxyError::Runtime("没有等待手动解析的 ACME 订单".into()))?;
        let task_token = cancellation.clone();
        let generation = self.next_generation();
        let id = certificate_id.to_owned();
        self.tasks.spawn(async move {
            let _permit = permit;
            let result = match verify_manual(&pending, &task_token).await {
                Ok(()) => {
                    TaskResult::Continued(continue_manual(pending, &cache_root, &task_token).await)
                }
                Err(error) => TaskResult::ManualNotReady(Box::new(pending), error),
            };
            TaskOutput {
                certificate_id: id,
                generation,
                result,
            }
        });
        self.active.insert(
            certificate_id.into(),
            ActiveTask {
                generation,
                cancellation,
            },
        );
        if let Some(status) = self.statuses.get_mut(certificate_id) {
            status.status = CertificateStatus::Issuing;
            status.last_error = None;
        }
        self.publish();
        Ok(())
    }

    fn handle_task(&mut self, output: TaskOutput) {
        let Some(active) = self.active.get(&output.certificate_id) else {
            return;
        };
        if active.generation != output.generation {
            return;
        }
        self.active.remove(&output.certificate_id);
        match output.result {
            TaskResult::Begin(Ok(IssueResult::Issued(certificate)))
            | TaskResult::Continued(Ok(certificate)) => {
                self.deploy(&output.certificate_id, &certificate);
            }
            TaskResult::Begin(Ok(IssueResult::Waiting(pending))) => {
                let records = pending.records.clone();
                self.pending.insert(output.certificate_id.clone(), *pending);
                if let Some(status) = self.statuses.get_mut(&output.certificate_id) {
                    status.status = CertificateStatus::WaitingDns;
                    status.last_error = None;
                    status.manual_records = records;
                }
            }
            TaskResult::ManualNotReady(pending, error) => {
                let records = pending.records.clone();
                self.pending.insert(output.certificate_id.clone(), *pending);
                if let Some(status) = self.statuses.get_mut(&output.certificate_id) {
                    status.status = CertificateStatus::WaitingDns;
                    status.last_error = Some(error.to_string());
                    status.manual_records = records;
                }
            }
            TaskResult::Begin(Err(error)) | TaskResult::Continued(Err(error)) => {
                self.fail(&output.certificate_id, &error);
            }
        }
        self.publish();
    }

    fn deploy(&mut self, certificate_id: &str, certificate: &IssuedCertificate) {
        let result = self
            .resolvers
            .get(certificate_id)
            .ok_or_else(|| ProxyError::Runtime("证书解析器不存在".into()))
            .and_then(|resolver| {
                resolver.set_pem(&certificate.certificate_pem, &certificate.private_key_pem)
            });
        match result {
            Ok(()) => {
                self.retry.remove(certificate_id);
                if let Some(status) = self.statuses.get_mut(certificate_id) {
                    status.status = CertificateStatus::Valid;
                    status.expires_at = Some(certificate.expires_at);
                    status.last_error = None;
                    status.manual_records.clear();
                }
                tracing::info!(
                    certificate_id,
                    expires_at = certificate.expires_at,
                    "证书已签发并热更新"
                );
            }
            Err(error) => self.fail(certificate_id, &error),
        }
    }

    fn fail(&mut self, certificate_id: &str, error: &ProxyError) {
        let now = unix_timestamp();
        let can_retry = self
            .config
            .as_ref()
            .and_then(|config| {
                config
                    .certificates
                    .iter()
                    .find(|certificate| certificate.id == certificate_id)
            })
            .is_some_and(|certificate| certificate.auto_renew)
            && self
                .statuses
                .get(certificate_id)
                .and_then(|status| status.expires_at)
                .is_some();
        if can_retry {
            let retry = self
                .retry
                .entry(certificate_id.into())
                .or_insert(RetryState {
                    attempts: 0,
                    retry_at: now,
                });
            let exponent = retry.attempts.min(6);
            let delay = RETRY_BASE.saturating_mul(1_u32 << exponent).min(RETRY_MAX);
            let jitter = certificate_id
                .bytes()
                .fold(0_u64, |value, byte| value.wrapping_add(u64::from(byte)))
                % 60;
            retry.retry_at = now.saturating_add(delay.as_secs()).saturating_add(jitter);
            retry.attempts = retry.attempts.saturating_add(1);
        }
        if let Some(status) = self.statuses.get_mut(certificate_id) {
            status.status = if status.expires_at.is_some_and(|expiry| expiry <= now) {
                CertificateStatus::Expired
            } else {
                CertificateStatus::Failed
            };
            status.last_error = Some(error.to_string());
            status.manual_records.clear();
        }
        tracing::error!(certificate_id, %error, "证书签发失败");
    }

    fn schedule_due(&mut self) {
        let now = unix_timestamp();
        let mut status_changed = false;
        for status in self.statuses.values_mut() {
            if status.expires_at.is_some_and(|expiry| expiry <= now)
                && matches!(
                    status.status,
                    CertificateStatus::Valid | CertificateStatus::Failed
                )
            {
                status.status = CertificateStatus::Expired;
                status_changed = true;
            }
        }
        let Some(config) = &self.config else {
            if status_changed {
                self.publish();
            }
            return;
        };
        let due = config
            .certificates
            .iter()
            .filter(|certificate| certificate.auto_renew)
            .filter_map(|certificate| {
                let status = self.statuses.get(&certificate.id)?;
                let expires_at = status.expires_at?;
                let renewal_at = expires_at.saturating_sub(RENEW_BEFORE.as_secs());
                let retry_ready = self
                    .retry
                    .get(&certificate.id)
                    .is_none_or(|retry| retry.retry_at <= now);
                (renewal_at <= now && retry_ready && !self.active.contains_key(&certificate.id))
                    .then_some(certificate.id.clone())
            })
            .collect::<Vec<_>>();
        for certificate_id in due {
            if let Err(error) = self.start_issue(&certificate_id, true) {
                self.fail(&certificate_id, &error);
            }
        }
        if status_changed {
            self.publish();
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn publish_config(&self, restart_required: bool, error: Option<String>) {
        let mut snapshot = self.build_snapshot();
        snapshot.config_status = ProxyConfigStatus {
            revision: self.config_revision,
            restart_required,
            last_apply_error: error,
        };
        self.snapshot.send_replace(snapshot);
    }

    fn publish(&self) {
        let config_status = self.snapshot.borrow().config_status.clone();
        let mut snapshot = self.build_snapshot();
        snapshot.config_status = config_status;
        self.snapshot.send_replace(snapshot);
    }

    fn build_snapshot(&self) -> ProxyRuntimeSnapshot {
        let mut certificates = self.statuses.values().cloned().collect::<Vec<_>>();
        certificates.sort_unstable_by(|left, right| left.certificate_id.cmp(&right.certificate_id));
        ProxyRuntimeSnapshot {
            certificates,
            config_status: ProxyConfigStatus {
                revision: self.config_revision,
                restart_required: false,
                last_apply_error: None,
            },
        }
    }
}

async fn load_certificate_runtime(
    cache_root: &Path,
    certificate: &CertificateConfig,
) -> (Arc<DirectResolver>, CertificateRuntimeStatus) {
    let direct = Arc::new(DirectResolver::default());
    let cache = CertificateCache::new(cache_root, &certificate.id);
    let cached = cache.load_certificate(certificate).await;
    let status = match cached {
        Ok(Some(cached)) => match direct.set_pem(&cached.certificate, &cached.private_key) {
            Ok(()) => CertificateRuntimeStatus {
                certificate_id: certificate.id.clone(),
                status: if cached.expires_at <= unix_timestamp() {
                    CertificateStatus::Expired
                } else {
                    CertificateStatus::Valid
                },
                expires_at: Some(cached.expires_at),
                last_error: certificate.migration_error.clone(),
                manual_records: Vec::new(),
            },
            Err(error) => CertificateRuntimeStatus {
                certificate_id: certificate.id.clone(),
                status: CertificateStatus::Failed,
                expires_at: Some(cached.expires_at),
                last_error: Some(error.to_string()),
                manual_records: Vec::new(),
            },
        },
        Ok(None) => CertificateRuntimeStatus {
            certificate_id: certificate.id.clone(),
            status: if certificate.migration_error.is_some() {
                CertificateStatus::Failed
            } else {
                CertificateStatus::Idle
            },
            expires_at: None,
            last_error: certificate.migration_error.clone(),
            manual_records: Vec::new(),
        },
        Err(error) => CertificateRuntimeStatus {
            certificate_id: certificate.id.clone(),
            status: CertificateStatus::Failed,
            expires_at: None,
            last_error: Some(error.to_string()),
            manual_records: Vec::new(),
        },
    };
    (direct, status)
}

fn certificates_requiring_reload(
    current: Option<&ProxyConfig>,
    next: &ProxyConfig,
) -> HashSet<String> {
    let Some(current) = current else {
        return next
            .certificates
            .iter()
            .map(|certificate| certificate.id.clone())
            .collect();
    };
    next.certificates
        .iter()
        .filter(|next_certificate| {
            current
                .certificates
                .iter()
                .find(|certificate| certificate.id == next_certificate.id)
                .is_none_or(|current_certificate| {
                    certificate_runtime_changed(
                        current,
                        next,
                        current_certificate,
                        next_certificate,
                    )
                })
        })
        .map(|certificate| certificate.id.clone())
        .collect()
}

fn certificate_runtime_changed(
    current: &ProxyConfig,
    next: &ProxyConfig,
    current_certificate: &CertificateConfig,
    next_certificate: &CertificateConfig,
) -> bool {
    current.proxy.cache_dir != next.proxy.cache_dir
        || current_certificate.domains != next_certificate.domains
        || current_certificate.acme_account_id != next_certificate.acme_account_id
        || current_certificate.validation != next_certificate.validation
        || current_certificate.migration_error != next_certificate.migration_error
        || !same_acme_runtime_account(
            current
                .acme_accounts
                .iter()
                .find(|account| account.id == current_certificate.acme_account_id),
            next.acme_accounts
                .iter()
                .find(|account| account.id == next_certificate.acme_account_id),
        )
        || !same_dns_runtime_account(
            dns_account_for_certificate(current, current_certificate),
            dns_account_for_certificate(next, next_certificate),
        )
}

fn dns_account_for_certificate<'a>(
    config: &'a ProxyConfig,
    certificate: &CertificateConfig,
) -> Option<&'a DnsAccountConfig> {
    let CertificateValidation::DnsAccount { dns_account_id } = certificate.validation.as_ref()?
    else {
        return None;
    };
    config
        .dns_accounts
        .iter()
        .find(|account| account.id == *dns_account_id)
}

fn same_acme_runtime_account(
    current: Option<&AcmeAccountConfig>,
    next: Option<&AcmeAccountConfig>,
) -> bool {
    match (current, next) {
        (Some(current), Some(next)) => {
            current.id == next.id
                && current.provider == next.provider
                && current.environment == next.environment
                && current.email == next.email
                && current.key_algorithm == next.key_algorithm
                && current.eab_key_id == next.eab_key_id
                && current.eab_hmac_key == next.eab_hmac_key
        }
        (None, None) => true,
        _ => false,
    }
}

fn same_dns_runtime_account(
    current: Option<&DnsAccountConfig>,
    next: Option<&DnsAccountConfig>,
) -> bool {
    match (current, next) {
        (Some(current), Some(next)) => {
            current.id == next.id
                && current.provider == next.provider
                && current.api_token == next.api_token
                && current.access_key == next.access_key
                && current.secret_key == next.secret_key
        }
        (None, None) => true,
        _ => false,
    }
}

fn gate_for(gates: &mut HashMap<String, Arc<Semaphore>>, account_id: &str) -> Arc<Semaphore> {
    Arc::clone(
        gates
            .entry(account_id.into())
            .or_insert_with(|| Arc::new(Semaphore::new(1))),
    )
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn listener_restart_required(
    running: &ProxyListenerConfig,
    requested: &ProxyListenerConfig,
) -> bool {
    running != requested
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_config() -> ProxyConfig {
        ProxyConfig {
            proxy: ProxyListenerConfig {
                http_bind: "127.0.0.1:80".parse().expect("HTTP 地址有效"),
                https_bind: "127.0.0.1:443".parse().expect("HTTPS 地址有效"),
                cache_dir: "cache".into(),
                max_connections: 16,
            },
            acme_accounts: vec![AcmeAccountConfig {
                id: "acme".into(),
                name: "ACME".into(),
                provider: crate::AcmeProvider::LetsEncrypt,
                environment: crate::AcmeEnvironment::Staging,
                email: "admin@example.com".into(),
                key_algorithm: crate::KeyAlgorithm::Ec256,
                eab_key_id: None,
                eab_hmac_key: None,
            }],
            dns_accounts: vec![DnsAccountConfig {
                id: "dns".into(),
                name: "Cloudflare".into(),
                provider: crate::DnsProvider::Cloudflare,
                api_token: Some(crate::SecretString::new("token".into())),
                access_key: None,
                secret_key: None,
            }],
            certificates: vec![CertificateConfig {
                id: "site".into(),
                name: "站点".into(),
                domains: vec!["example.com".into()],
                acme_account_id: "acme".into(),
                validation: Some(CertificateValidation::DnsAccount {
                    dns_account_id: "dns".into(),
                }),
                auto_renew: true,
                migration_error: None,
            }],
            routes: Vec::new(),
        }
    }

    #[test]
    fn scheduler_marks_elapsed_certificate_expired() {
        let (snapshot, _) = watch::channel(ProxyRuntimeSnapshot {
            certificates: Vec::new(),
            config_status: ProxyConfigStatus {
                revision: 0,
                restart_required: false,
                last_apply_error: None,
            },
        });
        let mut manager = CertificateManager::new(CertificateResolver::new(), snapshot);
        manager.statuses.insert(
            "site".into(),
            CertificateRuntimeStatus {
                certificate_id: "site".into(),
                status: CertificateStatus::Valid,
                expires_at: Some(1),
                last_error: None,
                manual_records: Vec::new(),
            },
        );

        manager.schedule_due();

        assert!(matches!(
            manager.statuses.get("site").map(|status| status.status),
            Some(CertificateStatus::Expired)
        ));
    }

    #[test]
    fn unrelated_changes_do_not_rebuild_certificate_runtime() {
        let current = proxy_config();
        let mut next = current.clone();
        next.routes.push(crate::RouteConfig {
            name: "web".into(),
            host: "example.com".into(),
            path_prefix: "/".into(),
            upstream: "http://127.0.0.1:3000".into(),
            certificate_id: None,
        });
        next.dns_accounts.push(DnsAccountConfig {
            id: "other".into(),
            name: "其他账户".into(),
            provider: crate::DnsProvider::GoDaddy,
            api_token: None,
            access_key: Some(crate::SecretString::new("key".into())),
            secret_key: Some(crate::SecretString::new("secret".into())),
        });
        next.acme_accounts[0].name = "重命名账户".into();
        next.certificates[0].name = "重命名证书".into();
        next.certificates[0].auto_renew = false;

        assert!(certificates_requiring_reload(Some(&current), &next).is_empty());
    }

    #[test]
    fn dependency_changes_reload_only_referencing_certificate() {
        let current = proxy_config();
        let mut next = current.clone();
        next.dns_accounts[0].api_token = Some(crate::SecretString::new("new-token".into()));

        assert_eq!(
            certificates_requiring_reload(Some(&current), &next),
            HashSet::from(["site".into()])
        );

        next.proxy.cache_dir = "other-cache".into();
        assert_eq!(
            certificates_requiring_reload(Some(&current), &next),
            HashSet::from(["site".into()])
        );
    }

    #[test]
    fn account_operations_share_gate_by_stable_id() {
        let mut gates = HashMap::new();
        let first = gate_for(&mut gates, "account");
        let second = gate_for(&mut gates, "account");
        let other = gate_for(&mut gates, "other");

        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &other));
    }
}
