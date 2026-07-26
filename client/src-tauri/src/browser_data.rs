use std::{fs, io::ErrorKind, path::PathBuf};

use tauri::{App, Manager as _};

use crate::build_identity::UI_BUILD_ID;

#[cfg(any(target_os = "linux", windows))]
const DATA_DIRECTORY: &str = "webview-builds";
#[cfg(target_os = "macos")]
const BUILD_ID_FILE: &str = "webview-build-id";

/// 为当前前端构建准备独立的浏览数据目录。
///
/// # Errors
///
/// 无法确定应用数据目录或创建当前构建目录时返回错误。
pub(crate) fn prepare(application: &App) -> tauri::Result<Option<PathBuf>> {
    #[cfg(any(target_os = "linux", windows))]
    {
        let root = application
            .path()
            .app_local_data_dir()?
            .join(DATA_DIRECTORY);
        let current = root.join(UI_BUILD_ID);
        fs::create_dir_all(&current)?;
        cleanup_stale_builds(&root, UI_BUILD_ID);
        Ok(Some(current))
    }

    #[cfg(target_os = "macos")]
    {
        migrate_macos(application)?;
        Ok(None)
    }
}

#[cfg(any(target_os = "linux", windows))]
fn cleanup_stale_builds(root: &std::path::Path, current_build_id: &str) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(%error, path = %root.display(), "读取旧版 WebView 数据目录失败");
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(%error, "读取旧版 WebView 数据项失败");
                continue;
            }
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == current_build_id || !is_build_id(name) {
            continue;
        }

        let path = entry.path();
        let result = entry.file_type().and_then(|kind| {
            if kind.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            }
        });
        if let Err(error) = result
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(%error, path = %path.display(), "清理旧版 WebView 数据失败");
        }
    }
}

fn is_build_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(target_os = "macos")]
fn migrate_macos(application: &App) -> tauri::Result<()> {
    let marker = application.path().app_config_dir()?.join(BUILD_ID_FILE);
    match fs::read_to_string(&marker) {
        Ok(build_id) if build_id.trim() == UI_BUILD_ID => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    // macOS 不支持指定 WebView 数据目录，资源变化时清空非持久浏览数据。
    let cleanup = tauri::WebviewWindowBuilder::new(
        application,
        "browser-data-cleanup",
        tauri::WebviewUrl::External(
            tauri::Url::parse("about:blank").map_err(tauri::Error::InvalidUrl)?,
        ),
    )
    .visible(false)
    .incognito(false)
    .build()?;
    cleanup.clear_all_browsing_data()?;
    cleanup.destroy()?;
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(marker, UI_BUILD_ID)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn validates_build_ids_strictly() {
        assert!(is_build_id(&"a".repeat(64)));
        assert!(!is_build_id(&"a".repeat(63)));
        assert!(!is_build_id(&format!("{}g", "a".repeat(63))));
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn cleanup_removes_only_stale_build_directories() {
        let directory = tempdir().expect("创建临时目录");
        let current = "a".repeat(64);
        let stale = "b".repeat(64);
        let unrelated = directory.path().join("unrelated");
        fs::create_dir(directory.path().join(&current)).expect("创建当前构建目录");
        fs::create_dir(directory.path().join(&stale)).expect("创建旧构建目录");
        fs::create_dir(&unrelated).expect("创建无关目录");

        cleanup_stale_builds(directory.path(), &current);

        assert!(directory.path().join(current).is_dir());
        assert!(!directory.path().join(stale).exists());
        assert!(unrelated.is_dir());
    }
}
