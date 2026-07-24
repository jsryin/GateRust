use std::{
    fs::OpenOptions,
    io::{ErrorKind, Write as _},
    path::{Path, PathBuf},
};

use gaterust_tunnel::{ClientConfig, TunnelError, fetch_server_certificate, server_name_from_pem};
use rand::RngExt as _;

use crate::Result;

const SERVER_CERTIFICATE_NAME: &str = "server.pem";

pub(crate) async fn prepare(
    config: &mut ClientConfig,
    config_path: &Path,
    force_download: bool,
) -> Result<()> {
    if !force_download && let Some(configured) = config.server.ca_certificate.as_deref() {
        let configured = if configured.is_relative() {
            config_parent(config_path).join(configured)
        } else {
            configured.to_owned()
        };
        if configured
            .try_exists()
            .map_err(|source| TunnelError::ReadConfig {
                path: configured.clone(),
                source,
            })?
        {
            return Ok(());
        }
    }
    let certificate_path = config_parent(config_path).join(SERVER_CERTIFICATE_NAME);
    if !force_download
        && certificate_path
            .try_exists()
            .map_err(|source| TunnelError::ReadConfig {
                path: certificate_path.clone(),
                source,
            })?
    {
        let read_path = certificate_path.clone();
        let content = tokio::task::spawn_blocking(move || {
            std::fs::read(&read_path).map_err(|source| TunnelError::ReadConfig {
                path: read_path,
                source,
            })
        })
        .await??;
        config.server.name = Some(server_name_from_pem(&content, &config.server.address)?);
        config.server.ca_certificate = Some(PathBuf::from(SERVER_CERTIFICATE_NAME));
        return Ok(());
    }

    let certificate = fetch_server_certificate(&config.server.address, &config.key).await?;
    let server_name = certificate.server_name().to_owned();
    let pem = certificate.pem().as_bytes().to_vec();
    tokio::task::spawn_blocking(move || write_certificate(&certificate_path, &pem)).await??;
    config.server.name = Some(server_name);
    config.server.ca_certificate = Some(PathBuf::from(SERVER_CERTIFICATE_NAME));
    Ok(())
}

fn write_certificate(path: &Path, content: &[u8]) -> gaterust_tunnel::Result<()> {
    let parent = config_parent(path);
    std::fs::create_dir_all(parent).map_err(|source| TunnelError::WriteConfig {
        path: path.to_owned(),
        source,
    })?;
    let temporary = parent.join(format!(
        ".gaterust-server-{:016x}.tmp",
        rand::rng().random::<u64>()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o644);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(content)?;
        file.sync_all()?;
        replace_file(&temporary, path)
    })();
    if let Err(source) = result {
        cleanup(&temporary);
        return Err(TunnelError::WriteConfig {
            path: path.to_owned(),
            source,
        });
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, path)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(temporary, path)
}

fn config_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn cleanup(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "清理未完成的服务端证书失败");
    }
}
