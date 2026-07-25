use std::{fs::File, io::BufReader, net::SocketAddr, path::Path, sync::Arc, time::Duration};

use quinn::{ClientConfig, Endpoint, ServerConfig, TransportConfig, VarInt};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
};

use crate::{Result, TunnelError, certificate::validate_server_leaf, config::ServerQuicConfig};

pub(crate) fn server_endpoint(
    config: &ServerQuicConfig,
) -> Result<(Endpoint, CertificateDer<'static>)> {
    let (server, certificate) = build_server_config(config)?;
    let endpoint = Endpoint::server(server, config.bind)?;
    Ok((endpoint, certificate))
}

pub(crate) fn validate_server_credentials(config: &ServerQuicConfig) -> Result<()> {
    build_server_config(config).map(drop)
}

fn build_server_config(
    config: &ServerQuicConfig,
) -> Result<(ServerConfig, CertificateDer<'static>)> {
    let certificates = read_certificates(&config.certificate)?;
    let certificate = certificates
        .first()
        .cloned()
        .ok_or_else(|| TunnelError::Tls("服务端证书文件不包含叶证书".into()))?;
    validate_server_leaf(certificate.as_ref())?;
    let private_key = read_private_key(&config.private_key)?;
    let mut server = ServerConfig::with_single_cert(certificates, private_key)
        .map_err(|error| TunnelError::Tls(error.to_string()))?;
    server.transport_config(transport_config());
    Ok((server, certificate))
}

pub(crate) fn client_config(ca_path: Option<&Path>) -> Result<ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca_path) = ca_path {
        for certificate in read_certificates(ca_path)? {
            roots
                .add(certificate)
                .map_err(|error| TunnelError::Tls(format!("添加 CA 证书失败: {error}")))?;
        }
    } else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    quinn_client_config(tls)
}

pub(crate) fn bootstrap_client_config() -> Result<ClientConfig> {
    let tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(BootstrapCertificateVerifier::new())
        .with_no_client_auth();
    quinn_client_config(tls)
}

fn quinn_client_config(tls: rustls::ClientConfig) -> Result<ClientConfig> {
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|error| TunnelError::Tls(error.to_string()))?;
    let mut client = ClientConfig::new(Arc::new(crypto));
    client.transport_config(transport_config());
    Ok(client)
}

pub(crate) fn client_endpoint(server: SocketAddr, client: ClientConfig) -> Result<Endpoint> {
    let bind = if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    }
    .parse()?;
    let mut endpoint = Endpoint::client(bind)?;
    endpoint.set_default_client_config(client);
    Ok(endpoint)
}

/// 引导连接只跳过证书链和名称校验，TLS 握手签名仍由 rustls/ring 验证。
/// 连接建立后还必须完成绑定证书摘要的分组密钥双向证明，才能信任并保存证书。
#[derive(Debug)]
struct BootstrapCertificateVerifier(Arc<rustls::crypto::CryptoProvider>);

impl BootstrapCertificateVerifier {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl ServerCertVerifier for BootstrapCertificateVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn transport_config() -> Arc<TransportConfig> {
    let mut transport = TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(15)));
    transport.max_idle_timeout(Some(VarInt::from_u32(60_000).into()));
    transport.max_concurrent_bidi_streams(VarInt::from_u32(4_096));
    Arc::new(transport)
}

fn read_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path).map_err(|source| TunnelError::ReadConfig {
        path: path.to_owned(),
        source,
    })?;
    let certificates = rustls_pemfile::certs(&mut BufReader::new(file))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if certificates.is_empty() {
        return Err(TunnelError::Tls(format!(
            "证书文件 {} 不包含证书",
            path.display()
        )));
    }
    Ok(certificates)
}

fn read_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = File::open(path).map_err(|source| TunnelError::ReadConfig {
        path: path.to_owned(),
        source,
    })?;
    rustls_pemfile::private_key(&mut BufReader::new(file))?
        .ok_or_else(|| TunnelError::Tls(format!("私钥文件 {} 不包含私钥", path.display())))
}
