//! Agent-hop HTTP tests (issue 749 P2c, `#[cfg(test)]`).
//!
//! Loopback agent, real credential token, real cancel registry. The
//! loopback target is injected by constructing the admitted struct in the
//! test: the production path still derives its target only from inspected
//! private addresses, which never include loopback.

#[cfg(test)]
mod http_tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;

    use super::super::http::{AgentError, send_to_agent};
    use super::super::relay::AdmittedService;

    const USERNAME: &str = "agent";
    const TOKEN: &str = "11111111-2222-4333-8444-555555555555";

    fn admitted(port: u16, cancel: watch::Receiver<bool>) -> AdmittedService {
        AdmittedService {
            environment_id: "env".to_string(),
            generation: 4,
            target: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            username: USERNAME.to_string(),
            token: TOKEN.to_string(),
            cancel,
        }
    }

    fn basic_of(user: &str, token: &str) -> String {
        format!(
            "Basic {}",
            base64_encode(format!("{user}:{token}").as_bytes())
        )
    }

    /// Minimal base64 encoder for the expected `Authorization` value.
    fn base64_encode(input: &[u8]) -> String {
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

    struct Agent {
        addr: std::net::SocketAddr,
        hits: Arc<AtomicUsize>,
        seen_auth: Arc<std::sync::Mutex<Option<String>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    /// Serve one scripted reply: head line, headers, then body slices with
    /// a pause between the first two so streaming is observable.
    async fn scripted_agent(header_lines: Vec<String>, body: Vec<Vec<u8>>) -> Agent {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback agent");
        let addr = listener.local_addr().expect("addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let seen_auth = Arc::new(std::sync::Mutex::new(None));
        let task_hits = Arc::clone(&hits);
        let task_auth = Arc::clone(&seen_auth);
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            task_hits.fetch_add(1, Ordering::SeqCst);
            let mut raw = vec![0u8; 8192];
            let read = socket.read(&mut raw).await.expect("read head");
            let head = String::from_utf8_lossy(&raw[..read]).to_string();
            let auth = head.lines().find_map(|line| {
                line.to_ascii_lowercase()
                    .starts_with("authorization:")
                    .then(|| line["authorization:".len()..].trim().to_string())
            });
            *task_auth.lock().expect("auth slot") = auth;
            // Forbidden caller headers must never arrive with the request.
            assert!(
                !head.to_ascii_lowercase().contains("cookie:"),
                "caller cookies must never cross to the agent"
            );
            let mut reply = String::from("HTTP/1.1 200 OK\r\n");
            for line in &header_lines {
                reply.push_str(line);
                reply.push_str("\r\n");
            }
            reply.push_str("\r\n");
            socket
                .write_all(reply.as_bytes())
                .await
                .expect("write head");
            for (index, slice) in body.iter().enumerate() {
                socket.write_all(slice).await.expect("write chunk");
                socket.flush().await.expect("flush");
                if index == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        });
        Agent {
            addr,
            hits,
            seen_auth,
            _task: task,
        }
    }

    #[tokio::test]
    async fn the_agent_sees_exact_basic_auth_and_only_permitted_headers() {
        let agent = scripted_agent(
            vec!["content-type: application/json".to_string()],
            vec![b"{}".to_vec()],
        )
        .await;
        let (_cancel_tx, cancel) = watch::channel(false);
        let admitted = admitted(agent.addr.port(), cancel);
        let reply = send_to_agent(
            &admitted,
            "POST",
            "v1/chat",
            None,
            &[
                "content-type: application/json".to_string(),
                "authorization: Bearer caller-secret".to_string(),
                "cookie: session=secret".to_string(),
            ],
            b"{}".to_vec(),
            &admitted.cancel,
        )
        .await
        .expect("loopback agent serves");
        assert_eq!(reply.status, 200);
        assert_eq!(
            agent.seen_auth.lock().expect("auth").as_deref(),
            Some(basic_of(USERNAME, TOKEN).as_str()),
            "the agent must see exactly the per-environment credential"
        );
        assert_eq!(agent.hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_sse_body_streams_incrementally_with_safe_headers_only() {
        let agent = scripted_agent(
            vec![
                "content-type: text/event-stream".to_string(),
                "set-cookie: session=secret".to_string(),
            ],
            vec![b"data: one\n\n".to_vec(), b"data: two\n\n".to_vec()],
        )
        .await;
        let (_cancel_tx, cancel) = watch::channel(false);
        let admitted = admitted(agent.addr.port(), cancel);
        let reply = send_to_agent(
            &admitted,
            "GET",
            "v1/stream",
            None,
            &["accept: text/event-stream".to_string()],
            Vec::new(),
            &admitted.cancel,
        )
        .await
        .expect("stream serves");
        assert_eq!(
            reply.headers,
            vec!["content-type: text/event-stream".to_string()],
            "cookies must never travel back to the caller"
        );
        let mut body = reply.body;
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
            .await
            .expect("first chunk streams before the server finishes")
            .expect("chunk")
            .expect("bytes");
        assert_eq!(first, b"data: one\n\n");
        let mut rest = first;
        while let Some(chunk) = body.next().await {
            rest.extend_from_slice(&chunk.expect("bytes"));
        }
        assert_eq!(rest, b"data: one\n\ndata: two\n\n");
    }

    #[tokio::test]
    async fn a_cancel_before_the_first_poll_never_dials() {
        let agent = scripted_agent(vec![], vec![b"late".to_vec()]).await;
        let (cancel_tx, cancel) = watch::channel(false);
        cancel_tx.send(true).expect("cancel first");
        let admitted = admitted(agent.addr.port(), cancel);
        assert_eq!(
            send_to_agent(
                &admitted,
                "GET",
                "v1/models",
                None,
                &[],
                Vec::new(),
                &admitted.cancel,
            )
            .await
            .err(),
            Some(AgentError::Cancelled)
        );
        assert_eq!(
            agent.hits.load(Ordering::SeqCst),
            0,
            "a cancelled stream must never dial"
        );
    }

    /// Independent reviewer regression probe (issue 749 final round 3).
    ///
    /// `target.rs` accepts a unique-local IPv6 address on the trusted network
    /// as a valid agent target, so the hop must actually be able to dial it.
    /// The URL is built by interpolating the address into `http://{ip}:{port}`,
    /// and an IPv6 literal needs bracket delimiters.
    #[tokio::test]
    async fn an_ipv6_agent_target_is_dialable() {
        let listener = match tokio::net::TcpListener::bind("[::1]:0").await {
            Ok(listener) => listener,
            // No IPv6 loopback on this host: nothing to prove.
            Err(_) => return,
        };
        let port = listener.local_addr().expect("addr").port();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut raw = vec![0u8; 8192];
            let _ = socket.read(&mut raw).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                .await
                .expect("write");
        });
        let (_cancel_tx, cancel) = watch::channel(false);
        let admitted = AdmittedService {
            environment_id: "env".to_string(),
            generation: 4,
            target: IpAddr::V6("::1".parse().expect("v6 loopback")),
            port,
            username: USERNAME.to_string(),
            token: TOKEN.to_string(),
            cancel,
        };
        let outcome = send_to_agent(
            &admitted,
            "GET",
            "v1/models",
            None,
            &[],
            Vec::new(),
            &admitted.cancel,
        )
        .await;
        task.abort();
        assert!(
            outcome.is_ok(),
            "an accepted IPv6 target must be dialable, got {:?}",
            outcome.err()
        );
    }

    #[test]
    fn the_admitted_struct_never_debugs_its_token() {
        let (_cancel_tx, cancel) = watch::channel(false);
        let rendered = format!("{:?}", admitted(8443, cancel));
        assert!(
            !rendered.contains(TOKEN),
            "a debug format must never carry the credential"
        );
    }
}
