use std::{net::SocketAddr, path::Path, time::Duration};

use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, generate_simple_self_signed};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::watch,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    ClientConfig, ClientServerConfig, ClientServiceConfig, ClientStatus, ClientTunnelState,
    ServerConfig, TunnelError, TunnelKind, TunnelRuntime, TunnelRuntimeSnapshot,
    check_server_config, client_control_channel, fetch_server_certificate,
    run_client_with_shutdown, run_managed_client_with_status, run_server_with_runtime,
    verify_client_credentials,
};

const TEST_KEY: &str = "12345678901234567890123456789012";
const ROTATED_TEST_KEY: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";

#[test]
fn checks_server_tls_credentials_before_startup() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_certificate(directory.path());
    let config = r#"
[quic]
bind = "127.0.0.1:2333"
certificate = "server.pem"
private_key = "server-key.pem"
"#;
    let path = directory.path().join("server.toml");
    std::fs::write(&path, config).expect("应能写服务端配置");
    check_server_config(&path).expect("有效 TLS 凭据应通过校验");

    std::fs::remove_file(directory.path().join("server-key.pem")).expect("应能删除测试私钥");
    let error = check_server_config(&path).expect_err("缺少私钥应校验失败");
    assert!(error.to_string().contains("server-key.pem"));
}

#[test]
fn rejects_ca_certificate_before_server_startup() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_ca_certificate(directory.path());
    let config = r#"
[quic]
bind = "127.0.0.1:2333"
certificate = "server.pem"
private_key = "server-key.pem"
"#;
    let path = directory.path().join("server.toml");
    std::fs::write(&path, config).expect("应能写服务端配置");

    let error = check_server_config(&path).expect_err("CA 证书不能作为服务端叶证书");

    assert!(matches!(error, TunnelError::ServerCertificateIsCa));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstraps_certificate_with_key_proof_and_checks_credentials() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_certificate(directory.path());
    let quic = unused_udp_address();
    write_server_config(
        directory.path(),
        quic,
        unused_tcp_address(),
        unused_udp_address(),
        unused_tcp_address(),
        true,
    );
    let cancellation = CancellationToken::new();
    let server_path = directory.path().join("server.toml");
    let server_cancel = cancellation.clone();
    let server = tokio::spawn(async move {
        run_server_with_runtime(server_path, TunnelRuntime::new(), server_cancel).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // localhost 同时返回 IPv4/IPv6 时，应由可用地址先完成证书引导。
    let address = format!("localhost:{}", quic.port());
    let downloaded = fetch_server_certificate(&address, TEST_KEY)
        .await
        .expect("正确密钥应下载证书");
    assert_eq!(downloaded.server_name(), "localhost");
    let certificate_path = directory.path().join("downloaded-server.pem");
    std::fs::write(&certificate_path, downloaded.pem()).expect("保存下载的证书");
    let config = ClientConfig {
        key: TEST_KEY.into(),
        server: ClientServerConfig {
            address: address.clone(),
            name: Some(downloaded.server_name().into()),
            ca_certificate: Some(certificate_path),
        },
        services: Vec::new(),
    };
    let tunnels = verify_client_credentials(&config)
        .await
        .expect("下载证书后应通过正常认证");
    assert_eq!(tunnels.len(), 3);

    let error = fetch_server_certificate(&address, "00000000000000000000000000000000")
        .await
        .expect_err("错误密钥必须被拒绝");
    assert!(matches!(error, TunnelError::Authentication(_)));

    cancellation.cancel();
    assert_task_ok(server, "服务端").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hot_reloads_quic_listener_and_tls_credentials() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_certificate(directory.path());
    write_named_certificate(directory.path(), "rotated.pem", "rotated-key.pem");
    let initial_address = unused_udp_address();
    let updated_address = unused_udp_address();
    write_server_config(
        directory.path(),
        initial_address,
        unused_tcp_address(),
        unused_udp_address(),
        unused_tcp_address(),
        true,
    );

    let runtime = TunnelRuntime::new();
    let cancellation = CancellationToken::new();
    let server_path = directory.path().join("server.toml");
    let server = tokio::spawn(run_server_with_runtime(
        server_path.clone(),
        runtime.clone(),
        cancellation.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let initial_certificate =
        fetch_server_certificate(&format!("localhost:{}", initial_address.port()), TEST_KEY)
            .await
            .expect("初始 QUIC 入口应可下载证书")
            .pem()
            .to_owned();

    let revision = runtime.config_revision();
    let mut config = ServerConfig::read(&server_path).expect("读取服务端配置");
    config.quic.bind = updated_address;
    config.quic.certificate = "rotated.pem".into();
    config.quic.private_key = "rotated-key.pem".into();
    write_server_config_value(&server_path, &config);
    wait_for_runtime(&runtime, |snapshot| {
        snapshot.config_status.revision > revision
            && snapshot.config_status.last_apply_error.is_none()
    })
    .await;

    let updated_certificate =
        fetch_server_certificate(&format!("localhost:{}", updated_address.port()), TEST_KEY)
            .await
            .expect("更新后的 QUIC 入口应立即可用")
            .pem()
            .to_owned();
    assert_ne!(updated_certificate, initial_certificate);

    cancellation.cancel();
    assert_task_ok(server, "服务端").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_current_quic_listener_when_rebind_fails() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_certificate(directory.path());
    let initial_address = unused_udp_address();
    let occupied = std::net::UdpSocket::bind("127.0.0.1:0").expect("占用测试 UDP 端口");
    let occupied_address = occupied.local_addr().expect("读取占用地址");
    write_server_config(
        directory.path(),
        initial_address,
        unused_tcp_address(),
        unused_udp_address(),
        unused_tcp_address(),
        true,
    );

    let runtime = TunnelRuntime::new();
    let cancellation = CancellationToken::new();
    let server_path = directory.path().join("server.toml");
    let server = tokio::spawn(run_server_with_runtime(
        server_path.clone(),
        runtime.clone(),
        cancellation.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let revision = runtime.config_revision();
    let mut config = ServerConfig::read(&server_path).expect("读取服务端配置");
    config.quic.bind = occupied_address;
    write_server_config_value(&server_path, &config);
    wait_for_runtime(&runtime, |snapshot| {
        snapshot.config_status.revision > revision
            && snapshot.config_status.last_apply_error.is_some()
    })
    .await;

    fetch_server_certificate(&format!("localhost:{}", initial_address.port()), TEST_KEY)
        .await
        .expect("换绑失败后原 QUIC 入口应继续可用");

    cancellation.cancel();
    assert_task_ok(server, "服务端").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotating_group_key_revokes_existing_sessions() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_certificate(directory.path());
    let quic = unused_udp_address();
    write_server_config(
        directory.path(),
        quic,
        unused_tcp_address(),
        unused_udp_address(),
        unused_tcp_address(),
        true,
    );
    write_client_config(
        directory.path(),
        quic,
        unused_tcp_address(),
        unused_udp_address(),
        true,
    );
    let cancellation = CancellationToken::new();
    let runtime = TunnelRuntime::new();
    let server_path = directory.path().join("server.toml");
    let server_cancel = cancellation.clone();
    let server_runtime = runtime.clone();
    let server = tokio::spawn(async move {
        run_server_with_runtime(server_path, server_runtime, server_cancel).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let client_path = directory.path().join("client.toml");
    let client_cancel = cancellation.clone();
    let client =
        tokio::spawn(async move { run_client_with_shutdown(client_path, client_cancel).await });
    wait_for_runtime(&runtime, |snapshot| snapshot.clients.len() == 1).await;

    let server_path = directory.path().join("server.toml");
    let config = std::fs::read_to_string(&server_path)
        .expect("读取服务端配置")
        .replace(TEST_KEY, ROTATED_TEST_KEY);
    std::fs::write(&server_path, config).expect("轮换分组密钥");
    let snapshot = wait_for_runtime(&runtime, |snapshot| {
        snapshot.clients.is_empty()
            && snapshot.config_status.revision >= 2
            && snapshot.config_status.last_apply_error.is_none()
    })
    .await;
    assert!(
        snapshot
            .tunnels
            .iter()
            .all(|tunnel| tunnel.owner_session_id.is_none())
    );

    let address = format!("localhost:{}", quic.port());
    assert!(fetch_server_certificate(&address, TEST_KEY).await.is_err());
    fetch_server_certificate(&address, ROTATED_TEST_KEY)
        .await
        .expect("新密钥应立即生效");

    cancellation.cancel();
    assert_task_ok(server, "服务端").await;
    assert_task_ok(client, "客户端").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_client_applies_confirmed_ephemeral_tunnel_selection() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_certificate(directory.path());
    let quic = unused_udp_address();
    let public = unused_tcp_address();
    write_server_config(
        directory.path(),
        quic,
        public,
        unused_udp_address(),
        unused_tcp_address(),
        true,
    );
    write_client_config(
        directory.path(),
        quic,
        "127.0.0.1:9".parse().expect("测试目标地址有效"),
        unused_udp_address(),
        true,
    );

    let cancellation = CancellationToken::new();
    let runtime = TunnelRuntime::new();
    let server_path = directory.path().join("server.toml");
    let server_cancel = cancellation.clone();
    let server_runtime = runtime.clone();
    let server = tokio::spawn(async move {
        run_server_with_runtime(server_path, server_runtime, server_cancel).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client_path = directory.path().join("client.toml");
    let client_cancel = cancellation.clone();
    let (controller, commands) = client_control_channel();
    let (status, _status_receiver) = watch::channel(ClientStatus::Starting);
    let client = tokio::spawn(async move {
        run_managed_client_with_status(client_path, client_cancel, status, commands).await
    });
    wait_for_runtime(&runtime, |snapshot| {
        snapshot.clients.len() == 1
            && snapshot
                .tunnels
                .iter()
                .all(|tunnel| tunnel.owner_session_id.is_none())
    })
    .await;

    let services = vec![ClientServiceConfig {
        name: "tcp-echo".into(),
        kind: TunnelKind::Tcp,
        target: Some("127.0.0.1:9".into()),
    }];
    let enabled = tokio::time::timeout(
        Duration::from_secs(2),
        controller.update_services(services.clone()),
    )
    .await
    .expect("启用请求不应超时")
    .expect("启用请求应成功");
    assert!(
        enabled
            .iter()
            .find(|tunnel| tunnel.name == "tcp-echo")
            .is_some_and(|tunnel| tunnel.state == ClientTunnelState::Enabled)
    );

    let mut changes = runtime.subscribe();
    changes.borrow_and_update();
    controller
        .update_services(services)
        .await
        .expect("重复启用应返回当前状态");
    assert!(
        tokio::time::timeout(Duration::from_millis(150), changes.changed())
            .await
            .is_err(),
        "无状态变化的重复声明不应广播运行时更新"
    );

    let disabled = controller
        .update_services(Vec::new())
        .await
        .expect("停用请求应成功");
    assert!(
        disabled
            .iter()
            .all(|tunnel| tunnel.state != ClientTunnelState::Enabled)
    );
    let snapshot = runtime.snapshot().await;
    assert!(
        snapshot
            .tunnels
            .iter()
            .all(|tunnel| tunnel.owner_session_id.is_none())
    );

    cancellation.cancel();
    assert_task_ok(server, "服务端").await;
    assert_task_ok(client, "客户端").await;
}

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
    let runtime = TunnelRuntime::new();
    let server_path = directory.path().join("server.toml");
    let client_path = directory.path().join("client.toml");
    let server_cancel = cancellation.clone();
    let server_runtime = runtime.clone();
    let server = tokio::spawn(async move {
        run_server_with_runtime(server_path, server_runtime, server_cancel).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let client_cancel = cancellation.clone();
    let client_task_path = client_path.clone();
    let client =
        tokio::spawn(
            async move { run_client_with_shutdown(client_task_path, client_cancel).await },
        );

    let initial = wait_for_runtime(&runtime, |snapshot| {
        snapshot.clients.len() == 1
            && snapshot
                .tunnels
                .iter()
                .find(|tunnel| tunnel.name == "tcp-echo")
                .is_some_and(|tunnel| tunnel.owner_session_id.is_some())
    })
    .await;
    let session_id = initial.clients[0].session_id;

    assert_stream_echo(tcp_public, b"tcp-through-quic").await;
    let mut persistent = TcpStream::connect(tcp_public)
        .await
        .expect("应能建立持久 TCP 隧道");
    exchange(&mut persistent, b"before-reload").await;

    let client_config = std::fs::read(&client_path).expect("应能暂存客户端配置");
    std::fs::remove_file(&client_path).expect("应能模拟 Windows 配置替换空窗");
    tokio::time::sleep(Duration::from_millis(25)).await;
    std::fs::write(&client_path, client_config).expect("应能完成客户端配置替换");
    tokio::time::sleep(Duration::from_millis(150)).await;
    let snapshot = runtime.snapshot().await;
    assert_eq!(snapshot.clients.len(), 1);
    assert_eq!(snapshot.clients[0].session_id, session_id);
    exchange(&mut persistent, b"after-config-replace").await;

    let mut runtime_changes = runtime.subscribe();
    runtime_changes.borrow_and_update();
    write_client_config(
        directory.path(),
        quic,
        tcp_target_address,
        udp_target_address,
        false,
    );
    tokio::time::timeout(Duration::from_secs(2), runtime_changes.changed())
        .await
        .expect("服务端应收到后续客户端配置更新")
        .expect("运行时状态通道保持打开");
    let snapshot = runtime.snapshot().await;
    assert_eq!(snapshot.clients.len(), 1);
    assert_eq!(snapshot.clients[0].session_id, session_id);
    assert!(
        snapshot
            .tunnels
            .iter()
            .find(|tunnel| tunnel.name == "tcp-echo")
            .is_some_and(|tunnel| tunnel.owner_session_id.is_none())
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
    wait_for_runtime(&runtime, |snapshot| {
        snapshot
            .tunnels
            .iter()
            .find(|tunnel| tunnel.name == "tcp-echo")
            .is_some_and(|tunnel| tunnel.owner_session_id.is_none())
    })
    .await;
    write_client_config(
        directory.path(),
        quic,
        tcp_target_address,
        udp_target_address,
        true,
    );
    assert_stream_echo(tcp_public, b"after-server-add").await;
    drop(persistent);

    assert_udp_echo(udp_public, b"udp-through-quic").await;
    let config_revision = runtime.snapshot().await.config_status.revision;
    let server_path = directory.path().join("server.toml");
    let config = std::fs::read_to_string(&server_path)
        .expect("读取 UDP 重载前配置")
        .replace("udp_idle_seconds = 30", "udp_idle_seconds = 31");
    std::fs::write(&server_path, config).expect("修改 UDP 会话空闲时间");
    wait_for_runtime(&runtime, |snapshot| {
        snapshot.config_status.revision > config_revision
            && snapshot.config_status.last_apply_error.is_none()
    })
    .await;
    assert_udp_echo(udp_public, b"udp-after-reload").await;
    assert_socks_echo(socks_public, tcp_target_address, b"socks-through-quic").await;

    cancellation.cancel();
    echo_cancel.cancel();
    assert_task_ok(server, "服务端").await;
    assert_task_ok(client, "客户端").await;
    assert_join_ok(tcp_echo, "TCP 回显服务").await;
    assert_join_ok(udp_echo, "UDP 回显服务").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn occupied_tunnel_requires_a_new_selection_after_release() {
    let directory = tempfile::tempdir().expect("应能创建测试目录");
    write_certificate(directory.path());
    let quic = unused_udp_address();
    let public = unused_tcp_address();
    let server_config = format!(
        r#"
[quic]
bind = "{quic}"
certificate = "server.pem"
private_key = "server-key.pem"

[[groups]]
name = "shared"
key = "{TEST_KEY}"

[[tunnels]]
name = "shared-tunnel"
group = "shared"
kind = "tcp"
bind = "{public}"
local_port = 9
"#
    );
    std::fs::write(directory.path().join("server.toml"), server_config).expect("应能写服务端配置");
    let client_config = format!(
        r#"
key = "{TEST_KEY}"

[server]
address = "{quic}"
name = "localhost"
ca_certificate = "server.pem"

[[services]]
name = "shared-tunnel"
kind = "tcp"
target = "127.0.0.1:9"
"#
    );
    let first_path = directory.path().join("first.toml");
    let second_path = directory.path().join("second.toml");
    std::fs::write(&first_path, &client_config).expect("应能写第一个客户端配置");
    std::fs::write(&second_path, &client_config).expect("应能写第二个客户端配置");

    let cancellation = CancellationToken::new();
    let runtime = TunnelRuntime::new();
    let server_cancel = cancellation.clone();
    let server_runtime = runtime.clone();
    let server_path = directory.path().join("server.toml");
    let server = tokio::spawn(async move {
        run_server_with_runtime(server_path, server_runtime, server_cancel).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let first_cancel = cancellation.clone();
    let first =
        tokio::spawn(async move { run_client_with_shutdown(first_path, first_cancel).await });
    wait_for_runtime(&runtime, |snapshot| snapshot.clients.len() == 1).await;
    let second_cancel = cancellation.clone();
    let second_runtime_path = second_path.clone();
    let second =
        tokio::spawn(
            async move { run_client_with_shutdown(second_runtime_path, second_cancel).await },
        );
    let snapshot = wait_for_runtime(&runtime, |snapshot| {
        snapshot.clients.len() == 2
            && snapshot
                .tunnels
                .first()
                .is_some_and(|tunnel| tunnel.owner_session_id.is_some())
    })
    .await;
    let tunnel = snapshot.tunnels.first().expect("应存在隧道状态");
    let owner = tunnel.owner_session_id.expect("应存在隧道所有者");
    let occupied_client = snapshot
        .clients
        .iter()
        .find(|client| client.session_id != owner)
        .expect("应存在未获得隧道的客户端")
        .session_id;
    let catalog = runtime.catalog(occupied_client).await;
    assert_eq!(catalog[0].state, ClientTunnelState::Occupied);
    assert_eq!(catalog[0].server_port, public.port());
    assert_eq!(catalog[0].local_ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(catalog[0].local_port, Some(9));

    assert!(runtime.disconnect(owner).await);
    wait_for_runtime(&runtime, |snapshot| {
        snapshot.clients.len() == 1
            && snapshot
                .tunnels
                .first()
                .is_some_and(|tunnel| tunnel.owner_session_id.is_none())
    })
    .await;
    assert_eq!(
        runtime.catalog(occupied_client).await[0].state,
        ClientTunnelState::Idle
    );

    // 释放后不自动转交；重新保存相同选择代表客户端再次显式连接。
    ClientConfig::read(&second_path)
        .expect("应能读取第二个客户端配置")
        .save(&second_path)
        .expect("应能重新提交客户端选择");
    let claimed = wait_for_runtime(&runtime, |snapshot| {
        snapshot
            .tunnels
            .first()
            .is_some_and(|tunnel| tunnel.owner_session_id == Some(occupied_client))
    })
    .await;
    assert_eq!(claimed.clients.len(), 1);

    cancellation.cancel();
    assert_task_ok(server, "服务端").await;
    assert_task_ok(first, "第一个客户端").await;
    assert_task_ok(second, "第二个客户端").await;
}

async fn wait_for_runtime(
    runtime: &TunnelRuntime,
    predicate: impl Fn(&TunnelRuntimeSnapshot) -> bool,
) -> TunnelRuntimeSnapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = runtime.snapshot().await;
            if predicate(&snapshot) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("等待运行时状态超时")
}

fn write_certificate(directory: &Path) {
    write_named_certificate(directory, "server.pem", "server-key.pem");
}

fn write_named_certificate(directory: &Path, certificate: &str, private_key: &str) {
    let certified =
        generate_simple_self_signed(vec!["localhost".into()]).expect("应能生成测试证书");
    std::fs::write(directory.join(certificate), certified.cert.pem()).expect("应能写入测试证书");
    std::fs::write(
        directory.join(private_key),
        certified.signing_key.serialize_pem(),
    )
    .expect("应能写入测试私钥");
}

fn write_server_config_value(path: &Path, config: &ServerConfig) {
    let content = toml::to_string(config).expect("序列化服务端测试配置");
    std::fs::write(path, content).expect("写入服务端测试配置");
}

fn write_ca_certificate(directory: &Path) {
    let mut params =
        CertificateParams::new(vec!["localhost".into()]).expect("应能创建测试证书参数");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let signing_key = KeyPair::generate().expect("应能生成测试密钥");
    let certificate = params
        .self_signed(&signing_key)
        .expect("应能生成测试 CA 证书");
    std::fs::write(directory.join("server.pem"), certificate.pem()).expect("应能写入测试证书");
    std::fs::write(
        directory.join("server-key.pem"),
        signing_key.serialize_pem(),
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
udp_idle_seconds = 30

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
key = "{TEST_KEY}"

[server]
address = "{quic}"
name = "localhost"
ca_certificate = "server.pem"

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
    panic!(
        "TCP 隧道未在预期时间内就绪: {}",
        String::from_utf8_lossy(payload)
    );
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
