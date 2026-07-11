use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Result, rate_limit::RateLimiter};

const BUFFER_SIZE: usize = 16 * 1024;

pub(crate) struct QuinnStream(pub quinn::SendStream, pub quinn::RecvStream);

impl tokio::io::AsyncRead for QuinnStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncRead::poll_read(std::pin::Pin::new(&mut self.1), context, buffer)
    }
}

impl tokio::io::AsyncWrite for QuinnStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        tokio::io::AsyncWrite::poll_write(std::pin::Pin::new(&mut self.0), context, buffer)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_flush(std::pin::Pin::new(&mut self.0), context)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_shutdown(std::pin::Pin::new(&mut self.0), context)
    }
}

pub(crate) async fn copy_bidirectional<A, B>(
    left: &mut A,
    right: &mut B,
    limiter: &RateLimiter,
) -> Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (left_read, left_write) = tokio::io::split(left);
    let (right_read, right_write) = tokio::io::split(right);
    tokio::try_join!(
        copy_one_way(left_read, right_write, limiter.clone()),
        copy_one_way(right_read, left_write, limiter.clone()),
    )?;
    Ok(())
}

async fn copy_one_way<R, W>(mut reader: R, mut writer: W, limiter: RateLimiter) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0; BUFFER_SIZE];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            writer.shutdown().await?;
            return Ok(());
        }
        limiter.acquire(read).await;
        writer.write_all(&buffer[..read]).await?;
    }
}
