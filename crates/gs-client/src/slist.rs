//! The LAN server list of `pingservers` / `slist` / `list` (docs/re/commands-client.md §9-§11):
//! A2S_INFO broadcasts and the 16-entry list built from the `I` replies.

use std::net::SocketAddrV4;

use gs_net::oob::InfoReply;

/// `slist_entries[]` capacity.
pub const MAX_ENTRIES: usize = 16;

/// One `slist_entries[]` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub adr: SocketAddrV4,
    pub info: InfoReply,
    /// The spectator address (the sender's IP with the EDF spectator port).
    pub spectator: Option<SocketAddrV4>,
}

impl Entry {
    /// The record `CL_ConnectionlessPacket` fills from a reply sent by `from`.
    pub fn from_reply(from: SocketAddrV4, info: InfoReply) -> Self {
        let adr = info.port.map_or(from, |p| SocketAddrV4::new(*from.ip(), p));
        let spectator = info.spectator_port.map(|p| SocketAddrV4::new(*from.ip(), p));
        Self { adr, info, spectator }
    }

    /// The block `list` and the reply arrival print for entry number `n` (1-based).
    pub fn text(&self, n: usize) -> String {
        let e = &self.info;
        let kind = match e.kind {
            b'd' => "Dedicated",
            b'p' => "Proxy",
            b'l' => "Listen",
            _ => "Unknown",
        };
        let os = match e.os {
            b'w' => "Windows",
            b'l' => "Linux",
            _ => "macOS",
        };
        let mut s = format!(
            "------------------\n{n} Name: {}\nAddress: {}\nMap: {}\nDescription: {}\nPlayers: {}/{}   Bots: {}\n{kind} - {os} - {} - {}\n",
            e.name,
            self.adr,
            e.map,
            e.desc,
            e.players,
            e.maxplayers,
            e.bots,
            if e.password { "Private" } else { "Public" },
            if e.vac { "Secure" } else { "Insecure" }
        );
        if let Some(spec) = self.spectator {
            s.push_str(&format!("Spectator Address: {spec}\n"));
        }
        s
    }
}

/// `cls.slist_time` and the entries.
#[derive(Debug, Clone, Default)]
pub struct ServerList {
    pub time: f32,
    pub entries: Vec<Entry>,
}

impl ServerList {
    /// `CL_AddToServerList`: within the `cl_slist` window and below 16 entries a new address is
    /// appended; returns its print.
    pub fn add(&mut self, entry: Entry, realtime: f64, window: f32) -> Option<String> {
        if self.entries.len() >= MAX_ENTRIES || (window > 0.0 && realtime as f32 - self.time >= window) {
            return None;
        }
        if self.entries.iter().any(|e| e.adr == entry.adr) {
            return None;
        }
        self.entries.push(entry);
        self.entries.last().map(|e| e.text(self.entries.len()))
    }

    /// `cmd_list`.
    pub fn text(&self) -> String {
        if self.entries.is_empty() {
            return "No local servers in list.\nTry 'slist' to search again.\n".to_owned();
        }
        self.entries.iter().enumerate().map(|(i, e)| e.text(i + 1)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn reply(port: Option<u16>) -> InfoReply {
        InfoReply { name: "srv".into(), kind: b'd', os: b'w', port, ..InfoReply::default() }
    }

    #[test]
    fn window_capacity_and_duplicates_follow_cl_add_to_server_list() {
        let from = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 2), 27015);
        let mut list = ServerList { time: 100.0, entries: Vec::new() };
        assert!(list.add(Entry::from_reply(from, reply(None)), 111.0, 10.0).is_none(), "outside the cl_slist window");
        let first = list.add(Entry::from_reply(from, reply(None)), 105.0, 10.0);
        assert!(first.is_some_and(|t| t.starts_with("------------------\n1 Name: srv\nAddress: 10.0.0.2:27015\n")));
        assert!(list.add(Entry::from_reply(from, reply(None)), 105.0, 10.0).is_none(), "same address");
        // The EDF game port replaces the sender's port, so this is a second server.
        assert!(list.add(Entry::from_reply(from, reply(Some(27016))), 105.0, 0.0).is_some());
        for i in 0..20u16 {
            list.add(Entry::from_reply(SocketAddrV4::new(Ipv4Addr::new(10, 0, 1, 1), 1000 + i), reply(None)), 105.0, 0.0);
        }
        assert_eq!(list.entries.len(), MAX_ENTRIES);
    }
}
