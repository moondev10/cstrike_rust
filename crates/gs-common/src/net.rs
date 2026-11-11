//! Where requests a server can trigger (MOTD pages and what they load, FastDL) may go: a public
//! server must not make the client reach this machine or its local network.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

/// An address only this machine or its local network reaches: loopback, private, link-local,
/// carrier-grade NAT, "this network", unspecified, broadcast and multicast, and the IPv6
/// equivalents (unique-local, link-local, IPv4-mapped).
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() || a == 0 || (a == 100 && b & 0xC0 == 64)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || first & 0xFE00 == 0xFC00 || first & 0xFFC0 == 0xFE80 || v6.to_ipv4_mapped().is_some_and(|v4| is_local(IpAddr::V4(v4)))
        }
    }
}

/// Whether a request a server asked for may go to `host:port`: always when the server itself is
/// local (a LAN game), else only when every address the name resolves to is public. A name that
/// does not resolve is refused. The HTTP client resolves the name again, so a resolver that
/// answers differently the second time is not covered.
pub fn request_allowed(host: &str, port: u16, server_local: bool) -> bool {
    server_local || allowed_addrs(host, port, false).is_some()
}

/// The addresses `host:port` resolves to when a request a server asked for may go there (the
/// rule of [`request_allowed`]), for a client that connects to exactly these: no second lookup
/// can answer differently.
pub fn allowed_addrs(host: &str, port: u16, server_local: bool) -> Option<Vec<SocketAddr>> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().ok()?.collect();
    (!addrs.is_empty() && (server_local || addrs.iter().all(|a| !is_local(a.ip())))).then_some(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_public_addresses() {
        for local in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.1.1", "100.64.0.1", "0.0.0.0", "255.255.255.255", "224.0.0.1", "::1", "fd00::1", "fe80::1", "::ffff:192.168.0.1"] {
            assert!(is_local(local.parse().unwrap()), "{local}");
        }
        for public in ["8.8.8.8", "203.0.113.151", "100.128.0.1", "172.32.0.1", "2001:4860:4860::8888"] {
            assert!(!is_local(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn requests_to_local_hosts_need_a_local_server() {
        assert!(!request_allowed("127.0.0.1", 80, false));
        assert!(!request_allowed("[::1]", 80, false));
        assert!(!request_allowed("localhost", 80, false));
        assert!(request_allowed("127.0.0.1", 80, true));
        assert!(request_allowed("8.8.8.8", 80, false));
    }
}
