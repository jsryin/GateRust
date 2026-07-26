#[cfg(unix)]
use std::fs::File;
use std::{
    fs::OpenOptions,
    io::{ErrorKind, Write as _},
    path::{Path, PathBuf},
};

use gaterust_tunnel::{ClientConfig, TunnelError, fetch_server_certificate};
use rand::RngExt as _;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{Result, run_login_step};

pub(crate) struct PreparedTrust {
    path: PathBuf,
    replaced: Option<PathBuf>,
    committed: bool,
}

impl PreparedTrust {
    pub(crate) fn commit(mut self) {
        self.committed = true;
        if let Some(path) = self.replaced.take()
            && let Err(error) = std::fs::remove_file(&path)
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), %error, "清理已替换的服务端证书失败");
        }
    }
}

impl Drop for PreparedTrust {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "清理未提交的服务端证书失败");
        }
    }
}

pub(crate) async fn prepare(
    config: &mut ClientConfig,
    config_path: &Path,
    replaced: Option<PathBuf>,
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<PreparedTrust> {
    let certificate = run_login_step(
        fetch_server_certificate(&config.server.address, &config.key),
        cancellation,
        deadline,
    )
    .await?;
    let server_name = certificate.server_name().to_owned();
    let pem = certificate.pem().as_bytes().to_vec();
    let parent = config_parent(config_path).to_owned();
    let (prepared, file_name) =
        tokio::task::spawn_blocking(move || write_candidate_certificate(&parent, &pem, replaced))
            .await??;
    config.server.name = Some(server_name);
    config.server.ca_certificate = Some(file_name);
    Ok(prepared)
}

pub(crate) fn managed_certificate_path(
    config: &ClientConfig,
    config_path: &Path,
) -> Option<PathBuf> {
    let configured = config.server.ca_certificate.as_deref()?;
    if configured
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return None;
    }
    let name = configured.file_name()?.to_str()?;
    let suffix = name.strip_prefix("server-")?.strip_suffix(".pem")?;
    (suffix.len() == 16 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| config_parent(config_path).join(configured))
}

fn write_candidate_certificate(
    parent: &Path,
    content: &[u8],
    replaced: Option<PathBuf>,
) -> gaterust_tunnel::Result<(PreparedTrust, PathBuf)> {
    std::fs::create_dir_all(parent).map_err(|source| TunnelError::WriteConfig {
        path: parent.to_owned(),
        source,
    })?;
    let file_name = PathBuf::from(format!("server-{:016x}.pem", rand::rng().random::<u64>()));
    let path = parent.join(&file_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o644);
    }
    let result = (|| {
        let mut file = options.open(&path)?;
        file.write_all(content)?;
        file.sync_all()?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(source) = result {
        let _ = std::fs::remove_file(&path);
        return Err(TunnelError::WriteConfig { path, source });
    }
    Ok((
        PreparedTrust {
            path,
            replaced,
            committed: false,
        },
        file_name,
    ))
}

fn config_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}
