use std::{fs, io::ErrorKind};

#[cfg(any(target_os = "linux", windows))]
use std::path::PathBuf;

use tauri::{App, Manager as _};

const VERSION_FILE: &str = "webview-cache-version";
#[cfg(any(target_os = "linux", windows))]
const DATA_DIRECTORY: &str = "webview";

/// 在应用版本变化时清理旧浏览数据；失败时保留版本标记，以便下次启动重试。
pub(crate) fn migrate(application: &App) {
    if let Err(error) = migrate_inner(application) {
        tracing::warn!(%error, "清理旧版 WebView 数据失败，将在下次启动时重试");
    }
}

#[cfg(any(target_os = "linux", windows))]
pub(crate) fn directory(application: &App) -> tauri::Result<PathBuf> {
    application
        .path()
        .app_local_data_dir()
        .map(|path| path.join(DATA_DIRECTORY))
}

fn migrate_inner(application: &App) -> tauri::Result<()> {
    let marker = application.path().app_config_dir()?.join(VERSION_FILE);
    let migrated_before = marker.try_exists()?;
    let stored_version = read_version(&marker)?;
    let current_version = application.package_info().version.to_string();
    if stored_version.as_deref() == Some(current_version.as_str()) {
        return Ok(());
    }

    clear_persistent_data(application, migrated_before)?;
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(marker, current_version)?;
    Ok(())
}

fn read_version(path: &std::path::Path) -> std::io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(version) => Ok((!version.trim().is_empty()).then(|| version.trim().to_owned())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "linux", windows))]
fn clear_persistent_data(application: &App, migrated_before: bool) -> tauri::Result<()> {
    let local_data = application.path().app_local_data_dir()?;
    let target = local_data_target(local_data, migrated_before);
    match fs::remove_dir_all(target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(any(target_os = "linux", windows))]
fn local_data_target(local_data: PathBuf, migrated_before: bool) -> PathBuf {
    if migrated_before {
        local_data.join(DATA_DIRECTORY)
    } else {
        // 旧版让 Tauri 将整个应用本地数据目录作为 WebView 数据目录。
        local_data
    }
}

#[cfg(target_os = "macos")]
fn clear_persistent_data(application: &App, _migrated_before: bool) -> tauri::Result<()> {
    let cleanup = tauri::WebviewWindowBuilder::new(
        application,
        "browser-data-cleanup",
        tauri::WebviewUrl::External(tauri::Url::parse("about:blank")?),
    )
    .visible(false)
    .incognito(false)
    .build()?;
    cleanup.clear_all_browsing_data()?;
    cleanup.destroy()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn missing_or_empty_version_is_not_current() {
        let directory = tempdir().expect("创建临时目录");
        let marker = directory.path().join(VERSION_FILE);
        assert_eq!(read_version(&marker).expect("读取不存在的标记"), None);

        fs::write(&marker, "  \n").expect("写入空标记");
        assert_eq!(read_version(&marker).expect("读取空标记"), None);
    }

    #[test]
    fn reads_trimmed_version() {
        let directory = tempdir().expect("创建临时目录");
        let marker = directory.path().join(VERSION_FILE);
        fs::write(&marker, " 1.2.3\n").expect("写入版本标记");
        assert_eq!(
            read_version(&marker).expect("读取版本标记").as_deref(),
            Some("1.2.3")
        );
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn first_migration_clears_legacy_root_and_updates_clear_only_webview() {
        let root = PathBuf::from("application-local-data");
        assert_eq!(local_data_target(root.clone(), false), root);
        assert_eq!(
            local_data_target(root.clone(), true),
            root.join(DATA_DIRECTORY)
        );
    }
}
