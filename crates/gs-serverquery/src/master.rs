//! The master-server query (`0x31`, the HL1/HL2 master protocol Valve retired) and
//! `platform/config/MasterServers.vdf`, the list of community masters that still speak it.

use std::net::{Ipv4Addr, SocketAddrV4};

use gs_gameui::Fs;
use gs_vgui::keyvalues::KeyValues;

/// Masters answering `\gamedir\cstrike` when this was written; together about 7 900
/// addresses, 6 700 of them answering A2S_INFO.
pub const DEFAULT_MASTERS: [&str; 4] = ["ms.tsarvar.com:27010", "ds-servers.com:27010", "cs.hlmasterserver.net:27010", "ms.cs16.net:27010"];

const FILE: &str = "MasterServers.vdf";

/// `31 FF <seed "a.b.c.d:port"> 00 <filter> 00`: all regions, the page after `seed`
/// (`0.0.0.0:0` for the first).
pub fn request(seed: SocketAddrV4, filter: &str) -> Vec<u8> {
    let mut p = vec![0x31, 0xFF];
    p.extend_from_slice(seed.to_string().as_bytes());
    p.push(0);
    p.extend_from_slice(filter.as_bytes());
    p.push(0);
    p
}

/// `FF FF FF FF 66 0A` and big-endian `ip, port` pairs; `0.0.0.0:0` ends the list.
pub fn parse(datagram: &[u8]) -> Option<Vec<SocketAddrV4>> {
    let body = datagram.strip_prefix(&[0xFF, 0xFF, 0xFF, 0xFF, 0x66, 0x0A])?;
    Some(body.chunks_exact(6).map(|c| SocketAddrV4::new(Ipv4Addr::new(c[0], c[1], c[2], c[3]), u16::from_be_bytes([c[4], c[5]]))).collect())
}

/// The page's Steam filters (§6.2: `gamedir`, `empty`, `full`, `secure`, `proxy`) as a
/// master filter, `\key\value…`.
pub fn filter(filters: &[(String, String)]) -> String {
    filters.iter().map(|(k, v)| format!("\\{k}\\{v}")).collect()
}

/// The `hl1` masters of `MasterServers.vdf` (`CONFIG`). A missing file is written with the
/// defaults so it can be edited.
pub fn load(fs: &Fs) -> Vec<String> {
    let Some(data) = fs.read_in(FILE, "CONFIG") else {
        let mut kv = KeyValues::new("MasterServers");
        for (i, addr) in DEFAULT_MASTERS.iter().enumerate() {
            kv.set_string(&format!("hl1/{i}/addr"), addr);
        }
        if !fs.write_in(FILE, "CONFIG", &kv.save_to_buffer()) {
            log::warn!("server browser: {FILE} not written");
        }
        return DEFAULT_MASTERS.iter().map(|s| (*s).to_owned()).collect();
    };
    let mut kv = KeyValues::new("MasterServers");
    kv.load_from_buffer(&data);
    let list: Vec<String> = kv.find_key("hl1").map(|hl1| hl1.children.iter().map(|m| m.get_string("addr", "")).filter(|a| !a.is_empty()).collect()).unwrap_or_default();
    if list.is_empty() {
        log::warn!("server browser: no masters in {FILE}");
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_reply() {
        let seed = SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 27015);
        assert_eq!(request(seed, "\\gamedir\\cstrike"), b"\x31\xff1.2.3.4:27015\0\\gamedir\\cstrike\0");
        let reply = b"\xff\xff\xff\xff\x66\x0a\xcb\x00\x71\x97\x69\x92\x00\x00\x00\x00\x00\x00";
        assert_eq!(parse(reply), Some(vec![SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 151), 27026), SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)]));
        assert_eq!(parse(b"\xff\xff\xff\xffI"), None);
        assert_eq!(filter(&[("gamedir".into(), "cstrike".into()), ("proxy".into(), "1".into())]), "\\gamedir\\cstrike\\proxy\\1");
    }
}
