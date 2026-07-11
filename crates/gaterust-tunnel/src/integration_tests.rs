use std::{net::SocketAddr, path::Path, time::Duration};

use rcgen::generate_simple_self_signed;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream, UdpSocket},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{run_client_with_shutdown, run_server_with_shutdown};

const TEST_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwards_tcp_udp_and_socks5() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_certificate(directory.path());

    let tcp_target = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("应能绑定 TCP 回显服务");
    let tcp_target_address = tcp_target.local_addr().expect("应能读取 TCP 回显地址");
    let udp_target = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("应能绑定 UDP 回显服务");
    let udp_target_address = udp_target.local_addr().expect("应能读取 UDP 回显地址");
    let echo_cancel = CancellationToken::new();
    let tcp_echo = tokio::spawn(run_tcp_echo(tcp_target, echo_cancel.clone()));
    let udp_echo = tokio::spawn(run_udp_echo(udp_target, echo_cancel.clone()));

    let quic = unused_udp_address();
    let tcp_public = unused_tcp_address();
    let udp_public = unused_udp_address();
    let socks_public = unused_tcp_address();
    write_configs(
        directory.path(),
        quic,
        tcp_public,
        udp_public,
        socks_public,
        tcp_target_address,
        udp_target_address,
    );

    let cancellation = CancellationToken::new();
    let server_path = directory.path().join("server.toml");
    let client_path = directory.path().join("client.toml");
    let server_cancel = cancellation.clone();
    let server =
        tokio::spawn(async move { run_server_with_shutdown(server_path, server_cancel).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let client_cancel = cancellation.clone();
    let client =
        tokio::spawn(async move { run_client_with_shutdown(client_path, client_cancel).await });

    assert_stream_echo(tcp_public, b"tcp-through-quic").await;
    let mut persistent = TcpStream::connect(tcp_public)
        .await
        .expect("应能建立持久 TCP 隧道");
    exchange(&mut persistent, b"before-reload").await;

    write_client_config(
        directory.path(),
        quic,
        tcp_target_address,
        udp_target_address,
        false,
    );
    wait_until_stream_unavailable(tcp_public).await;
    exchange(&mut persistent, b"after-client-remove").await;
    write_client_config(
        directory.path(),
        quic,
        tcp_target_address,
        udp_target_address,
        true,
    );
    assert_stream_echo(tcp_public, b"after-client-add").await;

    write_server_config(
        directory.path(),
        quic,
        tcp_public,
        udp_public,
        socks_public,
        false,
    );
    wait_until_stream_unavailable(tcp_public).await;
    exchange(&mut persistent, b"after-server-remove").await;
    write_server_config(
        directory.path(),
        quic,
        tcp_public,
        udp_public,
        socks_public,
        true,
    );
    assert_stream_echo(tcp_public, b"after-server-add").await;
    drop(persistent);

    assert_udp_echo(udp_public, b"udp-through-quic").await;
    assert_socks_echo(socks_public, tcp_target_address, b"socks-through-quic").await;

    cancellation.cancel();
    echo_cancel.cancel();
    assert_task_ok(server, "服务端").await;
    assert_task_ok(client, "客户端").await;
    assert_join_ok(tcp_echo, "TCP 回显服务").await;
    assert_join_ok(udp_echo, "UDP 回显服务").await;
}

fn write_certificate(directory: &Path) {
    let certified =
        generate_simple_self_signed(vec!["localhost".into()]).expect("应能生成测试证书");
    std::fs::write(directory.join("server.pem"), certified.cert.pem()).expect("应能写入测试证书");
    std::fs::write(
        directory.join("server-key.pem"),
        certified.signing_key.serialize_pem(),
    )
    .expect("应能写入测试私钥");
}

#[allow(clippy::too_many_arguments)]
fn write_configs(
    directory: &Path,
    quic: SocketAddr,
    tcp_public: SocketAddr,
    udp_public: SocketAddr,
    socks_public: SocketAddr,
    tcp_target: SocketAddr,
    udp_target: SocketAddr,
) {
    write_server_config(directory, quic, tcp_public, udp_public, socks_public, true);
    write_client_config(directory, quic, tcp_target, udp_target, true);
}

fn write_server_config(
    directory: &Path,
    quic: SocketAddr,
    tcp_public: SocketAddr,
    udp_public: SocketAddr,
    socks_public: SocketAddr,
    include_tcp: bool,
) {
    let tcp_tunnel = if include_tcp {
        format!(
            r#"
[[tunnels]]
name = "tcp-echo"
group = "test"
kind = "tcp"
bind = "{tcp_public}"
"#
        )
    } else {
        String::new()
    };
    let server = format!(
        r#"
[quic]
bind = "{quic}"
certificate = "server.pem"
private_key = "server-key.pem"

[[groups]]
name = "test"
key = "{TEST_KEY}"
{tcp_tunnel}

[[tunnels]]
name = "udp-echo"
group = "test"
kind = "udp"
bind = "{udp_public}"

[[tunnels]]
name = "socks"
group = "test"
kind = "socks5"
bind = "{socks_public}"
"#
    );
    std::fs::write(directory.join("server.toml"), server).expect("应能写服务端配置");
}

fn write_client_config(
    directory: &Path,
    quic: SocketAddr,
    tcp_target: SocketAddr,
    udp_target: SocketAddr,
    include_tcp: bool,
) {
    let tcp_service = if include_tcp {
        format!(
            r#"
[[services]]
name = "tcp-echo"
kind = "tcp"
target = "{tcp_target}"
"#
        )
    } else {
        String::new()
    };
    let client = format!(
        r#"
[server]
address = "{quic}"
name = "localhost"
ca_certificate = "server.pem"

[group]
name = "test"
key = "{TEST_KEY}"
{tcp_service}

[[services]]
name = "udp-echo"
kind = "udp"
target = "{udp_target}"

[[services]]
name = "socks"
kind = "socks5"
"#
    );
    std::fs::write(directory.join("client.toml"), client).expect("应能写客户端配置");
}

async fn run_tcp_echo(listener: TcpListener, cancellation: CancellationToken) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            accepted = listener.accept() => {
                let Ok((mut stream, _)) = accepted else { break };
                connections.spawn(async move {
                    let (mut reader, mut writer) = stream.split();
                    tokio::io::copy(&mut reader, &mut writer).await
                });
            }
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                assert!(result.expect("TCP 回显任务不应 panic").is_ok());
            }
        }
    }
}

async fn run_udp_echo(socket: UdpSocket, cancellation: CancellationToken) {
    let mut buffer = vec![0; 65_535];
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            received = socket.recv_from(&mut buffer) => {
                let (length, peer) = received.expect("UDP 回显接收不应失败");
                socket.send_to(&buffer[..length], peer).await.expect("UDP 回显发送不应失败");
            }
        }
    }
}

async fn assert_stream_echo(address: SocketAddr, payload: &[u8]) {
    for _ in 0..50 {
        if let Ok(mut stream) = TcpStream::connect(address).await
            && stream.write_all(payload).await.is_ok()
        {
            let mut response = vec![0; payload.len()];
            if tokio::time::timeout(Duration::from_millis(200), stream.read_exact(&mut response))
                .await
                .is_ok_and(|result| result.is_ok())
                && response == payload
            {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("TCP 隧道未在预期时间内就绪");
}

async fn wait_until_stream_unavailable(address: SocketAddr) {
    for _ in 0..50 {
        let unavailable = match TcpStream::connect(address).await {
            Ok(mut stream) => {
                if stream.write_all(b"must-not-echo").await.is_err() {
                    true
                } else {
                    matches!(
                        tokio::time::timeout(Duration::from_millis(100), stream.read_u8()).await,
                        Ok(Err(_))
                    )
                }
            }
            Err(_) => true,
        };
        if unavailable {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("TCP 隧道未在预期时间内停止接受新连接");
}

async fn exchange(stream: &mut TcpStream, payload: &[u8]) {
    stream
        .write_all(payload)
        .await
        .expect("持久隧道写入不应失败");
    let mut response = vec![0; payload.len()];
    tokio::time::timeout(Duration::from_secs(1), stream.read_exact(&mut response))
        .await
        .expect("持久隧道读取不应超时")
        .expect("持久隧道读取不应失败");
    assert_eq!(response, payload);
}

async fn assert_udp_echo(address: SocketAddr, payload: &[u8]) {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("应能绑定 UDP 测试客户端");
    let mut response = vec![0; payload.len()];
    for _ in 0..50 {
        socket
            .send_to(payload, address)
            .await
            .expect("应能发送 UDP 测试包");
        if let Ok(Ok((length, _))) =
            tokio::time::timeout(Duration::from_millis(200), socket.recv_from(&mut response)).await
            && response[..length] == *payload
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("UDP 隧道未在预期时间内就绪");
}

async fn assert_socks_echo(proxy: SocketAddr, target: SocketAddr, payload: &[u8]) {
    let mut stream = TcpStream::connect(proxy)
        .await
        .expect("应能连接 SOCKS5 公网监听");
    stream
        .write_all(&[5, 1, 0])
        .await
        .expect("应能发送 SOCKS5 协商");
    let mut negotiation = [0; 2];
    stream
        .read_exact(&mut negotiation)
        .await
        .expect("应能读取 SOCKS5 协商");
    assert_eq!(negotiation, [5, 0]);
    let SocketAddr::V4(target) = target else {
        panic!("测试目标应为 IPv4");
    };
    let mut request = vec![5, 1, 0, 1];
    request.extend_from_slice(&target.ip().octets());
    request.extend_from_slice(&target.port().to_be_bytes());
    stream
        .write_all(&request)
        .await
        .expect("应能发送 SOCKS5 请求");
    let mut reply = [0; 10];
    stream
        .read_exact(&mut reply)
        .await
        .expect("应能读取 SOCKS5 响应");
    assert_eq!(reply[1], 0);
    stream
        .write_all(payload)
        .await
        .expect("应能经 SOCKS5 发送数据");
    let mut response = vec![0; payload.len()];
    stream
        .read_exact(&mut response)
        .await
        .expect("应能经 SOCKS5 读取数据");
    assert_eq!(response, payload);
}

fn unused_tcp_address() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("应能预留 TCP 地址");
    listener.local_addr().expect("应能读取 TCP 地址")
}

fn unused_udp_address() -> SocketAddr {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("应能预留 UDP 地址");
    socket.local_addr().expect("应能读取 UDP 地址")
}

async fn assert_task_ok(task: JoinHandle<crate::Result<()>>, name: &str) {
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap_or_else(|_| panic!("{name}未按时退出"))
        .unwrap_or_else(|error| panic!("{name}任务异常: {error}"));
    result.unwrap_or_else(|error| panic!("{name}返回错误: {error}"));
}

async fn assert_join_ok(task: JoinHandle<()>, name: &str) {
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap_or_else(|_| panic!("{name}未按时退出"))
        .unwrap_or_else(|error| panic!("{name}任务异常: {error}"));
}
