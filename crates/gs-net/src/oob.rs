//! Connectionless (`FF FF FF FF`) packets: what the client sends and the replies it understands
//! (docs/re/net-transport.md §5–6).

use gs_common::info::{InfoString, MAX_PROTINFO};
use gs_common::token;

pub const OOB_PREFIX: [u8; 4] = [0xFF; 4];
pub const PROTOCOL_VERSION: i32 = 48;
/// The connect text plus ticket must fit the original's 2048-byte buffer.
pub const MAX_CONNECT_PACKET: usize = 2048;
pub const MAX_TICKET: usize = 1024;
/// `MSG_ReadStringLine` limit.
const MAX_LINE: usize = 2047;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OobError {
    #[error("connect packet would be {0} bytes (limit 2048)")]
    TooLong(usize),
    #[error("Steam ticket of {0} bytes (must be 1..1024)")]
    BadTicket(usize),
    #[error("protinfo: {0}")]
    Info(#[from] gs_common::info::InfoError),
}

pub fn is_oob(datagram: &[u8]) -> bool {
    datagram.starts_with(&OOB_PREFIX)
}

pub fn getchallenge() -> Vec<u8> {
    let mut out = OOB_PREFIX.to_vec();
    out.extend_from_slice(b"getchallenge steam\n");
    out
}

/// `\prot\3\unique\-1\raw\steam\cdkey\<hash>` for Steam authentication.
pub fn steam_protinfo(cdkey_hash: &str) -> Result<String, OobError> {
    let mut info = InfoString::new(MAX_PROTINFO);
    info.set("prot", "3")?;
    info.set("unique", "-1")?;
    info.set("raw", "steam")?;
    info.set("cdkey", cdkey_hash)?;
    Ok(info.as_str().to_owned())
}

/// `\prot\2\raw\<hash>`: the hashed key of a server that offered auth protocol 2 (HLTV
/// proxies); no ticket follows.
pub fn hashed_protinfo(cdkey_hash: &str) -> Result<String, OobError> {
    let mut info = InfoString::new(MAX_PROTINFO);
    info.set("prot", "2")?;
    info.set("raw", cdkey_hash)?;
    Ok(info.as_str().to_owned())
}

/// `connect 48 <challenge> "<protinfo>" "<userinfo>"\n` followed by the raw Steam ticket
/// (none for protocol 2).
pub fn connect(challenge: i32, protinfo: &str, userinfo: &str, ticket: &[u8]) -> Result<Vec<u8>, OobError> {
    if ticket.len() >= MAX_TICKET {
        return Err(OobError::BadTicket(ticket.len()));
    }
    let text = gs_common::bytestr::encode(&format!("connect {PROTOCOL_VERSION} {challenge} \"{protinfo}\" \"{userinfo}\"\n"));
    let total = OOB_PREFIX.len() + text.len() + ticket.len();
    if total > MAX_CONNECT_PACKET {
        return Err(OobError::TooLong(total));
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&OOB_PREFIX);
    out.extend_from_slice(&text);
    out.extend_from_slice(ticket);
    Ok(out)
}

/// `Netchan_OutOfBandPrint` (rcon): text including its terminating NUL.
pub fn print(text: &str) -> Vec<u8> {
    let mut out = OOB_PREFIX.to_vec();
    out.extend_from_slice(&gs_common::bytestr::encode(text));
    out.push(0);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OobReply {
    /// `A`: challenge for `connect`.
    Challenge { challenge: i32, auth_protocol: u8, server_steam_id: Option<u64>, secure: bool },
    /// `B`: connection accepted; the netchan starts now.
    Accept { userid: i32, true_address: String, build: Option<i32> },
    /// `9`: connection rejected.
    Reject(String),
    /// `8`: wrong server password.
    BadPassword(String),
    /// `l`: print (also rcon output).
    Print(String),
    /// `L`: reconnect elsewhere.
    Redirect(String),
    /// `challenge rcon <n>`.
    RconChallenge(u32),
    /// `I`: an A2S_INFO reply (`pingservers`/`slist`).
    Info(InfoReply),
    /// `G <challenge> <auth> <group>`: a multicast challenge (`listen`).
    Multicast { challenge: i32, auth_protocol: u8, group: String },
    /// A known command whose arguments the original rejects without a print.
    Ignored,
    /// Any other command: `Unknown command:\n%c\n` with argv0's first byte (0 = none).
    Unknown(u8),
}

/// One `I` reply as `CL_ConnectionlessPacket` reads it into an `slist` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfoReply {
    pub name: String,
    pub map: String,
    pub desc: String,
    /// `MSG_ReadByte` values (-1 after a bad read).
    pub players: i32,
    pub maxplayers: i32,
    pub bots: i32,
    pub kind: u8,
    pub os: u8,
    pub password: bool,
    pub vac: bool,
    /// EDF `0x80`: the game port (replaces the sender's).
    pub port: Option<u16>,
    /// EDF `0x40`: the spectator port (the sender's IP).
    pub spectator_port: Option<u16>,
}

/// `MSG_Read*` over a connectionless datagram: past the end a byte or short reads -1 and a
/// string stops, and `msg_badread` is set.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    bad: bool,
}

impl Reader<'_> {
    fn byte(&mut self) -> i32 {
        match self.data.get(self.pos) {
            Some(&b) => {
                self.pos += 1;
                i32::from(b)
            }
            None => {
                self.bad = true;
                -1
            }
        }
    }

    fn short(&mut self) -> i32 {
        match self.data.get(self.pos..self.pos + 2) {
            Some(b) => {
                self.pos += 2;
                i32::from(i16::from_le_bytes([b[0], b[1]]))
            }
            None => {
                self.bad = true;
                -1
            }
        }
    }

    fn long(&mut self) {
        if self.pos + 4 <= self.data.len() {
            self.pos += 4;
        } else {
            self.bad = true;
        }
    }

    /// `MSG_ReadString` (stops at NUL or `0xFF`, both consumed); `Q_strncpy(256)`.
    fn string(&mut self) -> String {
        let mut out = Vec::new();
        loop {
            let c = self.byte();
            if c == -1 || c == 0 || c == 0xFF {
                break;
            }
            if out.len() < 8191 {
                out.push(c as u8);
            }
        }
        out.truncate(255);
        gs_common::bytestr::decode(&out)
    }
}

/// The `I` case: the line after `I` and the protocol byte is the name, then the A2S fields.
fn parse_info(body: &[u8]) -> InfoReply {
    let end = body.iter().position(|&b| matches!(b, b'\n' | 0 | 0xFF)).unwrap_or(body.len()).min(MAX_LINE);
    let name = body.get(2..end).unwrap_or_default();
    let name = gs_common::bytestr::decode(&name[..name.len().min(255)]);
    let mut r = Reader { data: body, pos: (end + 1).min(body.len()), bad: false };
    let mut info = InfoReply { name, ..InfoReply::default() };
    info.map = r.string();
    r.string();
    info.desc = r.string();
    r.short();
    info.players = r.byte();
    info.maxplayers = r.byte();
    info.bots = r.byte();
    info.kind = r.byte() as u8;
    info.os = r.byte() as u8;
    info.password = r.byte() != 0;
    info.vac = r.byte() != 0;
    r.string();
    let edf = r.byte() as u8;
    if !r.bad {
        if edf & 0x80 != 0 {
            info.port = Some(r.short() as u16);
        }
        if edf & 0x10 != 0 {
            r.long();
            r.long();
        }
        if edf & 0x40 != 0 {
            info.spectator_port = Some(r.short() as u16);
            r.string();
        }
        if edf & 0x20 != 0 {
            r.string();
        }
    }
    info
}

/// `MSG_ReadStringLine`: stops at `\n`, NUL, `0xFF` or end; max 2047 bytes.
fn read_string_line(data: &[u8]) -> String {
    let end = data.iter().position(|&b| matches!(b, b'\n' | 0 | 0xFF)).unwrap_or(data.len()).min(MAX_LINE);
    gs_common::bytestr::decode(&data[..end])
}

/// `MSG_ReadString`: stops at NUL or `0xFF`.
fn read_string(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0 || b == 0xFF).unwrap_or(data.len());
    gs_common::bytestr::decode(&data[..end])
}

/// Parses a connectionless datagram (including its prefix) the way `CL_ConnectionlessPacket` does:
/// the switch runs on argv0's first byte; the per-case source and state checks are the caller's.
pub fn parse_reply(datagram: &[u8]) -> Option<OobReply> {
    let body = datagram.strip_prefix(&OOB_PREFIX)?;
    let line = read_string_line(body);
    let args = token::tokenize(&line);
    let argv0 = args.arg(0);
    if argv0.eq_ignore_ascii_case("challenge") {
        let valid = args.argc() == 3 && args.arg(1).eq_ignore_ascii_case("rcon");
        return Some(if valid { OobReply::RconChallenge(q_atoi(args.arg(2)) as u32) } else { OobReply::Ignored });
    }
    let first = argv0.bytes().next().unwrap_or(0);
    let rest = line.get(1..).unwrap_or("");
    Some(match first {
        b'A' => {
            let argc = args.argc();
            if argc != 3 && argc != 5 {
                return Some(OobReply::Ignored);
            }
            let challenge = q_atoi(args.arg(1));
            let auth_protocol = q_atoi(args.arg(2)) as u8;
            if auth_protocol == 3 && argc != 5 {
                return Some(OobReply::Ignored);
            }
            let (server_steam_id, secure) = if argc == 5 {
                (Some(q_atoi64(args.arg(3)) as u64), q_atoi(args.arg(4)) != 0)
            } else {
                (None, false)
            };
            OobReply::Challenge { challenge, auth_protocol, server_steam_id, secure }
        }
        b'B' => OobReply::Accept {
            userid: q_atoi(args.arg(1)),
            true_address: args.arg(2).chars().take(31).collect(),
            build: (args.argc() >= 5).then(|| q_atoi(args.arg(4))),
        },
        b'G' if args.argc() == 4 => {
            OobReply::Multicast { challenge: q_atoi(args.arg(1)), auth_protocol: q_atoi(args.arg(2)) as u8, group: args.arg(3).to_owned() }
        }
        b'G' => OobReply::Ignored,
        b'9' => OobReply::Reject(rest.to_owned()),
        b'8' => {
            let skip = rest.get(..11).is_some_and(|p| p.eq_ignore_ascii_case("BADPASSWORD"));
            OobReply::BadPassword(if skip { rest[11..].to_owned() } else { rest.to_owned() })
        }
        b'l' => OobReply::Print(read_string(body.get(1..).unwrap_or_default())),
        b'L' => OobReply::Redirect(rest.to_owned()),
        b'I' => OobReply::Info(parse_info(body)),
        other => OobReply::Unknown(other),
    })
}

/// `Q_atoi`: optional `-`, `0x` hex, `'c'` character literal, else decimal; wraps like int32.
pub fn q_atoi(s: &str) -> i32 {
    q_atoi64(s) as i32
}

fn q_atoi64(s: &str) -> i64 {
    let b = s.as_bytes();
    let (neg, b) = match b.first() {
        Some(b'-') => (true, &b[1..]),
        _ => (false, b),
    };
    let mut value: i64 = 0;
    if b.len() >= 2 && b[0] == b'0' && (b[1] == b'x' || b[1] == b'X') {
        for &c in &b[2..] {
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => break,
            };
            value = value.wrapping_mul(16).wrapping_add(i64::from(d));
        }
    } else if b.first() == Some(&b'\'') {
        value = b.get(1).map_or(0, |&c| i64::from(c));
    } else {
        for &c in b {
            if !c.is_ascii_digit() {
                break;
            }
            value = value.wrapping_mul(10).wrapping_add(i64::from(c - b'0'));
        }
    }
    if neg { value.wrapping_neg() } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_reply_from_real_hlds() {
        let reply = b"\xff\xff\xff\xffA00000000 98992762 3 72057594037927936 0\n\x00";
        assert_eq!(
            parse_reply(reply),
            Some(OobReply::Challenge {
                challenge: 98992762,
                auth_protocol: 3,
                server_steam_id: Some(72057594037927936),
                secure: false
            })
        );
    }

    #[test]
    fn large_challenge_wraps_to_negative_int32() {
        let reply = b"\xff\xff\xff\xffA00000000 2400000000 2\n\x00";
        assert!(matches!(parse_reply(reply), Some(OobReply::Challenge { challenge: -1894967296, auth_protocol: 2, .. })));
        let packet = connect(-1894967296, "\\prot\\3", "\\name\\x", &[1]).unwrap_or_default();
        assert!(packet.starts_with(b"\xff\xff\xff\xffconnect 48 -1894967296 \"\\prot\\3\" \"\\name\\x\"\n\x01"));
    }

    #[test]
    fn accept_reject_and_print() {
        let accept = b"\xff\xff\xff\xffB 2 \"127.0.0.1:27005\" 0 10210\0";
        assert_eq!(
            parse_reply(accept),
            Some(OobReply::Accept { userid: 2, true_address: "127.0.0.1:27005".into(), build: Some(10210) })
        );
        assert_eq!(parse_reply(b"\xff\xff\xff\xff9Server is full.\n\0"), Some(OobReply::Reject("Server is full.".into())));
        assert_eq!(parse_reply(b"\xff\xff\xff\xfflhello world\n\0\0"), Some(OobReply::Print("hello world\n".into())));
        assert_eq!(parse_reply(b"\xff\xff\xff\xffchallenge rcon 4000000000\n\0"), Some(OobReply::RconChallenge(4000000000)));
    }

    #[test]
    fn steam_protinfo_layout() {
        assert_eq!(
            steam_protinfo("d41d8cd98f00b204e9800998ecf8427e").ok().as_deref(),
            Some("\\prot\\3\\unique\\-1\\raw\\steam\\cdkey\\d41d8cd98f00b204e9800998ecf8427e")
        );
    }

    #[test]
    fn q_atoi_forms() {
        assert_eq!(q_atoi("0x1F"), 31);
        assert_eq!(q_atoi("-12abc"), -12);
        assert_eq!(q_atoi("'A"), 65);
        assert_eq!(q_atoi("4294967295"), -1);
    }
}
