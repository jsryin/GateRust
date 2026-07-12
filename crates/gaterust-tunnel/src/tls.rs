use std::{fs::File, io::BufReader, net::SocketAddr, path::Path, sync::Arc, time::Duration};

use quinn::{ClientConfig, Endpoint, ServerConfig, TransportConfig, VarInt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::{Result, TunnelError, config::ServerQuicConfig};

pub(crate) fn server_endpoint(config: &ServerQuicConfig) -> Result<Endpoint> {
    let server = build_server_config(config)?;
    Endpoint::server(server, config.bind).map_err(Into::into)
}

pub(crate) fn validate_server_credentials(config: &ServerQuicConfig) -> Result<()> {
    build_server_config(config).map(drop)
}

fn build_server_config(config: &ServerQuicConfig) -> Result<ServerConfig> {
    let certificates = read_certificates(&config.certificate)?;
    let private_key = read_private_key(&config.private_key)?;
    let mut server = ServerConfig::with_single_cert(certificates, private_key)
        .map_err(|error| TunnelError::Tls(error.to_string()))?;
    server.transport_config(transport_config());
    Ok(server)
}

pub(crate) fn client_endpoint(server: SocketAddr, ca_path: Option<&Path>) -> Result<Endpoint> {
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
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|error| TunnelError::Tls(error.to_string()))?;
    let mut client = ClientConfig::new(Arc::new(crypto));
    client.transport_config(transport_config());

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
