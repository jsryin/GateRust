use std::{
    net::{SocketAddr, TcpListener as StdTcpListener},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::run_proxy_with_shutdown;

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn proxies_requests_and_hot_reloads_routes() {
    let first = start_upstream("first").await;
    let second = start_upstream("second").await;
    let http = free_address();
    let https = free_address();
    let directory = temporary_directory();
    let config_path = directory.join("proxy.toml");
    write_config(&config_path, http, https, first.address);

    let cancellation = CancellationToken::new();
    let task = tokio::spawn(run_proxy_with_shutdown(
        config_path.clone(),
        cancellation.clone(),
    ));
    wait_for_body(http, "first").await;

    let replacement = directory.join("proxy.toml.new");
    write_config(&replacement, http, https, second.address);
    std::fs::rename(replacement, &config_path).expect("原子替换测试配置");
    wait_for_body(http, "second").await;

    cancellation.cancel();
    task.await.expect("代理任务可等待").expect("代理正常退出");
    first.stop().await;
    second.stop().await;
    std::fs::remove_dir_all(directory).expect("删除测试目录");
}

struct TestUpstream {
    address: SocketAddr,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

impl TestUpstream {
    async fn stop(self) {
        self.cancellation.cancel();
        self.task.await.expect("上游任务可等待");
    }
}

async fn start_upstream(body: &'static str) -> TestUpstream {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定测试上游");
    let address = listener.local_addr().expect("读取上游地址");
    let cancellation = CancellationToken::new();
    let child = cancellation.clone();
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                () = child.cancelled() => break,
                accepted = listener.accept() => accepted,
            };
            let Ok((mut stream, _)) = accepted else {
                continue;
            };
            tokio::spawn(async move {
                let mut request = [0; 2048];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    TestUpstream {
        address,
        cancellation,
        task,
    }
}

async fn wait_for_body(address: SocketAddr, expected: &str) {
    for _ in 0..100 {
        if request(address).await.ends_with(expected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("代理响应未更新为 {expected}");
}

async fn request(address: SocketAddr) -> String {
    let Ok(mut stream) = TcpStream::connect(address).await else {
        return String::new();
    };
    if stream
        .write_all(b"GET /api?q=1 HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
        .await
        .is_err()
    {
        return String::new();
    }
    let mut response = Vec::new();
    if stream.read_to_end(&mut response).await.is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&response).into_owned()
}

fn write_config(path: &Path, http: SocketAddr, https: SocketAddr, upstream: SocketAddr) {
    let content = format!(
        r#"[proxy]
http_bind = "{http}"
https_bind = "{https}"
cache_dir = "cache"
max_connections = 16

[[routes]]
name = "app"
host = "example.com"
path_prefix = "/"
upstream = "http://{upstream}"
"#
    );
    std::fs::write(path, content).expect("写入测试配置");
}

fn free_address() -> SocketAddr {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("预留测试端口");
    listener.local_addr().expect("读取测试端口")
}

fn temporary_directory() -> PathBuf {
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("gaterust-proxy-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&path).expect("创建测试目录");
    path
}
