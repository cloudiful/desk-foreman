//! Private agent address derivation (issue 749 P2c).
//!
//! The relay never dials an address a caller supplied: the only destination
//! is the environment container's address on the manager-trusted Docker
//! network, read from `docker inspect` under the exact configured network
//! name. Anything else — the daemon's default `IPAddress` guess, another
//! network's address, or a value that is not a private address — is refused
//! rather than dialed, because dialing it would send a per-environment
//! credential somewhere the operator did not approve.

use std::net::IpAddr;

/// Derive the agent address from one `docker inspect` document.
///
/// Reads only `NetworkSettings.Networks[<trusted network>].IPAddress` and
/// `.GlobalIPv6Address`. The top-level `NetworkSettings.IPAddress` is never
/// consulted: it reports the daemon's default network, which is not the
/// manager-connected private network this relay is bound to.
pub(crate) fn private_agent_ip(
    inspect: &serde_json::Value,
    trusted_network: &str,
) -> anyhow::Result<IpAddr> {
    let networks = inspect
        .get("NetworkSettings")
        .and_then(|settings| settings.get("Networks"))
        .and_then(|networks| networks.get(trusted_network));
    let Some(networks) = networks else {
        anyhow::bail!(
            "the environment container is not attached to the trusted service network; refusing to dial"
        );
    };
    let candidates = [
        networks.get("IPAddress").and_then(|value| value.as_str()),
        networks
            .get("GlobalIPv6Address")
            .and_then(|value| value.as_str()),
    ];
    for raw in candidates.into_iter().flatten() {
        let Ok(address) = raw.parse::<IpAddr>() else {
            continue;
        };
        if is_private_agent_ip(&address) {
            return Ok(address);
        }
    }
    anyhow::bail!(
        "the environment container has no private address on the trusted service network; refusing to dial"
    )
}

/// True only for addresses a private agent may hold: IPv4 RFC 1918 or IPv6
/// unique-local. Loopback, link-local, unspecified, and public addresses all
/// fail, because none of them can be the manager-connected private network.
fn is_private_agent_ip(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 10
                || (octets[0] == 172 && (16..32).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 168)
        }
        IpAddr::V6(v6) => (v6.octets()[0] & 0xfe) == 0xfc,
    }
}
