//! Connect-packet authentication (docs/re/net-transport.md §6–7).

use std::net::Ipv4Addr;

use md5::{Digest, Md5};

/// Supplies the Steam ticket appended to the `connect` packet.
pub trait Authenticator {
    /// `ISteamUser::InitiateGameConnection` equivalent for the server that sent the challenge.
    fn ticket(&mut self, server_steam_id: u64, ip: Ipv4Addr, port: u16, secure: bool) -> Result<Vec<u8>, String>;
    /// Called on disconnect (`TerminateGameConnection`).
    fn end_session(&mut self) {}
    /// Once per host frame (`SteamAPI_RunCallbacks`).
    fn run_frame(&mut self) {}
}

/// Ticket for servers with `sv_lan 1`, which tolerate failed Steam validation: the bytes are
/// never accepted by an internet server.
#[derive(Debug, Default)]
pub struct LanAuthenticator;

impl Authenticator for LanAuthenticator {
    fn ticket(&mut self, _: u64, _: Ipv4Addr, _: u16, _: bool) -> Result<Vec<u8>, String> {
        Ok(vec![0; 4])
    }
}

/// `CL_GetCDKeyHash`: lowercase hex MD5 of the local IPv4 address string (the server only checks
/// that it is 32 characters).
pub fn cdkey_hash(local_ip: Ipv4Addr) -> String {
    let o = local_ip.octets();
    let digest = Md5::digest(format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]).as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn cdkey_hash_is_32_lowercase_hex() {
        let h = super::cdkey_hash(std::net::Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(h, "f528764d624db129b32c21fbca0cb8d6");
    }
}
