use std::{
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use tauri::{App, AppHandle, Manager as _};

use crate::build_identity::CLIENT_BUILD_ID;

const INSTANCE_DIRECTORY: &str = "instance";
const INSTANCE_LOCK_FILE: &str = "client.lock";
const REQUEST_LOCK_FILE: &str = "request.lock";
const REQUEST_FILE: &str = "request";
const TAKEOVER_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) struct PrimaryInstance {
    lock: File,
    paths: InstancePaths,
}

struct InstanceLock {
    _file: File,
}

pub(crate) struct InstanceMonitor {
    signals: mpsc::Sender<MonitorSignal>,
    task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
struct InstancePaths {
    request: PathBuf,
}

enum AcquireOutcome {
    Primary(PrimaryInstance),
    Existing,
}

#[derive(Debug)]
enum RequestAction {
    Focus,
    Replace { next_build_id: String },
}

enum MonitorSignal {
    Changed(notify::Result<notify::Event>),
    Stop,
}

impl PrimaryInstance {
    /// 获取当前用户的客户端实例所有权；已有相同构建运行时直接结束本次启动。
    ///
    /// # Errors
    ///
    /// 无法访问实例目录，或等待旧构建退出超时时返回错误。
    pub(crate) fn acquire(application: &App) -> tauri::Result<Self> {
        let directory = application
            .path()
            .app_config_dir()?
            .join(INSTANCE_DIRECTORY);
        match acquire_in(&directory, CLIENT_BUILD_ID, TAKEOVER_TIMEOUT)? {
            AcquireOutcome::Primary(instance) => Ok(instance),
            AcquireOutcome::Existing => {
                application.cleanup_before_exit();
                std::process::exit(0);
            }
        }
    }

    /// 在应用初始化完成后启动实例请求监控。
    ///
    /// # Errors
    ///
    /// 无法创建监控线程时返回错误。
    pub(crate) fn start(self, application: &App) -> io::Result<()> {
        let paths = self.paths;
        let (signals, receiver) = mpsc::channel();
        let notify_signals = signals.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            if notify_signals.send(MonitorSignal::Changed(event)).is_err() {
                tracing::debug!("客户端实例监控已停止接收文件事件");
            }
        })
        .map_err(io::Error::other)?;
        watcher
            .watch(
                paths.request.parent().ok_or_else(|| {
                    io::Error::new(ErrorKind::InvalidInput, "实例请求路径缺少父目录")
                })?,
                RecursiveMode::NonRecursive,
            )
            .map_err(io::Error::other)?;
        let handle = application.handle().clone();
        let task = thread::Builder::new()
            .name("gaterust-instance".to_owned())
            .spawn(move || monitor_requests(&handle, &paths, watcher, &receiver))?;

        application.manage(InstanceLock { _file: self.lock });
        application.manage(InstanceMonitor {
            signals,
            task: Mutex::new(Some(task)),
        });
        Ok(())
    }
}

impl InstanceMonitor {
    pub(crate) fn shutdown(&self) {
        let task = match self.task.lock() {
            Ok(mut task) => task.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(task) = task else {
            return;
        };
        if self.signals.send(MonitorSignal::Stop).is_err() && !task.is_finished() {
            tracing::debug!("客户端实例监控线程已主动退出");
        }
        if task.join().is_err() {
            tracing::warn!("等待客户端实例监控线程退出失败");
        }
    }
}

impl Drop for InstanceMonitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn acquire_in(directory: &Path, build_id: &str, timeout: Duration) -> io::Result<AcquireOutcome> {
    if !is_build_id(build_id) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "客户端构建标识格式无效",
        ));
    }
    fs::create_dir_all(directory)?;
    let paths = InstancePaths {
        request: directory.join(REQUEST_FILE),
    };
    let instance_lock = open_lock_file(&directory.join(INSTANCE_LOCK_FILE))?;
    if try_lock(&instance_lock)? {
        remove_if_exists(&paths.request)?;
        return Ok(AcquireOutcome::Primary(PrimaryInstance {
            lock: instance_lock,
            paths,
        }));
    }

    let deadline = Instant::now() + timeout;
    // 多个候选新进程必须串行交接，避免相互覆盖实例请求。
    let request_lock = open_lock_file(&directory.join(REQUEST_LOCK_FILE))?;
    lock_until(&request_lock, deadline)?;
    if try_lock(&instance_lock)? {
        remove_if_exists(&paths.request)?;
        return Ok(AcquireOutcome::Primary(PrimaryInstance {
            lock: instance_lock,
            paths,
        }));
    }

    write_request(directory, &paths.request, build_id)?;
    // 同构建由旧实例删除请求确认聚焦；不同构建则释放实例锁供新进程继续。
    loop {
        if try_lock(&instance_lock)? {
            remove_if_exists(&paths.request)?;
            return Ok(AcquireOutcome::Primary(PrimaryInstance {
                lock: instance_lock,
                paths,
            }));
        }
        match paths.request.try_exists() {
            Ok(false) => return Ok(AcquireOutcome::Existing),
            Ok(true) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            remove_if_exists(&paths.request)?;
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "等待旧版客户端退出超时",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn lock_until(file: &File, deadline: Instant) -> io::Result<()> {
    loop {
        if try_lock(file)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "等待客户端实例请求锁超时",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn try_lock(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(fs::TryLockError::WouldBlock) => Ok(false),
        Err(fs::TryLockError::Error(error)) => Err(error),
    }
}

fn write_request(directory: &Path, request: &Path, build_id: &str) -> io::Result<()> {
    remove_if_exists(request)?;
    let temporary = directory.join(format!("request-{}.tmp", std::process::id()));
    fs::write(&temporary, build_id)?;
    if let Err(error) = fs::rename(&temporary, request) {
        if let Err(cleanup_error) = remove_if_exists(&temporary) {
            tracing::warn!(%cleanup_error, path = %temporary.display(), "清理实例请求临时文件失败");
        }
        return Err(error);
    }
    Ok(())
}

fn monitor_requests(
    application: &AppHandle,
    paths: &InstancePaths,
    _watcher: RecommendedWatcher,
    receiver: &mpsc::Receiver<MonitorSignal>,
) {
    if process_request(application, paths) {
        return;
    }
    while let Ok(signal) = receiver.recv() {
        match signal {
            MonitorSignal::Changed(Ok(_)) => {
                if process_request(application, paths) {
                    return;
                }
            }
            MonitorSignal::Changed(Err(error)) => {
                tracing::warn!(%error, "监控客户端实例请求失败");
            }
            MonitorSignal::Stop => return,
        }
    }
}

fn process_request(application: &AppHandle, paths: &InstancePaths) -> bool {
    match read_request(&paths.request, CLIENT_BUILD_ID) {
        Ok(Some(RequestAction::Focus)) => {
            focus_main_window(application);
            if let Err(error) = remove_if_exists(&paths.request) {
                tracing::warn!(%error, "确认客户端实例聚焦请求失败");
            }
        }
        Ok(Some(RequestAction::Replace { next_build_id })) => {
            tracing::info!(current_build_id = CLIENT_BUILD_ID, %next_build_id, "新构建请求接管客户端实例");
            application.exit(0);
            return true;
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, "读取客户端实例请求失败");
            if let Err(remove_error) = remove_if_exists(&paths.request) {
                tracing::warn!(%remove_error, "清理无效客户端实例请求失败");
            }
        }
    }
    false
}

fn read_request(path: &Path, current_build_id: &str) -> io::Result<Option<RequestAction>> {
    let request = match fs::read_to_string(path) {
        Ok(request) => request,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let build_id = request.trim();
    if !is_build_id(build_id) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "客户端实例请求中的构建标识无效",
        ));
    }
    if build_id == current_build_id {
        Ok(Some(RequestAction::Focus))
    } else {
        Ok(Some(RequestAction::Replace {
            next_build_id: build_id.to_owned(),
        }))
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_build_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn focus_main_window(application: &AppHandle) {
    let Some(window) = application.get_webview_window("main") else {
        return;
    };
    match window.is_minimized() {
        Ok(true) => {
            if let Err(error) = window.unminimize() {
                tracing::warn!(%error, "恢复客户端窗口失败");
            }
        }
        Ok(false) => {}
        Err(error) => tracing::warn!(%error, "读取客户端窗口状态失败"),
    }
    if let Err(error) = window.show() {
        tracing::warn!(%error, "显示客户端窗口失败");
    }
    if let Err(error) = window.set_focus() {
        tracing::warn!(%error, "聚焦客户端窗口失败");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use tempfile::tempdir;

    use super::*;

    const FIRST_BUILD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SECOND_BUILD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn classifies_same_and_changed_build_requests() {
        let directory = tempdir().expect("创建临时目录");
        let request = directory.path().join(REQUEST_FILE);
        fs::write(&request, FIRST_BUILD).expect("写入相同构建请求");
        assert!(matches!(
            read_request(&request, FIRST_BUILD).expect("读取相同构建请求"),
            Some(RequestAction::Focus)
        ));

        fs::write(&request, SECOND_BUILD).expect("写入新构建请求");
        assert!(matches!(
            read_request(&request, FIRST_BUILD).expect("读取新构建请求"),
            Some(RequestAction::Replace { next_build_id }) if next_build_id == SECOND_BUILD
        ));
    }

    #[test]
    fn rejects_invalid_build_request() {
        let directory = tempdir().expect("创建临时目录");
        let request = directory.path().join(REQUEST_FILE);
        fs::write(&request, "invalid").expect("写入无效请求");
        assert_eq!(
            read_request(&request, FIRST_BUILD)
                .expect_err("无效构建标识必须失败")
                .kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    fn changed_build_waits_for_instance_lock() {
        let directory = tempdir().expect("创建临时目录");
        let first = acquire_primary(directory.path());
        let path = directory.path().to_owned();
        let (sender, receiver) = mpsc::channel();
        let task = thread::spawn(move || {
            let result = acquire_in(&path, SECOND_BUILD, Duration::from_secs(2));
            sender.send(result).expect("发送接管结果");
        });

        wait_for_request(directory.path());
        drop(first);
        let outcome = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("新构建应在锁释放后继续")
            .expect("新构建接管实例");
        assert!(matches!(outcome, AcquireOutcome::Primary(_)));
        task.join().expect("等待接管线程");
    }

    #[test]
    fn same_build_exits_after_focus_acknowledgement() {
        let directory = tempdir().expect("创建临时目录");
        let _first = acquire_primary(directory.path());
        let path = directory.path().to_owned();
        let (sender, receiver) = mpsc::channel();
        let task = thread::spawn(move || {
            let result = acquire_in(&path, FIRST_BUILD, Duration::from_secs(2));
            sender.send(result).expect("发送同构建启动结果");
        });

        wait_for_request(directory.path());
        remove_if_exists(&directory.path().join(REQUEST_FILE)).expect("确认聚焦请求");
        let outcome = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("同构建应在确认后退出")
            .expect("读取同构建启动结果");
        assert!(matches!(outcome, AcquireOutcome::Existing));
        task.join().expect("等待同构建启动线程");
    }

    #[test]
    fn takeover_times_out_when_old_instance_does_not_respond() {
        let directory = tempdir().expect("创建临时目录");
        let _first = acquire_primary(directory.path());

        let Err(error) = acquire_in(directory.path(), SECOND_BUILD, Duration::from_millis(100))
        else {
            panic!("旧实例无响应时接管不应成功");
        };

        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(!directory.path().join(REQUEST_FILE).exists());
    }

    fn acquire_primary(directory: &Path) -> PrimaryInstance {
        match acquire_in(directory, FIRST_BUILD, Duration::from_secs(1)).expect("首个构建获取实例")
        {
            AcquireOutcome::Primary(instance) => instance,
            AcquireOutcome::Existing => panic!("首个构建不应存在旧实例"),
        }
    }

    fn wait_for_request(directory: &Path) {
        let request = directory.join(REQUEST_FILE);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !request.exists() {
            assert!(Instant::now() < deadline, "实例请求写入超时");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
