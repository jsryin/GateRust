use std::{io::Cursor, net::IpAddr};

use rustls::pki_types::CertificateDer;
use x509_parser::{extensions::GeneralName, parse_x509_certificate};

use crate::{Result, TunnelError, config::address_host};

const MAX_CERTIFICATE_CHAIN_LENGTH: usize = 16;

/// 经分组密钥证明验证后取得的服务端证书。
#[derive(Debug)]
pub struct DownloadedServerCertificate {
    pem: String,
    server_name: String,
}

impl DownloadedServerCertificate {
    pub(crate) fn from_chain(
        certificates: &[CertificateDer<'static>],
        address: &str,
    ) -> Result<Self> {
        if certificates.is_empty() || certificates.len() > MAX_CERTIFICATE_CHAIN_LENGTH {
            return Err(TunnelError::Protocol("服务端证书链数量无效".into()));
        }
        let server_name = server_name_from_der(certificates[0].as_ref(), address)?;
        let blocks = certificates
            .iter()
            .map(|certificate| pem::Pem::new("CERTIFICATE", certificate.as_ref()))
            .collect::<Vec<_>>();
        Ok(Self {
            pem: pem::encode_many(&blocks),
            server_name,
        })
    }

    #[must_use]
    pub fn pem(&self) -> &str {
        &self.pem
    }

    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

/// 从 PEM 证书中选择可用于 TLS 校验的服务器名称。
///
/// # Errors
///
/// PEM 或叶证书无效，或者证书不包含可用的 DNS/IP SAN 时返回错误。
pub fn server_name_from_pem(content: &[u8], address: &str) -> Result<String> {
    let certificate = rustls_pemfile::certs(&mut Cursor::new(content))
        .next()
        .transpose()?
        .ok_or_else(|| TunnelError::Tls("server.pem 不包含证书".into()))?;
    server_name_from_der(certificate.as_ref(), address)
}

fn server_name_from_der(certificate: &[u8], address: &str) -> Result<String> {
    let (_, certificate) = parse_x509_certificate(certificate)
        .map_err(|_| TunnelError::Tls("解析服务端叶证书失败".into()))?;
    let names = certificate
        .subject_alternative_name()
        .map_err(|_| TunnelError::Tls("解析服务端证书 SAN 失败".into()))?
        .ok_or_else(|| TunnelError::Tls("服务端证书缺少 SAN".into()))?;
    let address_host = address_host(address)
        .ok_or_else(|| TunnelError::InvalidConfig("无法从服务器地址推导主机名".into()))?;
    let address_ip = address_host.parse::<IpAddr>().ok();

    for name in &names.value.general_names {
        match name {
            GeneralName::DNSName(dns) if dns_matches(dns, address_host) => {
                return Ok(address_host.to_owned());
            }
            GeneralName::IPAddress(raw)
                if ip_from_san(raw).is_some_and(|ip| Some(ip) == address_ip) =>
            {
                return Ok(address_host.to_owned());
            }
            _ => {}
        }
    }
    names
        .value
        .general_names
        .iter()
        .find_map(|name| match name {
            GeneralName::DNSName(dns) if !dns.contains('*') => Some((*dns).to_owned()),
            _ => None,
        })
        .or_else(|| {
            names
                .value
                .general_names
                .iter()
                .find_map(|name| match name {
                    GeneralName::IPAddress(raw) => ip_from_san(raw).map(|ip| ip.to_string()),
                    _ => None,
                })
        })
        .ok_or_else(|| TunnelError::Tls("服务端证书不包含可用的 DNS/IP SAN".into()))
}

fn dns_matches(pattern: &str, host: &str) -> bool {
    if pattern.eq_ignore_ascii_case(host) {
        return true;
    }
    let Some(suffix) = pattern.strip_prefix("*.") else {
        return false;
    };
    host.len() > suffix.len()
        && host
            .get(host.len() - suffix.len()..)
            .is_some_and(|value| value.eq_ignore_ascii_case(suffix))
        && host.as_bytes().get(host.len() - suffix.len() - 1) == Some(&b'.')
        && !host[..host.len() - suffix.len() - 1].contains('.')
}

fn ip_from_san(raw: &[u8]) -> Option<IpAddr> {
    match raw {
        [a, b, c, d] => Some(IpAddr::from([*a, *b, *c, *d])),
        bytes if bytes.len() == 16 => {
            let octets: [u8; 16] = bytes.try_into().ok()?;
            Some(IpAddr::from(octets))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use rcgen::generate_simple_self_signed;

    use super::*;

    #[test]
    fn selects_address_name_or_first_concrete_san() {
        let certificate =
            generate_simple_self_signed(vec!["localhost".into(), "tunnel.example.com".into()])
                .expect("生成测试证书");
        let pem = certificate.cert.pem();

        assert_eq!(
            server_name_from_pem(pem.as_bytes(), "localhost:2333").expect("匹配地址名称"),
            "localhost"
        );
        assert_eq!(
            server_name_from_pem(pem.as_bytes(), "127.0.0.1:2333").expect("选择证书名称"),
            "localhost"
        );
    }
}
