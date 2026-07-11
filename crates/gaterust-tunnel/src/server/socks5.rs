use std::net::{Ipv4Addr, Ipv6Addr};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{Result, TunnelError};

const SOCKS_VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const NO_ACCEPTABLE_METHOD: u8 = 0xff;
const CONNECT: u8 = 1;

pub(super) async fn handshake(stream: &mut TcpStream) -> Result<String> {
    let version = stream.read_u8().await?;
    let method_count = stream.read_u8().await? as usize;
    if version != SOCKS_VERSION || method_count == 0 {
        return Err(TunnelError::Protocol("无效的 SOCKS5 协商请求".into()));
    }
    let mut methods = vec![0; method_count];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&NO_AUTH) {
        stream
            .write_all(&[SOCKS_VERSION, NO_ACCEPTABLE_METHOD])
            .await?;
        return Err(TunnelError::Protocol(
            "SOCKS5 客户端不支持免认证模式".into(),
        ));
    }
    stream.write_all(&[SOCKS_VERSION, NO_AUTH]).await?;

    let version = stream.read_u8().await?;
    let command = stream.read_u8().await?;
    let reserved = stream.read_u8().await?;
    let address_type = stream.read_u8().await?;
    if version != SOCKS_VERSION || command != CONNECT || reserved != 0 {
        send_reply(stream, 7).await?;
        return Err(TunnelError::Protocol("仅支持 SOCKS5 CONNECT 命令".into()));
    }
    let host = match address_type {
        1 => Ipv4Addr::from(stream.read_u32().await?).to_string(),
        3 => {
            let length = stream.read_u8().await? as usize;
            if length == 0 {
                send_reply(stream, 8).await?;
                return Err(TunnelError::Protocol("SOCKS5 域名为空".into()));
            }
            let mut domain = vec![0; length];
            stream.read_exact(&mut domain).await?;
            String::from_utf8(domain)
                .map_err(|_| TunnelError::Protocol("SOCKS5 域名不是 UTF-8".into()))?
        }
        4 => {
            let mut octets = [0; 16];
            stream.read_exact(&mut octets).await?;
            format!("[{}]", Ipv6Addr::from(octets))
        }
        _ => {
            send_reply(stream, 8).await?;
            return Err(TunnelError::Protocol("SOCKS5 地址类型不受支持".into()));
        }
    };
    let port = stream.read_u16().await?;
    Ok(format!("{host}:{port}"))
}

pub(super) async fn send_reply(stream: &mut TcpStream, status: u8) -> Result<()> {
    stream
        .write_all(&[SOCKS_VERSION, status, 0, 1, 0, 0, 0, 0, 0, 0])
        .await?;
    Ok(())
}
