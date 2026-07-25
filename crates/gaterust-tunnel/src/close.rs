use quinn::{ConnectionError, VarInt};

use crate::TunnelError;

/// QUIC 应用层关闭码由客户端和服务端共享，避免同一个数值在两端产生不同语义。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(crate) enum ApplicationCloseCode {
    ServerShutdown = 2,
    AuthenticationFailed = 3,
    CertificateBootstrapComplete = 4,
    ServerConnectionError = 5,
    ClientReconfigure = 10,
    ClientShutdown = 11,
    AdministratorDisconnect = 12,
    CredentialCheckComplete = 13,
    ClientConnectionError = 14,
}

impl ApplicationCloseCode {
    pub(crate) const fn value(self) -> VarInt {
        VarInt::from_u32(self as u32)
    }

    pub(crate) fn matches_error(self, error: &ConnectionError) -> bool {
        matches!(
            error,
            ConnectionError::ApplicationClosed(close) if close.error_code == self.value()
        )
    }
}

/// Tokio `AsyncRead` 会把 QUIC 流错误包装为普通 I/O 错误；连接状态仍保留精确关闭原因。
pub(crate) fn connection_error_or(
    connection: &quinn::Connection,
    fallback: TunnelError,
) -> TunnelError {
    connection
        .close_reason()
        .map_or(fallback, TunnelError::from)
}

#[cfg(test)]
mod tests {
    use quinn::ApplicationClose;

    use super::*;

    #[test]
    fn matches_application_close_code() {
        for code in [
            ApplicationCloseCode::ServerShutdown,
            ApplicationCloseCode::AuthenticationFailed,
            ApplicationCloseCode::CertificateBootstrapComplete,
            ApplicationCloseCode::ServerConnectionError,
            ApplicationCloseCode::ClientReconfigure,
            ApplicationCloseCode::ClientShutdown,
            ApplicationCloseCode::AdministratorDisconnect,
            ApplicationCloseCode::CredentialCheckComplete,
            ApplicationCloseCode::ClientConnectionError,
        ] {
            let error = ConnectionError::ApplicationClosed(ApplicationClose {
                error_code: code.value(),
                reason: "test".into(),
            });
            assert!(code.matches_error(&error));
        }
    }

    #[test]
    fn rejects_transport_and_other_application_codes() {
        assert!(!ApplicationCloseCode::ClientShutdown.matches_error(&ConnectionError::TimedOut));
        let unknown = ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(99),
            reason: "unknown".into(),
        });
        assert!(!ApplicationCloseCode::ClientShutdown.matches_error(&unknown));
    }
}
