//! Frames on the link stream: a 4-byte big-endian length, then the bytes of one JSON-RPC
//! message.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The longest accepted frame (8 MB, as on the WebSocket link).
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Read one frame. `None`: the peer finished the stream between two frames.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < head.len() {
        match r.read(&mut head[got..]).await? {
            0 if got == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => got += n,
        }
    }
    let len = u32::from_be_bytes(head) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

/// Write one frame.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut buf = Vec::with_capacity(payload.len() + 4);
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"{\"a\":1}").await.unwrap();
        write_frame(&mut wire, b"").await.unwrap();
        assert_eq!(&wire[..4], &[0, 0, 0, 7]);
        let mut r = &wire[..];
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(read_frame(&mut r).await.unwrap().unwrap(), b"");
        assert!(read_frame(&mut r).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_cut_frame_is_an_error() {
        let wire = [0u8, 0, 0, 9, b'x'];
        assert!(read_frame(&mut &wire[..]).await.is_err());
        let huge = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert!(read_frame(&mut &huge[..]).await.is_err());
    }
}
