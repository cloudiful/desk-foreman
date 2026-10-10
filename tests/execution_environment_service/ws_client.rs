//! Minimal WebSocket client for the mock runner manager (issue 749 P2b).
//!
//! The relay tunnel is a plain RFC 6455 endpoint, so the mock manager speaks
//! it directly over `TcpStream` rather than pulling in a client crate. Only
//! what a manager needs is implemented: a text/binary handshake, masked
//! client frames, and unmasked server frames with a ping/pong answer.

use std::time::Duration;

use base64less::base64_encode;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

/// One decoded server frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

/// A connected mock-manager tunnel.
pub struct WsClient {
    stream: TcpStream,
}

impl WsClient {
    /// Open the relay tunnel as a manager.
    ///
    /// Returns `Err(status)` when the control plane refuses the handshake,
    /// which is the shape several cases assert on.
    pub async fn connect(base: &str, token: &str, protocol: &str) -> Result<Self, u16> {
        let (host, port) = crate::support::split_base(base);
        let mut stream = TcpStream::connect(format!("{host}:{port}"))
            .await
            .expect("connect to the control plane");
        let key = base64_encode(b"df749p2bmanager16");
        let request = format!(
            "GET /api/internal/runner-manager/service-relay HTTP/1.1\r\n\
             Host: {host}:{port}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Protocol: {protocol}\r\n\
             Authorization: Bearer {token}\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write handshake");
        let mut raw = Vec::new();
        let mut byte = [0u8; 1];
        while !raw.ends_with(b"\r\n\r\n") {
            let read = stream.read(&mut byte).await.expect("read handshake");
            if read == 0 {
                break;
            }
            raw.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&raw).to_string();
        let status: u16 = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        if status != 101 {
            return Err(status);
        }
        Ok(Self { stream })
    }

    pub async fn send_text(&mut self, payload: &str) {
        self.send_frame(0x1, payload.as_bytes()).await;
    }

    /// Send a text frame, reporting whether the peer accepted the write.
    ///
    /// A bounded server closes the connection the moment it sees an
    /// oversized frame, which surfaces here as a reset. That reset is the
    /// refusal, so a probe that asserts on it must not panic inside the
    /// writer.
    pub async fn try_send_text(&mut self, payload: &str) -> std::io::Result<()> {
        self.try_send_frame(0x1, payload.as_bytes()).await
    }

    pub async fn send_binary(&mut self, payload: &[u8]) {
        self.send_frame(0x2, payload).await;
    }

    pub async fn send_close(&mut self) {
        let mut frame = vec![0x88u8, 0x80, 0, 0, 0, 0];
        frame[2..6].copy_from_slice(&[0x37, 0xfa, 0x21, 0x3d]);
        self.stream.write_all(&frame).await.expect("write close");
    }

    async fn send_frame(&mut self, opcode: u8, payload: &[u8]) {
        self.try_send_frame(opcode, payload)
            .await
            .expect("write frame");
    }

    async fn try_send_frame(&mut self, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        let mask = [0x37u8, 0xfa, 0x21, 0x3d];
        let mut frame = vec![0x80 | opcode];
        let length = payload.len();
        if length < 126 {
            frame.push(0x80 | length as u8);
        } else if length <= u16::MAX as usize {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
        self.stream.write_all(&frame).await
    }

    /// Read frames until one the caller asked for arrives.
    ///
    /// Control frames are answered transparently; a stream that only ever
    /// sees heartbeats would otherwise look like a hang.
    pub async fn next_frame(&mut self, within: Duration) -> Result<Frame, &'static str> {
        let deadline = tokio::time::sleep(within);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return Err("timed out waiting for a frame"),
                read = self.read_frame() => {
                    let frame = read.map_err(|_| "tunnel closed while reading a frame")?;
                    match frame {
                        // A bounded peer may reset us while answering a ping;
                        // that is the refusal, not a client failure.
                        Frame::Ping(payload) => {
                            let _ = self.try_send_frame(0xA, &payload).await;
                        }
                        other => return Ok(other),
                    }
                }
            }
        }
    }

    async fn read_frame(&mut self) -> Result<Frame, std::io::Error> {
        let mut header = [0u8; 2];
        self.stream.read_exact(&mut header).await?;
        let opcode = header[0] & 0x0F;
        let masked = header[1] & 0x80 != 0;
        let mut length = (header[1] & 0x7F) as usize;
        if length == 126 {
            let mut extended = [0u8; 2];
            self.stream.read_exact(&mut extended).await?;
            length = u16::from_be_bytes(extended) as usize;
        } else if length == 127 {
            let mut extended = [0u8; 8];
            self.stream.read_exact(&mut extended).await?;
            length = u64::from_be_bytes(extended) as usize;
        }
        let mut mask = [0u8; 4];
        if masked {
            self.stream.read_exact(&mut mask).await?;
        }
        let mut payload = vec![0u8; length];
        self.stream.read_exact(&mut payload).await?;
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        Ok(match opcode {
            0x1 => Frame::Text(String::from_utf8_lossy(&payload).to_string()),
            0x2 => Frame::Binary(payload),
            0x8 => Frame::Close,
            0x9 => Frame::Ping(payload),
            0xA => Frame::Pong(payload),
            other => return Err(std::io::Error::other(format!("opcode {other}"))),
        })
    }
}

/// Base64 for the handshake key. Sixteen bytes, no padding surprises.
mod base64less {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn base64_encode(input: &[u8]) -> String {
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let mut buffer = [0u8; 3];
            buffer[..chunk.len()].copy_from_slice(chunk);
            let triple =
                (u32::from(buffer[0]) << 16) | (u32::from(buffer[1]) << 8) | u32::from(buffer[2]);
            out.push(ALPHABET[(triple >> 18) as usize & 0x3F] as char);
            out.push(ALPHABET[(triple >> 12) as usize & 0x3F] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(triple >> 6) as usize & 0x3F] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[triple as usize & 0x3F] as char
            } else {
                '='
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::base64less::base64_encode;

    #[test]
    fn the_handshake_key_is_valid_base64_of_sixteen_bytes() {
        let key = base64_encode(b"df749p2bmanager16");
        assert_eq!(
            key.len(),
            24,
            "sixteen bytes encode to twenty-four characters"
        );
        assert!(key.ends_with('='), "a whole-number base64 group is padded");
    }
}
