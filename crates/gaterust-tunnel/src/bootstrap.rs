use hmac::{Hmac, Mac as _};
use rand::RngExt as _;
use sha2::{Digest as _, Sha256};

use crate::{Result, TunnelError};

pub(crate) const NONCE_LENGTH: usize = 32;
pub(crate) const PROOF_LENGTH: usize = 32;

type HmacSha256 = Hmac<Sha256>;

const CLIENT_PROOF_DOMAIN: &[u8] = b"gaterust/certificate-bootstrap/client/v1";
const SERVER_PROOF_DOMAIN: &[u8] = b"gaterust/certificate-bootstrap/server/v1";

pub(crate) fn random_nonce() -> [u8; NONCE_LENGTH] {
    let mut nonce = [0_u8; NONCE_LENGTH];
    rand::rng().fill(&mut nonce);
    nonce
}

pub(crate) fn client_proof(
    key: &[u8],
    version: u16,
    client_nonce: &[u8; NONCE_LENGTH],
    certificate: &[u8],
) -> Result<[u8; PROOF_LENGTH]> {
    proof(
        key,
        CLIENT_PROOF_DOMAIN,
        version,
        client_nonce,
        None,
        certificate,
    )
}

pub(crate) fn server_proof(
    key: &[u8],
    version: u16,
    client_nonce: &[u8; NONCE_LENGTH],
    server_nonce: &[u8; NONCE_LENGTH],
    certificate: &[u8],
) -> Result<[u8; PROOF_LENGTH]> {
    proof(
        key,
        SERVER_PROOF_DOMAIN,
        version,
        client_nonce,
        Some(server_nonce),
        certificate,
    )
}

pub(crate) fn verify_client_proof(
    key: &[u8],
    version: u16,
    client_nonce: &[u8; NONCE_LENGTH],
    certificate: &[u8],
    candidate: &[u8; PROOF_LENGTH],
) -> bool {
    verify(
        key,
        CLIENT_PROOF_DOMAIN,
        version,
        client_nonce,
        None,
        certificate,
        candidate,
    )
}

pub(crate) fn verify_server_proof(
    key: &[u8],
    version: u16,
    client_nonce: &[u8; NONCE_LENGTH],
    server_nonce: &[u8; NONCE_LENGTH],
    certificate: &[u8],
    candidate: &[u8; PROOF_LENGTH],
) -> bool {
    verify(
        key,
        SERVER_PROOF_DOMAIN,
        version,
        client_nonce,
        Some(server_nonce),
        certificate,
        candidate,
    )
}

fn proof(
    key: &[u8],
    domain: &[u8],
    version: u16,
    client_nonce: &[u8; NONCE_LENGTH],
    server_nonce: Option<&[u8; NONCE_LENGTH]>,
    certificate: &[u8],
) -> Result<[u8; PROOF_LENGTH]> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| TunnelError::Protocol("初始化证书引导密钥证明失败".into()))?;
    update_proof(
        &mut mac,
        domain,
        version,
        client_nonce,
        server_nonce,
        certificate,
    );
    Ok(mac.finalize().into_bytes().into())
}

fn verify(
    key: &[u8],
    domain: &[u8],
    version: u16,
    client_nonce: &[u8; NONCE_LENGTH],
    server_nonce: Option<&[u8; NONCE_LENGTH]>,
    certificate: &[u8],
    candidate: &[u8; PROOF_LENGTH],
) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(key) else {
        return false;
    };
    update_proof(
        &mut mac,
        domain,
        version,
        client_nonce,
        server_nonce,
        certificate,
    );
    mac.verify_slice(candidate).is_ok()
}

fn update_proof(
    mac: &mut HmacSha256,
    domain: &[u8],
    version: u16,
    client_nonce: &[u8; NONCE_LENGTH],
    server_nonce: Option<&[u8; NONCE_LENGTH]>,
    certificate: &[u8],
) {
    // 证明绑定协议版本、双方随机数和当前 TLS 证书，阻止跨连接转发与重放。
    mac.update(domain);
    mac.update(&version.to_be_bytes());
    mac.update(client_nonce);
    if let Some(server_nonce) = server_nonce {
        mac.update(server_nonce);
    }
    mac.update(&Sha256::digest(certificate));
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"12345678901234567890123456789012";

    #[test]
    fn proofs_bind_key_certificate_and_nonces() {
        let client_nonce = [1_u8; NONCE_LENGTH];
        let server_nonce = [2_u8; NONCE_LENGTH];
        let certificate = b"certificate";
        let client = client_proof(KEY, 4, &client_nonce, certificate).expect("生成客户端证明");
        let server = server_proof(KEY, 4, &client_nonce, &server_nonce, certificate)
            .expect("生成服务端证明");

        assert!(verify_client_proof(
            KEY,
            4,
            &client_nonce,
            certificate,
            &client
        ));
        assert!(verify_server_proof(
            KEY,
            4,
            &client_nonce,
            &server_nonce,
            certificate,
            &server
        ));
        assert!(!verify_client_proof(
            b"wrong-wrong-wrong-wrong-wrong-key!",
            4,
            &client_nonce,
            certificate,
            &client
        ));
        assert!(!verify_server_proof(
            KEY,
            4,
            &client_nonce,
            &server_nonce,
            b"other-certificate",
            &server
        ));
    }
}
