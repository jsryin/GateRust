use std::path::Path;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;

use crate::{ProxyError, Result};

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
            .ok_or_else(|| ProxyError::InvalidConfig("配置文件路径缺少文件名".into()))?
            .to_owned();
        let (sender, receiver) = mpsc::channel(1);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    let relevant = matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) && event
                        .paths
                        .iter()
                        .any(|path| path.file_name() == Some(file_name.as_ref()));
                    if relevant {
                        match sender.try_send(()) {
                            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {}
                            Err(mpsc::error::TrySendError::Closed(())) => {
                                tracing::debug!("代理配置监听接收方已释放");
                            }
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
