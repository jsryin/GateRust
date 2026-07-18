use std::{
    future::IntoFuture as _,
    io::ErrorKind,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    time::Duration,
};

use gaterust_tunnel::{ClientStatus, run_client_with_status};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::watch,
};
use tokio_util::sync::CancellationToken;

use crate::{
    api, browser,
    error::{ClientError, Result},
};

pub(crate) const UI_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 47_823));
pub(crate) const UI_AUTHORITY: &str = "127.0.0.1:47823";
pub(crate) const UI_URL: &str = "http://127.0.0.1:47823/";

pub(crate) async fn run(config_path: PathBuf, open_browser: bool) -> Result<()> {
    let listener = match TcpListener::bind(UI_ADDRESS).await {
        Ok(listener) => listener,
        Err(source) if source.kind() == ErrorKind::AddrInUse && existing_client().await => {
            tracing::info!(url = UI_URL, "客户端已在运行");
            if open_browser {
                open_browser_page();
            }
            return Ok(());
        }
        Err(source) => {
            return Err(ClientError::Bind {
                address: UI_ADDRESS,
                source,
            });
        }
    };

    let cancellation = CancellationToken::new();
    let (status_sender, status_receiver) = watch::channel(ClientStatus::Starting);
    let (revision_sender, revision_receiver) = watch::channel(0_u64);
    let tunnel_cancellation = cancellation.clone();
    let tunnel_path = config_path.clone();
    let tunnel_task = tokio::spawn(async move {
        supervise_tunnel(
            tunnel_path,
            tunnel_cancellation,
            status_sender,
            revision_receiver,
        )
        .await;
    });

    let router = api::router(
        config_path,
        status_receiver,
        revision_sender,
        cancellation.clone(),
    );
    tracing::info!(url = UI_URL, "客户端本机管理界面已启动");
    if open_browser {
        open_browser_page();
    }

    let server_cancellation = cancellation.clone();
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(server_cancellation.cancelled_owned())
        .into_future();
    tokio::pin!(server);
    let trigger: Result<Option<std::io::Result<()>>> = tokio::select! {
        result = &mut server => Ok(Some(result)),
        result = tokio::signal::ctrl_c() => {
            result.map(|()| None).map_err(ClientError::Signal)
        }
        () = cancellation.cancelled() => Ok(None),
    };
    cancellation.cancel();
    let (trigger_error, server_result) = match trigger {
        Ok(Some(result)) => (None, result),
        Ok(None) => (None, server.await),
        Err(error) => (Some(error), server.await),
    };
    let tunnel_result = tunnel_task.await;
    if let Some(error) = trigger_error {
        return Err(error);
    }
    tunnel_result?;
    server_result.map_err(ClientError::Serve)
}

async fn supervise_tunnel(
    config_path: PathBuf,
    cancellation: CancellationToken,
    status: watch::Sender<ClientStatus>,
    mut revision: watch::Receiver<u64>,
) {
    loop {
        revision.borrow_and_update();
        let client_cancellation = CancellationToken::new();
        let client =
            run_client_with_status(&config_path, client_cancellation.clone(), status.clone());
        tokio::pin!(client);
        let result = tokio::select! {
            result = &mut client => result,
            () = cancellation.cancelled() => {
                client_cancellation.cancel();
                client.await
            }
        };
        if cancellation.is_cancelled() {
            break;
        }
        if let Err(error) = result {
            if matches!(&error, gaterust_tunnel::TunnelError::InvalidConfig(_)) {
                status.send_replace(ClientStatus::Unconfigured {
                    reason: error.to_string(),
                });
            } else {
                tracing::error!(%error, "隧道客户端已停止，等待配置更新");
                status.send_replace(ClientStatus::Stopped {
                    reason: Some(error.to_string()),
                });
            }
        }

        // 任务异常退出后等待用户保存新配置，避免无意义的快速重启。
        match revision.has_changed() {
            Ok(true) => continue,
            Err(_) => break,
            Ok(false) => {}
        }
        tokio::select! {
            () = cancellation.cancelled() => break,
            changed = revision.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
    }
}

async fn existing_client() -> bool {
    let Ok(Ok(mut stream)) =
        tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(UI_ADDRESS)).await
    else {
        return false;
    };
    let request =
        format!("GET /api/health HTTP/1.1\r\nHost: {UI_AUTHORITY}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut response = Vec::with_capacity(1_024);
    let Ok(Ok(_)) = tokio::time::timeout(
        Duration::from_secs(1),
        stream.take(1_024).read_to_end(&mut response),
    )
    .await
    else {
        return false;
    };
    response
        .windows(b"gaterust-client".len())
        .any(|value| value == b"gaterust-client")
}

fn open_browser_page() {
    if let Err(error) = browser::open(UI_URL) {
        tracing::warn!(%error, url = UI_URL, "无法自动打开浏览器，请手动访问本机管理界面");
    }
}
