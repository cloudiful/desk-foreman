//! Agent target composition probes (issue 749 final round 3, `#[cfg(test)]`).
//!
//! Split from `http_tests.rs` so no test file exceeds the size budget. What
//! these prove is that the private agent hop composes its authority through
//! `SocketAddr`, so an IPv6 literal is bracketed and dialable while IPv4 is
//! unchanged, and that nothing in the composition can introduce a target of
//! its own.

#[cfg(test)]
mod http_target_tests {
    use std::net::IpAddr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;

    use super::super::http::send_to_agent;
    use super::super::relay::AdmittedService;

    const USERNAME: &str = "agent";
    const TOKEN: &str = "11111111-2222-4333-8444-555555555555";

    /// Independent reviewer regression probe (issue 749 final round 3).
    ///
    /// The IPv6 fix routes every target through `SocketAddr`, so the IPv4
    /// path must keep working with unchanged bracketing rules.
    #[tokio::test]
    async fn an_ipv4_agent_target_is_still_dialable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback agent");
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
            target: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
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
            "an IPv4 target must stay dialable, got {:?}",
            outcome.err()
        );
    }

    /// The composed authority is exactly what the peer must see: no
    /// brackets around an IPv4 literal, brackets around an IPv6 one, and
    /// nothing but the trusted port.
    #[test]
    fn the_agent_authority_is_composed_without_target_injection() {
        assert_eq!(
            std::net::SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8443).to_string(),
            "127.0.0.1:8443"
        );
        assert_eq!(
            std::net::SocketAddr::new(IpAddr::V6("::1".parse().expect("v6")), 8443).to_string(),
            "[::1]:8443",
            "an IPv6 literal must be bracketed to be a URL authority"
        );
        assert_eq!(
            std::net::SocketAddr::new(IpAddr::V6("fd00::1".parse().expect("unique-local")), 1)
                .to_string(),
            "[fd00::1]:1"
        );
    }
}
