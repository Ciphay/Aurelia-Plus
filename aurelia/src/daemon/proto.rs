use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

pub const C_HEADER: u8 = 0x01;
pub const C_STDIN: u8 = 0x02;
pub const C_STDIN_EOF: u8 = 0x03;
pub const C_STDOUT: u8 = 0x11;
pub const C_STDERR: u8 = 0x12;
pub const C_EXIT: u8 = 0x13;

pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    channel: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "frame payload exceeds u32",
        )
    })?;
    w.write_u8(channel).await?;
    w.write_u32(len).await?;
    if !payload.is_empty() {
        w.write_all(payload).await?;
    }
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let channel = match r.read_u8().await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    };
    let len = r.read_u32().await?;
    if len > MAX_FRAME_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame length exceeds maximum",
        ));
    }
    let len = len as usize;
    let mut buf = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut buf).await?;
    }
    Ok(Some((channel, buf)))
}

pub type SharedWriter<W> = std::sync::Arc<tokio::sync::Mutex<W>>;

pub fn split_shared<S>(
    stream: S,
) -> (
    tokio::io::ReadHalf<S>,
    SharedWriter<tokio::io::WriteHalf<S>>,
)
where
    S: AsyncRead + AsyncWrite,
{
    let (reader, writer) = tokio::io::split(stream);
    (reader, std::sync::Arc::new(tokio::sync::Mutex::new(writer)))
}

pub async fn send_frame<W: AsyncWrite + Unpin>(
    writer: &SharedWriter<W>,
    channel: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let mut w = writer.lock().await;
    write_frame(&mut *w, channel, payload).await
}
