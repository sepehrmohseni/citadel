use std::net::{Ipv4Addr, Ipv6Addr};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Result, TransportError};

const VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const NO_ACCEPTABLE: u8 = 0xff;
const CMD_CONNECT: u8 = 1;
const ATYP_V4: u8 = 1;
const ATYP_DOMAIN: u8 = 3;
const ATYP_V6: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    Succeeded = 0,
    GeneralFailure = 1,
    HostUnreachable = 4,
    CommandNotSupported = 7,
    AddressNotSupported = 8,
}

pub(crate) async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    io: &mut S,
) -> Result<(String, u16)> {
    let mut head = [0u8; 2];
    io.read_exact(&mut head).await?;
    if head[0] != VERSION {
        return Err(TransportError::Socks("unsupported protocol version"));
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    io.read_exact(&mut methods).await?;
    if !methods.contains(&NO_AUTH) {
        io.write_all(&[VERSION, NO_ACCEPTABLE]).await?;
        return Err(TransportError::Socks("client requires authentication"));
    }
    io.write_all(&[VERSION, NO_AUTH]).await?;

    let mut req = [0u8; 4];
    io.read_exact(&mut req).await?;
    if req[0] != VERSION {
        return Err(TransportError::Socks("malformed request"));
    }
    if req[1] != CMD_CONNECT {
        reply(io, Reply::CommandNotSupported).await?;
        return Err(TransportError::Socks("only CONNECT is supported"));
    }
    let host = match req[3] {
        ATYP_V4 => {
            let mut a = [0u8; 4];
            io.read_exact(&mut a).await?;
            Ipv4Addr::from(a).to_string()
        }
        ATYP_V6 => {
            let mut a = [0u8; 16];
            io.read_exact(&mut a).await?;
            Ipv6Addr::from(a).to_string()
        }
        ATYP_DOMAIN => {
            let len = io.read_u8().await?;
            let mut name = vec![0u8; usize::from(len)];
            io.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| TransportError::Socks("domain is not UTF-8"))?
        }
        _ => {
            reply(io, Reply::AddressNotSupported).await?;
            return Err(TransportError::Socks("unsupported address type"));
        }
    };
    let port = io.read_u16().await?;
    if host.is_empty() || port == 0 {
        reply(io, Reply::GeneralFailure).await?;
        return Err(TransportError::Socks("empty destination"));
    }
    Ok((host, port))
}

pub(crate) async fn reply<S: AsyncWrite + Unpin>(io: &mut S, code: Reply) -> Result<()> {
    io.write_all(&[VERSION, code as u8, 0, ATYP_V4, 0, 0, 0, 0, 0, 0])
        .await?;
    io.flush().await?;
    Ok(())
}
