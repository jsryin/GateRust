use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Result, TunnelError, config::TunnelKind};

pub(crate) const PROTOCOL_VERSION: u16 = 2;
pub(crate) const MAX_CONTROL_FRAME: usize = 64 * 1024;
pub(crate) const MAX_DATAGRAM: usize = u16::MAX as usize;
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(serde::Deserialize, Serialize)]
pub(crate) struct ClientHello {
    pub version: u16,
    pub device_id: String,
    pub key: Vec<u8>,
    pub services: Vec<ServiceDeclaration>,
}

#[derive(Clone, Copy, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthenticationStatus {
    Accepted,
    Rejected,
    DeviceIdConflict,
    ServerBusy,
}

#[derive(serde::Deserialize, Serialize)]
pub(crate) struct ServerHello {
    pub status: AuthenticationStatus,
    pub message: String,
}

#[derive(serde::Deserialize, Serialize)]
pub(crate) enum ControlMessage {
    UpdateServices(Vec<ServiceDeclaration>),
}

#[derive(Clone, serde::Deserialize, Serialize)]
pub(crate) struct ServiceDeclaration {
    pub name: String,
    pub kind: TunnelKind,
}

pub(crate) fn validate_declarations(services: &[ServiceDeclaration]) -> Result<()> {
    if services.len() > 256 {
        return Err(TunnelError::Protocol(
            "单个客户端最多声明 256 个服务".into(),
        ));
    }
    let mut names = std::collections::HashSet::with_capacity(services.len());
    for service in services {
        if service.name.is_empty()
            || service.name.len() > 64
            || !service
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !names.insert(service.name.as_str())
        {
            return Err(TunnelError::Protocol("服务声明名称无效或重复".into()));
        }
    }
    Ok(())
}

#[derive(serde::Deserialize, Serialize)]
pub(crate) struct OpenRequest {
    pub service: String,
    pub destination: Option<String>,
}

#[derive(serde::Deserialize, Serialize)]
pub(crate) struct OpenResponse {
    pub accepted: bool,
    pub message: String,
}

pub(crate) async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value)
        .map_err(|error| TunnelError::Protocol(format!("序列化控制帧失败: {error}")))?;
    if payload.len() > MAX_CONTROL_FRAME {
        return Err(TunnelError::Protocol("控制帧超过 64 KiB".into()));
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| TunnelError::Protocol("控制帧长度溢出".into()))?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    Ok(())
}

pub(crate) async fn read_frame<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = reader.read_u32().await? as usize;
    if length > MAX_CONTROL_FRAME {
        return Err(TunnelError::Protocol("控制帧超过 64 KiB".into()));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload)
        .map_err(|error| TunnelError::Protocol(format!("解析控制帧失败: {error}")))
}

pub(crate) async fn write_datagram<W>(writer: &mut W, payload: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let length = u16::try_from(payload.len())
        .map_err(|_| TunnelError::Protocol("UDP 数据报超过 65535 字节".into()))?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    Ok(())
}

pub(crate) async fn read_datagram<R>(reader: &mut R, buffer: &mut Vec<u8>) -> Result<usize>
where
    R: AsyncRead + Unpin,
{
    let length = reader.read_u16().await? as usize;
    if length > MAX_DATAGRAM {
        return Err(TunnelError::Protocol("UDP 数据报长度无效".into()));
    }
    buffer.resize(length, 0);
    reader.read_exact(buffer).await?;
    Ok(length)
}
