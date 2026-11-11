//! The MOTD page's only way out: WebView2 sends every request, loopback included, through this
//! local proxy. It resolves the host itself, refuses this machine and its local network unless
//! the game server is on it (`gs_common::net::allowed_addrs`), and connects to the address it
//! checked, so no redirect, sub-resource or resolver answering differently the second time gets
//! past the check. HTTPS goes through `CONNECT` tunnels (end to end, unread); plain HTTP is
//! forwarded one request per connection.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// Largest request head read.
const MAX_HEAD: usize = 64 * 1024;
/// Connections served at once; more are closed unanswered.
const MAX_CONNECTIONS: usize = 64;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A connection idle this long in one direction is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// The open client connections, by id, to cut when the server stops being local.
type Connections = Arc<Mutex<HashMap<u64, TcpStream>>>;

pub struct Proxy {
    port: u16,
    server_local: Arc<AtomicBool>,
    connections: Connections,
}

impl Proxy {
    /// Listens on an ephemeral loopback port.
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let server_local = Arc::new(AtomicBool::new(false));
        let connections: Connections = Arc::default();
        let (local, open) = (Arc::clone(&server_local), Arc::clone(&connections));
        std::thread::Builder::new().name("motd proxy".into()).spawn(move || {
            let next_id = AtomicU64::new(0);
            for client in listener.incoming().flatten() {
                let id = next_id.fetch_add(1, Ordering::Relaxed);
                {
                    let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
                    let Ok(handle) = client.try_clone() else { continue };
                    if open.len() >= MAX_CONNECTIONS {
                        continue;
                    }
                    open.insert(id, handle);
                }
                let (server_local, conn_open) = (local.load(Ordering::SeqCst), Arc::clone(&open));
                let spawned = std::thread::Builder::new().name("motd proxy connection".into()).spawn(move || {
                    if let Err(e) = serve(client, server_local, port) {
                        log::debug!("MOTD proxy: {e}");
                    }
                    conn_open.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
                });
                if spawned.is_err() {
                    open.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
                }
            }
        })?;
        Ok(Self { port, server_local, connections })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The game server's locality for the connections to come. Leaving a local server cuts the
    /// open ones: a tunnel into the local network must not outlive it.
    pub fn set_server_local(&self, local: bool) {
        if self.server_local.swap(local, Ordering::SeqCst) && !local {
            for stream in self.connections.lock().unwrap_or_else(PoisonError::into_inner).values() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}

/// One client connection: a `CONNECT` tunnel or one absolute-form `http://` request.
fn serve(mut client: TcpStream, server_local: bool, own_port: u16) -> io::Result<()> {
    client.set_read_timeout(Some(IDLE_TIMEOUT))?;
    let (head, body) = read_head(&mut client)?;
    let mut lines = head.split(|&b| b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l));
    let request = std::str::from_utf8(lines.next().unwrap_or_default()).unwrap_or_default();
    let mut parts = request.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return reply(&mut client, "400 Bad Request");
    };
    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = authority(target) else { return reply(&mut client, "400 Bad Request") };
        let Some(mut upstream) = connect(&host, port, server_local, own_port) else { return reply(&mut client, "403 Forbidden") };
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
        upstream.write_all(&body)?;
        return splice(client, upstream);
    }
    // Only a proxy request (`http://host/path`): a page's own request to this port (origin
    // form) is not one.
    let Some(rest) = target.get(..7).filter(|s| s.eq_ignore_ascii_case("http://")).and_then(|_| target.get(7..)) else {
        return reply(&mut client, "400 Bad Request");
    };
    let (auth, path) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let Some((host, port)) = authority(auth) else { return reply(&mut client, "400 Bad Request") };
    let Some(mut upstream) = connect(&host, port, server_local, own_port) else { return reply(&mut client, "403 Forbidden") };
    let path = match path.split('#').next().unwrap_or_default() {
        "" => "/".to_owned(),
        p if p.starts_with('/') => p.to_owned(),
        p => format!("/{p}"),
    };
    let mut out = format!("{method} {path} {version}\r\n").into_bytes();
    for line in lines.filter(|l| !l.is_empty()) {
        let name = line.split(|&b| b == b':').next().unwrap_or_default();
        if [&b"proxy-connection"[..], b"proxy-authorization", b"connection", b"keep-alive"].iter().any(|h| name.trim_ascii().eq_ignore_ascii_case(h)) {
            continue;
        }
        out.extend_from_slice(line);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    upstream.write_all(&out)?;
    upstream.write_all(&body)?;
    splice(client, upstream)
}

/// The request head (without the blank line) and the bytes read past it.
fn read_head(stream: &mut TcpStream) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let from = buf.len().saturating_sub(3);
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf[from..].windows(4).position(|w| w == b"\r\n\r\n") {
            let body = buf.split_off(from + i + 4);
            buf.truncate(from + i);
            return Ok((buf, body));
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "request head too long"));
        }
    }
}

/// `host[:port]` (port 80 by default) of a request target.
fn authority(auth: &str) -> Option<(String, u16)> {
    if auth.contains(['@', '/', '?', '#']) {
        return None;
    }
    let url = url::Url::parse(&format!("http://{auth}/")).ok()?;
    Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
}

/// A connection to the first reachable address `host:port` may go to; never back to this proxy.
fn connect(host: &str, port: u16, server_local: bool, own_port: u16) -> Option<TcpStream> {
    let Some(addrs) = gs_common::net::allowed_addrs(host, port, server_local) else {
        log::warn!("MOTD {host}:{port}: refusing a local or unresolvable address");
        return None;
    };
    addrs.iter().filter(|a| !(a.port() == own_port && gs_common::net::is_local(a.ip()))).find_map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).ok())
}

fn reply(client: &mut TcpStream, status: &str) -> io::Result<()> {
    client.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
}

/// Copies both ways until the server's side ends, then closes both.
fn splice(client: TcpStream, upstream: TcpStream) -> io::Result<()> {
    upstream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    let (mut from_client, mut to_upstream) = (client.try_clone()?, upstream.try_clone()?);
    let up = std::thread::Builder::new().name("motd proxy upload".into()).spawn(move || {
        let _ = io::copy(&mut from_client, &mut to_upstream);
        let _ = to_upstream.shutdown(Shutdown::Write);
    })?;
    let (mut from_upstream, mut to_client) = (upstream, client);
    let _ = io::copy(&mut from_upstream, &mut to_client);
    let _ = to_client.shutdown(Shutdown::Both);
    let _ = from_upstream.shutdown(Shutdown::Both);
    let _ = up.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(proxy: &Proxy, head: &str) -> TcpStream {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port())).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(head.as_bytes()).unwrap();
        s
    }

    fn status(s: &mut TcpStream) -> String {
        let (head, _) = read_head(s).unwrap();
        String::from_utf8(head).unwrap().lines().next().unwrap().to_owned()
    }

    #[test]
    fn this_machine_is_reached_only_for_a_local_server_and_never_the_proxy_itself() {
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = target.local_addr().unwrap().port();
        let proxy = Proxy::start().unwrap();
        let connect = format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        assert_eq!(status(&mut request(&proxy, &connect)), "HTTP/1.1 403 Forbidden");
        assert_eq!(status(&mut request(&proxy, &format!("GET http://localhost:{port}/ HTTP/1.1\r\n\r\n"))), "HTTP/1.1 403 Forbidden");
        proxy.set_server_local(true);
        let mut tunnel = request(&proxy, &connect);
        assert_eq!(status(&mut tunnel), "HTTP/1.1 200 Connection Established");
        let (mut server, _) = target.accept().unwrap();
        tunnel.write_all(b"ping").unwrap();
        let mut got = [0u8; 4];
        server.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"ping");
        let own = format!("CONNECT 127.0.0.1:{0} HTTP/1.1\r\nHost: 127.0.0.1:{0}\r\n\r\n", proxy.port());
        assert_eq!(status(&mut request(&proxy, &own)), "HTTP/1.1 403 Forbidden");
        // Leaving the local server cuts the tunnel.
        proxy.set_server_local(false);
        let cut = match tunnel.read(&mut [0u8; 1]) {
            Ok(n) => n == 0,
            Err(e) => !matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut),
        };
        assert!(cut, "the tunnel outlived the local server");
    }

    #[test]
    fn plain_http_goes_out_in_origin_form_and_closes() {
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = target.local_addr().unwrap().port();
        let proxy = Proxy::start().unwrap();
        proxy.set_server_local(true);
        let mut client = request(&proxy, &format!("GET http://127.0.0.1:{port}/motd.html?id=7#top HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nProxy-Connection: keep-alive\r\nCookie: a=b\r\n\r\n"));
        let (mut server, _) = target.accept().unwrap();
        server.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let (head, _) = read_head(&mut server).unwrap();
        let head = String::from_utf8(head).unwrap();
        assert_eq!(head, format!("GET /motd.html?id=7 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: a=b\r\nConnection: close"));
        server.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").unwrap();
        drop(server);
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert_eq!(response, "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        // A page's own request to the proxy (origin form) is not forwarded.
        assert_eq!(status(&mut request(&proxy, "GET / HTTP/1.1\r\nHost: x\r\n\r\n")), "HTTP/1.1 400 Bad Request");
    }
}
