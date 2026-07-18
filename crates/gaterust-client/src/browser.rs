use std::{io, process::Command};

#[cfg(windows)]
pub(crate) fn open(url: &str) -> io::Result<()> {
    let mut command = Command::new("cmd");
    command.args(["/C", "start", "", url]);
    command.spawn().map(drop)
}

#[cfg(target_os = "macos")]
pub(crate) fn open(url: &str) -> io::Result<()> {
    let mut command = Command::new("open");
    command.arg(url);
    command.spawn().map(drop)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn open(url: &str) -> io::Result<()> {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command.spawn().map(drop)
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn open(_url: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "当前平台不支持自动打开浏览器",
    ))
}
