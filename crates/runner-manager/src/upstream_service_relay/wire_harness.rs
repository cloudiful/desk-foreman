#[cfg(test)]
pub(crate) mod harness {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub const ENV_NAME: &str = "DF_AGENT_TOKEN";
    pub const USERNAME: &str = "agent";
    pub const TEST_ENV: &str = "123e4567-e89b-12d3-a456-426614174000";

    /// Loopback agent routing by path. Records hits, the Basic credential
    /// it was offered, and connections dropped mid-reply.
    pub struct Agent {
        pub addr: SocketAddr,
        pub hits: Arc<AtomicUsize>,
        pub seen_auth: Arc<std::sync::Mutex<Option<String>>>,
        pub dropped: Arc<AtomicUsize>,
        _task: tokio::task::JoinHandle<()>,
    }

    pub async fn start_agent() -> Agent {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback agent");
        let addr = listener.local_addr().expect("addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let seen_auth = Arc::new(std::sync::Mutex::new(None));
        let dropped = Arc::new(AtomicUsize::new(0));
        let task = {
            let hits = Arc::clone(&hits);
            let seen_auth = Arc::clone(&seen_auth);
            let dropped = Arc::clone(&dropped);
            tokio::spawn(async move {
                loop {
                    let Ok((socket, _)) = listener.accept().await else {
                        return;
                    };
                    let hits = Arc::clone(&hits);
                    let seen_auth = Arc::clone(&seen_auth);
                    let dropped = Arc::clone(&dropped);
                    tokio::spawn(async move {
                        serve_agent_conn(socket, hits, seen_auth, dropped).await;
                    });
                }
            })
        };
        Agent {
            addr,
            hits,
            seen_auth,
            dropped,
            _task: task,
        }
    }

    async fn serve_agent_conn(
        mut socket: tokio::net::TcpStream,
        hits: Arc<AtomicUsize>,
        seen_auth: Arc<std::sync::Mutex<Option<String>>>,
        dropped: Arc<AtomicUsize>,
    ) {
        hits.fetch_add(1, Ordering::SeqCst);
        let mut raw = vec![0u8; 65536];
        let mut head = Vec::new();
        loop {
            let read = match socket.read(&mut raw).await {
                Ok(0) | Err(_) => return,
                Ok(read) => read,
            };
            head.extend_from_slice(&raw[..read]);
            if head.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let split = head
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap_or(head.len());
        let text = String::from_utf8_lossy(&head[..split]).to_string();
        let path = text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string();
        let auth = text.lines().find_map(|line| {
            line.to_ascii_lowercase()
                .starts_with("authorization:")
                .then(|| line["authorization:".len()..].trim().to_string())
        });
        *seen_auth.lock().expect("auth slot") = auth;
        let content_length: usize = text
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .starts_with("content-length:")
                    .then(|| line["content-length:".len()..].trim().parse().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let mut body = head[split..].to_vec();
        while body.len() < content_length {
            match socket.read(&mut raw).await {
                Ok(0) | Err(_) => return,
                Ok(read) => body.extend_from_slice(&raw[..read]),
            }
        }
        // Returns false when the peer left mid-reply.
        async fn write_all(
            socket: &mut tokio::net::TcpStream,
            bytes: &[u8],
            _dropped: &AtomicUsize,
        ) -> bool {
            socket.write_all(bytes).await.is_ok()
        }
        if path.ends_with("/sse") {
            if !write_all(
                &mut socket,
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n",
                &dropped,
            )
            .await
            {
                dropped.fetch_add(1, Ordering::SeqCst);
                return;
            }
            if !write_all(&mut socket, b"data: one\n\n", &dropped).await {
                dropped.fetch_add(1, Ordering::SeqCst);
                return;
            }
            let _ = socket.flush().await;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if !write_all(&mut socket, b"data: two\n\n", &dropped).await {
                dropped.fetch_add(1, Ordering::SeqCst);
                return;
            }
            let _ = socket.flush().await;
            // Close-delimited body: the manager reads until EOF. Wait
            // briefly for an early close so an abandoned reply observes a
            // disconnect instead of looking cleanly finished.
            let mut probe = [0u8; 1];
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                socket.read(&mut probe),
            )
            .await
            {
                Ok(Ok(0)) | Ok(Err(_)) => {
                    dropped.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        } else if path.ends_with("/binary") {
            let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\n\r\n",
                payload.len()
            );
            if !write_all(&mut socket, head.as_bytes(), &dropped).await
                || !write_all(&mut socket, &payload, &dropped).await
            {
                dropped.fetch_add(1, Ordering::SeqCst);
            }
        } else if path.ends_with("/missing") {
            let _ = write_all(
                &mut socket,
                b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n",
                &dropped,
            )
            .await;
        } else {
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            );
            if !write_all(&mut socket, head.as_bytes(), &dropped).await
                || !write_all(&mut socket, &body, &dropped).await
            {
                dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// Minimal base64 encoder for the expected `Authorization` value.
    pub fn base64_encode(input: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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

    pub fn expected_basic(token: &str) -> String {
        format!(
            "Basic {}",
            base64_encode(format!("{USERNAME}:{token}").as_bytes())
        )
    }
}
