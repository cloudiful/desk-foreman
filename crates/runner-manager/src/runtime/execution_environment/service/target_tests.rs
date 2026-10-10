//! Private-address derivation tests (issue 749 P2c, `#[cfg(test)]`).
//!
//! Pure JSON in, address or refusal out. No Docker, no network: what these
//! prove is that only a private address on exactly the trusted network is
//! ever dialed, and that every other shape fails closed.

#[cfg(test)]
mod target_tests {
    use super::super::target::private_agent_ip;

    fn inspect(networks: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "NetworkSettings": { "Networks": networks } })
    }

    fn network(v4: &str, v6: &str) -> serde_json::Value {
        serde_json::json!({ "IPAddress": v4, "GlobalIPv6Address": v6 })
    }

    #[test]
    fn a_private_ipv4_address_on_the_trusted_network_is_dialed() {
        for address in ["10.0.0.5", "172.16.0.9", "172.31.255.1", "192.168.1.20"] {
            let inspect = inspect(serde_json::json!({ "df-private": network(address, "") }));
            assert_eq!(
                private_agent_ip(&inspect, "df-private")
                    .expect("private IPv4 must serve")
                    .to_string(),
                address
            );
        }
    }

    #[test]
    fn a_unique_local_ipv6_address_serves_when_no_ipv4_is_private() {
        let inspect = inspect(serde_json::json!({ "df-private": network("", "fd00::5") }));
        assert_eq!(
            private_agent_ip(&inspect, "df-private")
                .expect("unique-local IPv6 must serve")
                .to_string(),
            "fd00::5"
        );
    }

    #[test]
    fn a_public_loopback_or_link_local_address_is_never_dialed() {
        for address in [
            "203.0.113.9",
            "8.8.8.8",
            "127.0.0.1",
            "169.254.10.20",
            "::1",
            "fe80::1",
            "2001:db8::1",
            "",
        ] {
            let inspect = inspect(serde_json::json!({ "df-private": network(address, "") }));
            assert!(
                private_agent_ip(&inspect, "df-private").is_err(),
                "{address:?} must be refused"
            );
        }
    }

    #[test]
    fn an_absent_network_or_another_network_never_serves() {
        let missing = inspect(serde_json::json!({ "other": network("10.0.0.5", "") }));
        assert!(
            private_agent_ip(&missing, "df-private").is_err(),
            "a missing trusted network must be refused"
        );
        let empty = inspect(serde_json::json!({}));
        assert!(
            private_agent_ip(&empty, "df-private").is_err(),
            "no networks at all must be refused"
        );
    }

    #[test]
    fn the_daemon_default_address_is_not_a_service_target() {
        // A top-level IPAddress outside Networks must not leak in as the
        // target: it describes the daemon default, not the trusted network.
        let inspect = serde_json::json!({
            "NetworkSettings": {
                "IPAddress": "10.0.0.5",
                "Networks": { "df-private": network("", "") },
            },
        });
        assert!(
            private_agent_ip(&inspect, "df-private").is_err(),
            "the default address guess must never be dialed"
        );
    }
}
