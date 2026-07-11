use std::path::Path;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;

use crate::{Result, TunnelError};

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
        self.receiver.recv().await.is_some()
    }
}
