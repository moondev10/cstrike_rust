//! A2S over UDP (docs/re/serverbrowser.md §8.5): the info and player requests, the `A`
//! challenge, and the two info layouts GoldSrc servers send — the Source `I` reply and the
//! old `m` one (many send both).

use std::net::SocketAddrV4;

use gs_gameui::serverbrowser::GameServerItem;

const PREFIX: [u8; 4] = [0xFF; 4];

/// `A2S_INFO`; a server that answered with an `A` challenge wants it appended.
pub fn info_request(challenge: Option<[u8; 4]>) -> Vec<u8> {
    let mut p = Vec::with_capacity(29);
    p.extend_from_slice(&PREFIX);
    p.extend_from_slice(b"TSource Engine Query\0");
    if let Some(c) = challenge {
        p.extend_from_slice(&c);
    }
    p
}

/// `A2S_PLAYER`; the challenge `FF FF FF FF` asks for one.
pub fn players_request(challenge: [u8; 4]) -> Vec<u8> {
    let mut p = Vec::with_capacity(9);
    p.extend_from_slice(&PREFIX);
    p.push(b'U');
    p.extend_from_slice(&challenge);
    p
}

/// One row of an `A2S_PLAYER` reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    pub name: String,
    pub score: i32,
    pub time: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// An info reply as Steam's `gameserveritem_t` (ping and last-played left 0), with the
    /// server type (`d` dedicated, `l` listen, `p` HLTV proxy) and whether it was the `I` layout.
    Info { item: GameServerItem, kind: u8, source: bool },
    /// `A`: repeat the request with this challenge.
    Challenge([u8; 4]),
    Players(Vec<Player>),
}

/// A connectionless reply from `from` (the address queried); `None` for anything else.
pub fn parse(datagram: &[u8], from: SocketAddrV4) -> Option<Reply> {
    let body = datagram.strip_prefix(&PREFIX)?;
    let (&kind, rest) = body.split_first()?;
    let mut r = Reader(rest);
    match kind {
        b'A' => Some(Reply::Challenge(r.take::<4>()?)),
        b'I' => parse_source(&mut r, from),
        b'm' => parse_goldsrc(&mut r, from),
        b'D' => Some(Reply::Players(parse_players(&mut r))),
        _ => None,
    }
}

/// The parts every info layout fills.
fn base_item(from: SocketAddrV4) -> GameServerItem {
    GameServerItem { conn_port: from.port(), query_port: from.port(), ip: u32::from(*from.ip()), had_successful_response: true, ..GameServerItem::default() }
}

/// `I`: protocol, name, map, folder, game, app id, players, max, bots, type, environment,
/// visibility, VAC, version, then the extra-data flags (game port, Steam ID, SourceTV,
/// keywords, game ID).
fn parse_source(r: &mut Reader<'_>, from: SocketAddrV4) -> Option<Reply> {
    let mut item = base_item(from);
    r.byte()?;
    item.server_name = r.text(64)?;
    item.map = r.text(32)?;
    item.game_dir = r.text(32)?;
    item.game_description = r.text(64)?;
    item.app_id = u32::from(r.u16()?);
    item.players = i32::from(r.byte()?);
    item.max_players = i32::from(r.byte()?);
    item.bot_players = i32::from(r.byte()?);
    let kind = r.byte()?.to_ascii_lowercase();
    r.byte()?;
    item.password = r.byte()? != 0;
    item.secure = r.byte()? != 0;
    // The version string and the extra data are absent from some replies.
    if r.text(32).is_some()
        && let Some(edf) = r.byte()
    {
        if edf & 0x80 != 0 {
            item.conn_port = r.u16().unwrap_or(item.conn_port);
        }
        if edf & 0x10 != 0 {
            item.steam_id = r.u64().unwrap_or(0);
        }
        if edf & 0x40 != 0 {
            r.u16();
            r.text(64);
        }
        if edf & 0x20 != 0 {
            item.game_tags = r.text(128).unwrap_or_default();
        }
        if edf & 0x01 != 0
            && let Some(game_id) = r.u64()
        {
            item.app_id = (game_id & 0xff_ffff) as u32;
        }
    }
    Some(Reply::Info { item, kind, source: true })
}

/// `m`: the game address, name, map, folder, game, players, max, protocol, type,
/// environment, visibility, the mod block, VAC and bots. No app ID.
fn parse_goldsrc(r: &mut Reader<'_>, from: SocketAddrV4) -> Option<Reply> {
    let mut item = base_item(from);
    let address = r.text(64)?;
    if let Some(port) = address.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()) {
        item.conn_port = port;
    }
    item.server_name = r.text(64)?;
    item.map = r.text(32)?;
    item.game_dir = r.text(32)?;
    item.game_description = r.text(64)?;
    item.players = i32::from(r.byte()?);
    item.max_players = i32::from(r.byte()?);
    r.byte()?;
    let kind = r.byte()?.to_ascii_lowercase();
    r.byte()?;
    item.password = r.byte()? != 0;
    if r.byte()? == 1 {
        r.text(256)?;
        r.text(256)?;
        r.byte()?;
        r.take::<8>()?;
        r.byte()?;
        r.byte()?;
    }
    item.secure = r.byte()? != 0;
    item.bot_players = i32::from(r.byte()?);
    Some(Reply::Info { item, kind, source: false })
}

/// `D`: the count, then index, name, score and seconds connected per player; a truncated
/// reply keeps the complete rows.
fn parse_players(r: &mut Reader<'_>) -> Vec<Player> {
    let count = r.byte().unwrap_or(0);
    let mut out = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let Some(p) = player(r) else { break };
        out.push(p);
    }
    out
}

fn player(r: &mut Reader<'_>) -> Option<Player> {
    r.byte()?;
    let name = r.text(64)?;
    let score = i32::from_le_bytes(r.take::<4>()?);
    let time = f32::from_le_bytes(r.take::<4>()?);
    Some(Player { name, score, time })
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.0.split_first_chunk::<N>()?;
        self.0 = rest;
        Some(*head)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take::<1>().map(|[b]| b)
    }

    fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }

    /// A NUL-terminated string, cut to the `char[size]` of `gameserveritem_t` (`size − 1`
    /// bytes), decoded like the Steam path.
    fn text(&mut self, size: usize) -> Option<String> {
        let end = self.0.iter().position(|&b| b == 0)?;
        let s = &self.0[..end];
        self.0 = &self.0[end + 1..];
        let mut cut = s.len().min(size - 1);
        // Never split a UTF-8 sequence: back off to its first byte.
        while cut < s.len() && cut > 0 && s[cut] & 0xC0 == 0x80 {
            cut -= 1;
        }
        Some(String::from_utf8_lossy(&s[..cut]).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    const FROM: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 151), 27018);

    fn info(reply: Option<Reply>) -> (GameServerItem, u8, bool) {
        match reply {
            Some(Reply::Info { item, kind, source }) => (item, kind, source),
            other => panic!("not an info reply: {other:?}"),
        }
    }

    /// An HLDS `I` reply with the game port, Steam ID, keywords and game ID: the game ID's
    /// low 24 bits win over the short app ID, the extra-data port over the sender's.
    #[test]
    fn source_info_reads_the_extra_data() {
        let mut d = b"\xff\xff\xff\xffI\x30Alpha&Beta\0de_dust2\0cstrike\0Counter-Strike\0\x0a\x00\x0c\x20\x02dw\x01\x011.1.2.7/Stdio\0".to_vec();
        d.push(0x80 | 0x10 | 0x20 | 0x01);
        d.extend_from_slice(&27015u16.to_le_bytes());
        d.extend_from_slice(&0x0110_0001_0000_0042u64.to_le_bytes());
        d.extend_from_slice(b"ctf,fast\0");
        d.extend_from_slice(&80u64.to_le_bytes());
        let (item, kind, source) = info(parse(&d, FROM));
        assert_eq!((kind, source), (b'd', true));
        assert_eq!((item.ip, item.conn_port, item.query_port), (u32::from(*FROM.ip()), 27015, 27018));
        assert_eq!((item.server_name.as_str(), item.map.as_str(), item.game_dir.as_str(), item.game_description.as_str()), ("Alpha&Beta", "de_dust2", "cstrike", "Counter-Strike"));
        assert_eq!((item.app_id, item.players, item.max_players, item.bot_players), (80, 12, 32, 2));
        assert_eq!((item.password, item.secure, item.steam_id, item.game_tags.as_str()), (true, true, 0x0110_0001_0000_0042, "ctf,fast"));
        assert!(item.had_successful_response);
    }

    /// A reply cut after VAC still lists the server on the sender's port.
    #[test]
    fn source_info_without_extra_data_keeps_the_query_port() {
        let d = b"\xff\xff\xff\xffI\x30n\0m\0cstrike\0g\0\x0a\x00\x01\x10\x00p\x01\x00\x00";
        let (item, kind, _) = info(parse(d, FROM));
        assert_eq!((item.conn_port, item.app_id, kind), (27018, 10, b'p'));
    }

    /// The `m` layout: the game port from the address string, the mod block skipped, bots last.
    #[test]
    fn goldsrc_info_skips_the_mod_block() {
        let mut d = b"\xff\xff\xff\xffm203.0.113.151:27016\0Old\0de_nuke\0cstrike\0CS\0\x05\x10\x2fdw\x00\x01".to_vec();
        d.extend_from_slice(b"http://x\0http://y\0\0");
        d.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0, 1, 0]);
        d.extend_from_slice(&[1, 3]);
        let (item, kind, source) = info(parse(&d, FROM));
        assert_eq!((kind, source, item.conn_port, item.query_port), (b'd', false, 27016, 27018));
        assert_eq!((item.players, item.max_players, item.bot_players, item.secure, item.app_id), (5, 16, 3, true, 0));
    }

    /// Names longer than `char[64]` lose the tail without splitting a UTF-8 character.
    #[test]
    fn long_names_are_cut_at_a_character_boundary() {
        let name = "é".repeat(40);
        let mut d = b"\xff\xff\xff\xffI\x30".to_vec();
        d.extend_from_slice(name.as_bytes());
        d.extend_from_slice(b"\0m\0cstrike\0g\0\x0a\x00\x01\x10\x00dw\x00\x00");
        let (item, ..) = info(parse(&d, FROM));
        assert_eq!(item.server_name, "é".repeat(31));
    }

    #[test]
    fn challenge_and_players() {
        assert_eq!(parse(b"\xff\xff\xff\xffA\x01\x02\x03\x04", FROM), Some(Reply::Challenge([1, 2, 3, 4])));
        let mut d = b"\xff\xff\xff\xffD\x02\x00Lenya\0".to_vec();
        d.extend_from_slice(&7i32.to_le_bytes());
        d.extend_from_slice(&65.5f32.to_le_bytes());
        d.extend_from_slice(b"\x01cut");
        assert_eq!(parse(&d, FROM), Some(Reply::Players(vec![Player { name: "Lenya".into(), score: 7, time: 65.5 }])));
        assert_eq!(parse(b"\xff\xff\xff\xfflbanned\0", FROM), None);
    }
}
