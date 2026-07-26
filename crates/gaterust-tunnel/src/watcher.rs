use std::{path::Path, time::Duration};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;

use crate::{Result, TunnelError};

const CHANGE_QUIET_PERIOD: Duration = Duration::from_millis(50);

pub(crate) struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    receiver: mpsc::Receiver<()>,
}

impl ConfigWatcher {
    pub(crate) fn new(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file_name = path
            .file_name()
            .ok_or_else(|| TunnelError::InvalidConfig("配置文件路径缺少文件名".into()))?
            .to_owned();
        let (sender, receiver) = mpsc::channel(1);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    let relevant_kind = matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    );
                    let matches_file = relevant_kind
                        && event
                            .paths
                            .iter()
                            .any(|changed| changed.file_name() == Some(file_name.as_ref()));
                    if matches_file {
                        tracing::debug!(paths = ?event.paths, "检测到配置文件变化");
                        match sender.try_send(()) {
                            Err(mpsc::error::TrySendError::Closed(())) => {
                                tracing::debug!("配置监听接收方已释放");
                            }
                            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {}
                        }
                    }
                }
            })?;
        watcher.watch(parent, RecursiveMode::NonRecursive)?;
        Ok(Self {
            _watcher: watcher,
            receiver,
        })
    }

    pub(crate) async fn changed(&mut self) -> bool {
        if self.receiver.recv().await.is_none() {
            return false;
        }
        // 原子替换在部分平台会产生删除、创建等多个事件，等待短暂静默后只重载一次。
        loop {
            match tokio::time::timeout(CHANGE_QUIET_PERIOD, self.receiver.recv()).await {
                Ok(Some(())) => {}
                Ok(None) | Err(_) => return true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn coalesces_atomic_replace_events() {
        let directory = tempfile::tempdir().expect("创建测试目录");
        let path = directory.path().join("client.toml");
        std::fs::write(&path, "initial").expect("写入初始文件");
        let mut watcher = ConfigWatcher::new(&path).expect("创建配置监听器");

        std::fs::remove_file(&path).expect("删除旧文件");
        std::fs::write(&path, "updated").expect("写入替换文件");

        assert!(
            tokio::time::timeout(Duration::from_secs(2), watcher.changed())
                .await
                .expect("应检测到配置替换")
        );
        assert!(
            tokio::time::timeout(CHANGE_QUIET_PERIOD * 2, watcher.changed())
                .await
                .is_err(),
            "同一次原子替换不应留下第二个变化事件"
        );
    }
}
