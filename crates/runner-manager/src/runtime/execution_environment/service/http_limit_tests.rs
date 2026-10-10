//! Injected-limit HTTP tests (issue 749 P2c, `#[cfg(test)]`).
//!
//! Loopback agent with scripted stalls, tiny injected bounds: what these
//! prove is that a quiet head, a stalled body, and an oversized body all
//! fail fast with fixed errors instead of hanging or buffering. Production
//! always passes the fixed bounds recorded below.

#[cfg(test)]
mod http_limit_tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;

    use super::super::http::{AgentError, HttpLimits, PROD_LIMITS, send_to_agent_with_limits};
    use super::super::relay::AdmittedService;

    fn limits() -> HttpLimits {
        HttpLimits {
            head: Duration::from_millis(100),
            stream: Duration::from_millis(150),
            max_bytes: 64,
        }
    }

    fn admitted(port: u16, cancel: watch::Receiver<bool>) -> AdmittedService {
        AdmittedService {
            environment_id: "env".to_string(),
            generation: 4,
            target: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            username: "agent".to_string(),
            token: "11111111-2222-4333-8444-555555555555".to_string(),
            cancel,
        }
    }

    /// Agent that reads the head, then behaves per `mode`: `hang` never
    /// answers, `drip` sends one chunk and stalls, `flood` answers 128
    /// bytes at once.
    async fn stalling_agent(mode: &str) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback agent");
        let port = listener.local_addr().expect("addr").port();
        let mode = mode.to_string();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut raw = vec![0u8; 4096];
            let mut head = Vec::new();
            loop {
                match socket.read(&mut raw).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => {
                        head.extend_from_slice(&raw[..read]);
                        if head.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                }
            }
            match mode.as_str() {
                "drip" => {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\r\nchunk")
                        .await
                        .expect("write head");
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                "flood" => {
                    let payload = vec![b'x'; 128];
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\n\r\n",
                        payload.len()
                    );
                    socket.write_all(head.as_bytes()).await.expect("head");
                    socket.write_all(&payload).await.expect("body");
                }
                _ => tokio::time::sleep(Duration::from_secs(30)).await,
            }
        });
        (port, task)
    }

    #[test]
    fn the_production_bounds_stay_fixed() {
        assert_eq!(PROD_LIMITS.head, Duration::from_secs(10));
        assert_eq!(PROD_LIMITS.stream, Duration::from_secs(3600));
        assert_eq!(PROD_LIMITS.max_bytes, 128 * 1024 * 1024);
    }

    #[tokio::test]
    async fn a_quiet_head_fails_fast_as_a_timeout() {
        let (port, agent) = stalling_agent("hang").await;
        let (_cancel_tx, cancel) = watch::channel(false);
        let admitted = admitted(port, cancel);
        let started = std::time::Instant::now();
        assert_eq!(
            send_to_agent_with_limits(
                &admitted,
                "GET",
                "v1/models",
                None,
                &[],
                Vec::new(),
                &admitted.cancel,
                &limits(),
            )
            .await
            .err(),
            Some(AgentError::Timeout)
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "an injected head bound must fail in milliseconds, not production seconds"
        );
        agent.abort();
    }

    #[tokio::test]
    async fn a_stalled_body_fails_fast_as_a_timeout() {
        let (port, agent) = stalling_agent("drip").await;
        let (_cancel_tx, cancel) = watch::channel(false);
        let admitted = admitted(port, cancel);
        let reply = send_to_agent_with_limits(
            &admitted,
            "GET",
            "v1/stream",
            None,
            &[],
            Vec::new(),
            &admitted.cancel,
            &limits(),
        )
        .await
        .expect("the head arrives before the stall");
        let mut body = reply.body;
        assert!(body.next().await.expect("chunk").is_ok());
        assert!(
            body.next().await.expect("terminator").is_err(),
            "a body stalled past the stream bound must fail, not hang"
        );
        agent.abort();
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused_not_buffered() {
        let (port, agent) = stalling_agent("flood").await;
        let (_cancel_tx, cancel) = watch::channel(false);
        let admitted = admitted(port, cancel);
        let reply = send_to_agent_with_limits(
            &admitted,
            "GET",
            "v1/blob",
            None,
            &[],
            Vec::new(),
            &admitted.cancel,
            &limits(),
        )
        .await
        .expect("the head arrives before the flood");
        let mut body = reply.body;
        let mut failed = false;
        while let Some(chunk) = body.next().await {
            if chunk.is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed, "128 bytes over a 64-byte bound must fail");
        agent.abort();
    }
}
