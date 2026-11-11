//! Non-blocking client UDP socket with split-packet reassembly (docs/re/net-transport.md §3).

use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

use crate::split::{MAX_DATAGRAM, SplitReassembler};

/// The original client binds here first, falling back to an ephemeral port.
pub const DEFAULT_CLIENT_PORT: u16 = 27005;

#[derive(Debug)]
pub struct ClientSocket {
    socket: UdpSocket,
    split: SplitReassembler,
    buf: Vec<u8>,
}

impl ClientSocket {
    /// Binds `port` on all interfaces (0 = ephemeral), falling back to an ephemeral port;
    /// `NET_OpenIP` enables broadcast on every socket (`pingservers`).
    pub fn bind(port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).or_else(|_| UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
        socket.set_nonblocking(true)?;
        socket.set_broadcast(true)?;
        Ok(Self { socket, split: SplitReassembler::default(), buf: vec![0; MAX_DATAGRAM] })
    }

    /// NS_MULTICAST (`UDP_OpenSocket(…, multicast = 1)`): broadcast and `SO_REUSEADDR` set
    /// before binding `port` on all interfaces; `IP_MULTICAST_LOOP` only with `-loopback`.
    pub fn bind_multicast(port: u16, loopback: bool) -> io::Result<Self> {
        use socket2::{Domain, Protocol, Socket, Type};
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_nonblocking(true)?;
        s.set_broadcast(true)?;
        s.set_reuse_address(true)?;
        s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
        if let Err(e) = s.set_multicast_loop_v4(loopback) {
            log::debug!("IP_MULTICAST_LOOP: {e}");
        }
        Ok(Self { socket: s.into(), split: SplitReassembler::default(), buf: vec![0; MAX_DATAGRAM] })
    }

    /// `NET_JoinGroup`: `IP_ADD_MEMBERSHIP` on any interface; `Err` is the WinSock code.
    pub fn join_group(&self, group: Ipv4Addr) -> Result<(), i32> {
        self.socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED).map_err(|e| e.raw_os_error().unwrap_or(0))
    }

    /// `NET_LeaveGroup`: `IP_DROP_MEMBERSHIP`.
    pub fn leave_group(&self, group: Ipv4Addr) -> Result<(), i32> {
        self.socket.leave_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED).map_err(|e| e.raw_os_error().unwrap_or(0))
    }

    /// `NET_FlushSocket`: everything queued is read and dropped.
    pub fn flush(&mut self) {
        while let Ok(Some(_)) = self.recv() {}
    }

    pub fn local_port(&self) -> io::Result<u16> {
        Ok(self.socket.local_addr()?.port())
    }

    /// `net_showpackets 4`: `NET_GetLong`'s split-packet lines on or off.
    pub fn show_splits(&mut self, on: bool) {
        self.split.show = on;
    }

    /// The split-packet lines produced since the last call, oldest first.
    pub fn take_prints(&mut self) -> Vec<String> {
        std::mem::take(&mut self.split.prints)
    }

    pub fn send(&self, to: SocketAddrV4, data: &[u8]) -> io::Result<()> {
        match self.socket.send_to(data, to) {
            Ok(_) => Ok(()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused) => {
                log::debug!("send to {to}: {e}");
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Next complete datagram, or `None` when nothing is pending. Split parts are reassembled;
    /// ICMP-induced resets (Windows reports them on the next receive) are skipped.
    pub fn recv(&mut self) -> io::Result<Option<(SocketAddrV4, Vec<u8>)>> {
        loop {
            let (len, from) = match self.socket.recv_from(&mut self.buf) {
                Ok(r) => r,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(e) if matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused) => continue,
                Err(e) if e.raw_os_error() == Some(10040) => {
                    log::debug!("ignoring oversized datagram");
                    continue;
                }
                Err(e) => return Err(e),
            };
            let SocketAddr::V4(from) = from else { continue };
            if len >= MAX_DATAGRAM {
                log::debug!("oversize packet from {from}");
                continue;
            }
            let data = &self.buf[..len];
            if data.starts_with(&(-2i32).to_le_bytes()) {
                if let Some(full) = self.split.accept(data) {
                    return Ok(Some((from, full)));
                }
                continue;
            }
            return Ok(Some((from, data.to_vec())));
        }
    }
}

/// `WSAEAFNOSUPPORT` (ignored by `NET_JoinGroup`).
pub const WSAEAFNOSUPPORT: i32 = 10047;
/// `WSAENOTSOCK` (a group call on a socket that failed to open).
pub const WSAENOTSOCK: i32 = 10038;

/// `NET_ErrorString`.
pub fn error_string(code: i32) -> &'static str {
    match code {
        10004 => "WSAEINTR",
        10009 => "WSAEBADF",
        10013 => "WSAEACCES",
        10101 => "WSAEDISCON",
        10014 => "WSAEFAULT",
        10022 => "WSAEINVAL",
        10024 => "WSAEMFILE",
        10035 => "WSAEWOULDBLOCK",
        10036 => "WSAEINPROGRESS",
        10037 => "WSAEALREADY",
        10038 => "WSAENOTSOCK",
        10039 => "WSAEDESTADDRREQ",
        10040 => "WSAEMSGSIZE",
        10041 => "WSAEPROTOTYPE",
        10042 => "WSAENOPROTOOPT",
        10043 => "WSAEPROTONOSUPPORT",
        10044 => "WSAESOCKTNOSUPPORT",
        10045 => "WSAEOPNOTSUPP",
        10046 => "WSAEPFNOSUPPORT",
        10047 => "WSAEAFNOSUPPORT",
        10048 => "WSAEADDRINUSE",
        10049 => "WSAEADDRNOTAVAIL",
        10050 => "WSAENETDOWN",
        10051 => "WSAENETUNREACH",
        10052 => "WSAENETRESET",
        10053 => "WSWSAECONNABORTEDAEINTR",
        10054 => "WSAECONNRESET",
        10055 => "WSAENOBUFS",
        10056 => "WSAEISCONN",
        10057 => "WSAENOTCONN",
        10058 => "WSAESHUTDOWN",
        10059 => "WSAETOOMANYREFS",
        10060 => "WSAETIMEDOUT",
        10061 => "WSAECONNREFUSED",
        10062 => "WSAELOOP",
        10063 => "WSAENAMETOOLONG",
        10064 => "WSAEHOSTDOWN",
        10091 => "WSASYSNOTREADY",
        10092 => "WSAVERNOTSUPPORTED",
        10093 => "WSANOTINITIALISED",
        11001 => "WSAHOST_NOT_FOUND",
        11002 => "WSATRY_AGAIN",
        11003 => "WSANO_RECOVERY",
        11004 => "WSANO_DATA",
        _ => "NO ERROR",
    }
}
