use std::{
    env,
    error::Error,
    fs::{self, File},
    io::{self, Read as _},
    path::{Path, PathBuf},
};

use sha2::{Digest as _, Sha256};

const BUFFER_SIZE: usize = 16 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    emit_build_identities()?;
    tauri_build::build();
    Ok(())
}

fn emit_build_identities() -> io::Result<()> {
    let manifest = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "缺少 CARGO_MANIFEST_DIR"))?,
    );
    let workspace = fs::canonicalize(manifest.join("../.."))?;
    let ui_inputs = if tauri_build::is_dev() {
        vec![
            ("client-src", manifest.join("../src")),
            ("client-index", manifest.join("../index.html")),
            ("client-package", manifest.join("../package.json")),
            ("client-lock", manifest.join("../pnpm-lock.yaml")),
            ("client-tsconfig", manifest.join("../tsconfig.json")),
            ("client-vite", manifest.join("../vite.config.ts")),
        ]
    } else {
        vec![("client-dist", manifest.join("../dist"))]
    };
    let ui_build_id = hash_inputs(&workspace, &ui_inputs, &[])?;

    let client_inputs = vec![
        ("desktop-src", manifest.join("src")),
        ("desktop-build", manifest.join("build.rs")),
        ("desktop-cargo", manifest.join("Cargo.toml")),
        ("desktop-config", manifest.join("tauri.conf.json")),
        ("desktop-capabilities", manifest.join("capabilities")),
        ("client-package", manifest.join("../package.json")),
        ("client-crate", workspace.join("crates/gaterust-client")),
        ("tunnel-crate", workspace.join("crates/gaterust-tunnel")),
        ("workspace-cargo", workspace.join("Cargo.toml")),
        ("workspace-lock", workspace.join("Cargo.lock")),
    ];
    let environment = build_environment();
    let client_build_id = hash_inputs(
        &workspace,
        &client_inputs,
        &[
            ("ui-build-id".to_owned(), ui_build_id.clone()),
            ("build-environment".to_owned(), environment),
        ],
    )?;

    println!("cargo:rustc-env=GATERUST_UI_BUILD_ID={ui_build_id}");
    println!("cargo:rustc-env=GATERUST_CLIENT_BUILD_ID={client_build_id}");
    Ok(())
}

fn build_environment() -> String {
    let mut values = env::vars()
        .filter(|(name, _)| {
            matches!(name.as_str(), "HOST" | "PROFILE" | "TARGET")
                || name.starts_with("CARGO_FEATURE_")
        })
        .collect::<Vec<_>>();
    values.sort_unstable();
    values
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn hash_inputs(
    workspace: &Path,
    inputs: &[(&str, PathBuf)],
    values: &[(String, String)],
) -> io::Result<String> {
    let mut files = Vec::new();
    for (label, path) in inputs {
        println!("cargo:rerun-if-changed={}", path.display());
        collect_files(workspace, label, &fs::canonicalize(path)?, &mut files)?;
    }
    files.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    for (key, path) in files {
        hash_value(&mut hasher, key.as_bytes());
        hash_file(&mut hasher, &path)?;
    }
    for (name, value) in values {
        hash_value(&mut hasher, name.as_bytes());
        hash_value(&mut hasher, value.as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_files(
    workspace: &Path,
    label: &str,
    path: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("构建标识输入不得为符号链接: {}", path.display()),
        ));
    }
    if metadata.is_file() {
        files.push((input_key(workspace, label, path)?, path.to_owned()));
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("构建标识输入不是文件或目录: {}", path.display()),
        ));
    }

    for entry in fs::read_dir(path)? {
        collect_files(workspace, label, &entry?.path(), files)?;
    }
    Ok(())
}

fn input_key(workspace: &Path, label: &str, path: &Path) -> io::Result<String> {
    let relative = path.strip_prefix(workspace).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("构建标识输入不在工作区内: {error}"),
        )
    })?;
    let relative = relative.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("构建标识输入路径不是 UTF-8: {}", path.display()),
        )
    })?;
    Ok(format!("{label}:{}", relative.replace('\\', "/")))
}

fn hash_file(hasher: &mut Sha256, path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    hasher.update(file.metadata()?.len().to_le_bytes());
    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(())
}

fn hash_value(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(value.len().to_le_bytes());
    hasher.update(value);
}
