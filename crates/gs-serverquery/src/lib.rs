//! The server browser's `ISteamMatchmakingServers` / `ISteamMatchmaking` without a Steam client
//! (docs/re/serverbrowser.md §8.5). Valve's HL1 master servers are gone, so the Internet and
//! Spectate lists come from the community masters of `MasterServers.vdf` (same `0x31`
//! protocol); every server is then asked A2S_INFO from one non-blocking socket polled by
//! `run_callbacks`. LAN is an A2S_INFO broadcast; favorites and history live in Steam's file
//! layout under `platform/config`.

pub mod a2s;
mod favorites;
pub mod master;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::ops::RangeInclusive;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use gs_gameui::Fs;
use gs_gameui::serverbrowser::{GameServerItem, ListKind, ListRequest, Matchmaking, MmEvent, Responder, ServerQuery};

use a2s::Reply;
use favorites::{Entry, FAVORITE, HISTORY, Store};

/// A2S_INFO requests per second over every list (about 60 KB/s of replies).
const SEND_RATE: f64 = 400.0;
/// The most a long frame may send at once.
const MAX_BURST: f64 = 64.0;
/// A server, ping or player query without an answer after this has failed.
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
/// `A` replies answered per query before it is left to time out.
const MAX_CHALLENGES: u8 = 2;
/// A master page without an answer after this is asked again, up to [`MASTER_TRIES`] times.
const MASTER_TIMEOUT: Duration = Duration::from_secs(3);
const MASTER_TRIES: u32 = 3;
/// A master still paging after this many replies is cut off.
const MASTER_MAX_PAGES: u32 = 100;
/// How long LAN replies are collected after the broadcast.
const LAN_WINDOW: Duration = Duration::from_millis(1500);
/// The engine's `pingservers` ports (commands-client.md §9).
const LAN_PORTS: RangeInclusive<u16> = 27015..=27024;
/// `EMatchMakingServerResponse`.
const RESPONDED: i32 = 0;
const FAILED: i32 = 1;
const NO_SERVERS_LISTED: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Queued,
    Sent { at: Instant, challenges: u8 },
    Done,
}

struct Slot {
    item: GameServerItem,
    state: State,
    /// Filled from an `m` reply; a later `I` from the same server adds the app and Steam IDs.
    goldsrc: bool,
    /// Answered as something Steam would not have listed ([`List::shows`]).
    hidden: bool,
}

impl Slot {
    fn addr(&self) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::from(self.item.ip), self.item.query_port)
    }
}

/// One master of an Internet or Spectate request.
#[derive(Default)]
struct MasterFetch {
    addr: Option<SocketAddrV4>,
    /// The last address of the previous page.
    seed: Option<SocketAddrV4>,
    /// Every address this master listed, to see a page that brings nothing new.
    seen: HashSet<SocketAddrV4>,
    sent: Option<Instant>,
    tries: u32,
    pages: u32,
    done: bool,
}

/// One `HServerListRequest`.
struct List {
    kind: ListKind,
    responder: Responder,
    app: u32,
    servers: Vec<Slot>,
    index: HashMap<SocketAddrV4, usize>,
    /// Queued or sent slots.
    pending: usize,
    /// No slot before this is queued.
    cursor: usize,
    /// Sent slots in send order (all share one timeout, so the front is the oldest).
    in_flight: VecDeque<(usize, Instant)>,
    filter: String,
    masters: Vec<MasterFetch>,
    resolver: Option<Receiver<Vec<Option<SocketAddrV4>>>>,
    /// LAN: when the broadcast went out.
    broadcast: Option<Instant>,
    refreshing: bool,
    master_replied: bool,
}

impl List {
    fn new(kind: ListKind, responder: Responder, app: u32) -> Self {
        Self { kind, responder, app, servers: Vec::new(), index: HashMap::new(), pending: 0, cursor: 0, in_flight: VecDeque::new(), filter: String::new(), masters: Vec::new(), resolver: None, broadcast: None, refreshing: true, master_replied: false }
    }

    /// A new address, queued; `false` when listed already.
    fn add(&mut self, addr: SocketAddrV4, time_last_played: u32) -> bool {
        if self.index.contains_key(&addr) || addr.ip().is_unspecified() || addr.port() == 0 {
            return false;
        }
        let item = GameServerItem { conn_port: addr.port(), query_port: addr.port(), ip: u32::from(*addr.ip()), time_last_played, ..GameServerItem::default() };
        self.index.insert(addr, self.servers.len());
        self.servers.push(Slot { item, state: State::Queued, goldsrc: false, hidden: false });
        self.pending += 1;
        true
    }

    fn queue(&mut self, i: usize) {
        if self.servers[i].state == State::Done {
            self.pending += 1;
        }
        self.servers[i].state = State::Queued;
        self.cursor = self.cursor.min(i);
    }

    fn finish(&mut self, i: usize) {
        if self.servers[i].state != State::Done {
            self.pending -= 1;
            self.servers[i].state = State::Done;
        }
    }

    /// What Steam would list: the requested app (when the reply names one) and, for
    /// Spectate, only HLTV proxies.
    fn shows(&self, item: &GameServerItem, kind: u8) -> bool {
        (self.app == 0 || item.app_id == 0 || item.app_id == self.app) && (self.kind != ListKind::Spectator || kind == b'p')
    }

    fn masters_done(&self) -> bool {
        self.resolver.is_none() && self.masters.iter().all(|m| m.done)
    }

    /// `EMatchMakingServerResponse` for `RefreshComplete`: the masters never answered, or
    /// nothing is listed once the servers Steam would not list are left out; otherwise
    /// responded (the page itself tells whether any server did).
    fn response(&self) -> i32 {
        if matches!(self.kind, ListKind::Internet | ListKind::Spectator) && !self.master_replied {
            FAILED
        } else if self.servers.iter().all(|s| s.hidden) {
            NO_SERVERS_LISTED
        } else {
            RESPONDED
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum QueryKind {
    Ping,
    Players,
}

/// A `PingServer` / `PlayerDetails` query.
struct Query {
    kind: QueryKind,
    responder: Responder,
    addr: SocketAddrV4,
    sent: Instant,
    challenges: u8,
}

/// [`Matchmaking`] over UDP.
pub struct ServerQueries {
    socket: Option<UdpSocket>,
    fs: Fs,
    /// `MasterServers.vdf`, read on the first Internet or Spectate request.
    masters: Option<Vec<String>>,
    store: Store,
    lists: HashMap<ListRequest, List>,
    next_list: ListRequest,
    queries: HashMap<ServerQuery, Query>,
    next_query: ServerQuery,
    events: Vec<MmEvent>,
    budget: f64,
    last_tick: Instant,
    buf: Vec<u8>,
}

impl ServerQueries {
    pub fn new(fs: Fs) -> Self {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).and_then(|s| {
            s.set_nonblocking(true)?;
            s.set_broadcast(true)?;
            Ok(s)
        });
        let socket = socket.map_err(|e| log::warn!("server browser: no UDP socket: {e}")).ok();
        let store = Store::load(&fs);
        Self { socket, fs, masters: None, store, lists: HashMap::new(), next_list: 1, queries: HashMap::new(), next_query: 0, events: Vec::new(), budget: 0.0, last_tick: Instant::now(), buf: vec![0; 65536] }
    }

    fn send(&self, data: &[u8], to: SocketAddrV4) -> bool {
        let Some(socket) = &self.socket else { return false };
        match socket.send_to(data, to) {
            Ok(_) => true,
            Err(e) => {
                log::debug!("server browser: send to {to}: {e}");
                false
            }
        }
    }

    fn receive(&mut self, now: Instant) {
        let mut buf = std::mem::take(&mut self.buf);
        while let Some(received) = self.socket.as_ref().map(|s| s.recv_from(&mut buf)) {
            match received {
                Ok((n, SocketAddr::V4(from))) => self.handle(&buf[..n], from, now),
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                // Windows reports an ICMP port-unreachable for an earlier send this way.
                Err(e) if e.kind() == ErrorKind::ConnectionReset => {}
                Err(e) => {
                    log::debug!("server browser: receive: {e}");
                    break;
                }
            }
        }
        self.buf = buf;
    }

    fn handle(&mut self, data: &[u8], from: SocketAddrV4, now: Instant) {
        if let Some(addrs) = master::parse(data) {
            self.master_page(from, &addrs);
            return;
        }
        let Some(reply) = a2s::parse(data, from) else { return };
        self.answer_queries(&reply, from, now);
        let mut out = Vec::new();
        for (&h, list) in &mut self.lists {
            let i = match list.index.get(&from) {
                Some(&i) => i,
                None if list.kind == ListKind::Lan && list.refreshing && list.broadcast.is_some_and(|t| now < t + LAN_WINDOW) && !matches!(reply, Reply::Players(_)) => {
                    // A LAN reply to the broadcast: listed as sent at the broadcast.
                    if !list.add(from, 0) {
                        continue;
                    }
                    let i = list.servers.len() - 1;
                    let at = list.broadcast.unwrap_or(now);
                    list.servers[i].state = State::Sent { at, challenges: 0 };
                    list.in_flight.push_back((i, at));
                    list.cursor = list.servers.len();
                    i
                }
                None => continue,
            };
            match (&reply, list.servers[i].state) {
                (Reply::Info { item, kind, source }, State::Sent { at, .. }) => {
                    list.finish(i);
                    list.servers[i].hidden = !list.shows(item, *kind);
                    if list.servers[i].hidden {
                        continue;
                    }
                    let slot = &mut list.servers[i];
                    let time_last_played = slot.item.time_last_played;
                    slot.item = GameServerItem { ping: now.duration_since(at).as_millis() as i32, time_last_played, ..item.clone() };
                    slot.goldsrc = !source;
                    out.push(MmEvent::ServerResponded { responder: list.responder, request: h, server: i as i32 });
                }
                (Reply::Info { item, source: true, .. }, State::Done) if list.servers[i].goldsrc => {
                    let slot = &mut list.servers[i];
                    slot.item = GameServerItem { ping: slot.item.ping, time_last_played: slot.item.time_last_played, ..item.clone() };
                    slot.goldsrc = false;
                }
                (Reply::Challenge(c), State::Sent { challenges, .. }) if challenges < MAX_CHALLENGES => {
                    list.servers[i].state = State::Sent { at: now, challenges: challenges + 1 };
                    list.in_flight.push_back((i, now));
                    if let Some(socket) = &self.socket {
                        let _ = socket.send_to(&a2s::info_request(Some(*c)), from);
                    }
                }
                _ => {}
            }
        }
        self.events.extend(out);
    }

    /// Ping and player queries to `from`.
    fn answer_queries(&mut self, reply: &Reply, from: SocketAddrV4, now: Instant) {
        let ids: Vec<ServerQuery> = self.queries.iter().filter(|(_, q)| q.addr == from).map(|(&id, _)| id).collect();
        for id in ids {
            let Some(q) = self.queries.get_mut(&id) else { continue };
            match (q.kind, reply) {
                (QueryKind::Ping, Reply::Info { item, .. }) => {
                    let mut server = GameServerItem { ping: now.duration_since(q.sent).as_millis() as i32, ..item.clone() };
                    server.time_last_played = self.store.last_played(server.ip, server.conn_port);
                    self.events.push(MmEvent::PingResponded { responder: q.responder, server });
                    self.queries.remove(&id);
                }
                (QueryKind::Players, Reply::Players(players)) => {
                    let responder = q.responder;
                    self.events.extend(players.iter().map(|p| MmEvent::AddPlayer { responder, name: p.name.clone(), score: p.score, time: p.time }));
                    self.events.push(MmEvent::PlayersComplete { responder });
                    self.queries.remove(&id);
                }
                (kind, Reply::Challenge(c)) if q.challenges < MAX_CHALLENGES => {
                    q.challenges += 1;
                    q.sent = now;
                    let packet = if kind == QueryKind::Ping { a2s::info_request(Some(*c)) } else { a2s::players_request(*c) };
                    self.send(&packet, from);
                }
                _ => {}
            }
        }
    }

    /// A master's page: the new addresses are queued; the master goes on from the last one
    /// until the `0.0.0.0:0` terminator or a page with nothing it had not listed yet.
    fn master_page(&mut self, from: SocketAddrV4, addrs: &[SocketAddrV4]) {
        let awaited = |m: &MasterFetch| m.addr == Some(from) && m.sent.is_some() && !m.done;
        let Some(list) = self.lists.values_mut().find(|l| l.masters.iter().any(awaited)) else { return };
        list.master_replied = true;
        let listed: Vec<SocketAddrV4> = addrs.iter().copied().take_while(|a| !(a.ip().is_unspecified() && a.port() == 0)).collect();
        for &a in &listed {
            list.add(a, 0);
        }
        let ended = listed.len() < addrs.len() || listed.is_empty();
        let Some(m) = list.masters.iter_mut().find(|m| awaited(m)) else { return };
        let before = m.seen.len();
        m.seen.extend(listed.iter().copied());
        m.sent = None;
        m.tries = 0;
        m.pages += 1;
        m.seed = listed.last().copied();
        m.done = ended || m.seen.len() == before || m.pages >= MASTER_MAX_PAGES;
    }

    /// DNS answers, first and next pages, timeouts.
    fn advance_masters(&mut self, now: Instant) {
        let mut sends = Vec::new();
        for list in self.lists.values_mut() {
            if let Some(rx) = &list.resolver {
                match rx.try_recv() {
                    Ok(addrs) => {
                        for (m, addr) in list.masters.iter_mut().zip(addrs) {
                            m.addr = addr;
                            m.done = addr.is_none();
                        }
                        list.resolver = None;
                    }
                    Err(mpsc::TryRecvError::Empty) => continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        list.masters.iter_mut().for_each(|m| m.done = true);
                        list.resolver = None;
                    }
                }
            }
            for m in list.masters.iter_mut().filter(|m| !m.done) {
                let Some(addr) = m.addr else { continue };
                if m.sent.is_some_and(|t| now.duration_since(t) < MASTER_TIMEOUT) {
                    continue;
                }
                if m.tries >= MASTER_TRIES {
                    log::info!("server browser: master {addr} stopped answering after {} pages", m.pages);
                    m.done = true;
                    continue;
                }
                m.tries += 1;
                m.sent = Some(now);
                let seed = m.seed.unwrap_or(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
                sends.push((master::request(seed, &list.filter), addr));
            }
        }
        for (packet, addr) in sends {
            self.send(&packet, addr);
        }
    }

    /// Queued servers, as fast as the send budget allows.
    fn send_queued(&mut self, now: Instant) {
        let info = a2s::info_request(None);
        let mut failed = Vec::new();
        for (&h, list) in &mut self.lists {
            while self.budget >= 1.0 && list.cursor < list.servers.len() {
                let i = list.cursor;
                list.cursor += 1;
                if list.servers[i].state != State::Queued {
                    continue;
                }
                let addr = list.servers[i].addr();
                let sent = self.socket.as_ref().is_some_and(|s| s.send_to(&info, addr).is_ok());
                self.budget -= 1.0;
                if sent {
                    list.servers[i].state = State::Sent { at: now, challenges: 0 };
                    list.in_flight.push_back((i, now));
                } else {
                    list.finish(i);
                    failed.push(MmEvent::ServerFailedToRespond { responder: list.responder, request: h, server: i as i32 });
                }
            }
        }
        self.events.extend(failed);
    }

    /// Unanswered servers and queries fail.
    fn expire(&mut self, now: Instant) {
        for (&h, list) in &mut self.lists {
            while let Some(&(i, at)) = list.in_flight.front() {
                if now.duration_since(at) < QUERY_TIMEOUT {
                    break;
                }
                list.in_flight.pop_front();
                if matches!(list.servers[i].state, State::Sent { at: t, .. } if t == at) {
                    list.finish(i);
                    self.events.push(MmEvent::ServerFailedToRespond { responder: list.responder, request: h, server: i as i32 });
                }
            }
        }
        let expired: Vec<ServerQuery> = self.queries.iter().filter(|(_, q)| now.duration_since(q.sent) >= QUERY_TIMEOUT).map(|(&id, _)| id).collect();
        for id in expired {
            if let Some(q) = self.queries.remove(&id) {
                self.events.push(match q.kind {
                    QueryKind::Ping => MmEvent::PingFailed { responder: q.responder },
                    QueryKind::Players => MmEvent::PlayersFailed { responder: q.responder },
                });
            }
        }
    }

    /// `RefreshComplete` once the masters, the LAN window and every queued server are done.
    fn complete(&mut self, now: Instant) {
        for (&h, list) in &mut self.lists {
            let lan_open = list.broadcast.is_some_and(|t| now < t + LAN_WINDOW);
            if list.refreshing && list.pending == 0 && !lan_open && list.masters_done() {
                list.refreshing = false;
                self.events.push(MmEvent::RefreshComplete { responder: list.responder, request: h, response: list.response() });
            }
        }
    }

    fn masters(&mut self) -> Vec<String> {
        let fs = &self.fs;
        self.masters.get_or_insert_with(|| master::load(fs)).clone()
    }
}

/// The masters' addresses, looked up off the main thread.
fn resolve(hosts: Vec<String>) -> Option<Receiver<Vec<Option<SocketAddrV4>>>> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("master-dns".into()).spawn(move || {
        let addrs = hosts
            .iter()
            .map(|h| {
                let found = h.to_socket_addrs().ok().and_then(|mut it| it.find_map(|a| if let SocketAddr::V4(v) = a { Some(v) } else { None }));
                if found.is_none() {
                    log::info!("server browser: master {h} not found");
                }
                found
            })
            .collect();
        let _ = tx.send(addrs);
    });
    spawned.map_err(|e| log::warn!("server browser: master lookup: {e}")).ok().map(|_| rx)
}

impl Matchmaking for ServerQueries {
    fn request_server_list(&mut self, kind: ListKind, app: u32, filters: &[(String, String)], responder: Responder) -> ListRequest {
        let h = self.next_list;
        self.next_list += 1;
        let mut list = List::new(kind, responder, app);
        match kind {
            ListKind::Internet | ListKind::Spectator => {
                let hosts = self.masters();
                list.filter = master::filter(filters);
                list.masters = hosts.iter().map(|_| MasterFetch::default()).collect();
                list.resolver = resolve(hosts);
                if list.resolver.is_none() {
                    list.masters.clear();
                }
            }
            ListKind::Lan => {
                let now = Instant::now();
                list.broadcast = Some(now);
                let packet = a2s::info_request(None);
                for port in LAN_PORTS {
                    self.send(&packet, SocketAddrV4::new(Ipv4Addr::BROADCAST, port));
                }
            }
            ListKind::Favorites | ListKind::History => {
                let flags = if kind == ListKind::Favorites { FAVORITE } else { HISTORY };
                for e in self.store.list(flags).iter().filter(|e| app == 0 || e.app == 0 || e.app == app) {
                    if list.add(SocketAddrV4::new(Ipv4Addr::from(e.ip), e.query_port), e.last_played) {
                        let i = list.servers.len() - 1;
                        list.servers[i].item.conn_port = e.conn_port;
                    }
                }
            }
            // Steam friends do not exist here.
            ListKind::Friends => {}
        }
        self.lists.insert(h, list);
        h
    }

    fn release_request(&mut self, request: ListRequest) {
        self.lists.remove(&request);
    }

    fn server_details(&self, request: ListRequest, server: i32) -> Option<GameServerItem> {
        let list = self.lists.get(&request)?;
        usize::try_from(server).ok().and_then(|i| list.servers.get(i)).map(|s| s.item.clone())
    }

    /// Stops the masters and the queued servers; a running refresh then completes, so the
    /// page's "Stop refresh" button turns back (serverbrowser.md §17).
    fn cancel_query(&mut self, request: ListRequest) {
        let Some(list) = self.lists.get_mut(&request) else { return };
        list.resolver = None;
        list.masters.iter_mut().for_each(|m| m.done = true);
        list.broadcast = None;
        for i in 0..list.servers.len() {
            list.finish(i);
        }
        list.in_flight.clear();
        list.cursor = list.servers.len();
    }

    fn refresh_query(&mut self, request: ListRequest) {
        let Some(list) = self.lists.get_mut(&request) else { return };
        for i in 0..list.servers.len() {
            list.queue(i);
        }
        list.in_flight.clear();
        list.refreshing = true;
    }

    fn is_refreshing(&self, request: ListRequest) -> bool {
        self.lists.get(&request).is_some_and(|l| l.refreshing)
    }

    fn refresh_server(&mut self, request: ListRequest, server: i32) {
        let Some(list) = self.lists.get_mut(&request) else { return };
        if let Ok(i) = usize::try_from(server)
            && i < list.servers.len()
            && list.servers[i].state == State::Done
        {
            list.queue(i);
        }
    }

    fn ping_server(&mut self, ip: u32, port: u16, responder: Responder) -> ServerQuery {
        let addr = SocketAddrV4::new(Ipv4Addr::from(ip), port);
        self.send(&a2s::info_request(None), addr);
        self.add_query(QueryKind::Ping, responder, addr)
    }

    fn player_details(&mut self, ip: u32, port: u16, responder: Responder) -> ServerQuery {
        let addr = SocketAddrV4::new(Ipv4Addr::from(ip), port);
        self.send(&a2s::players_request([0xFF; 4]), addr);
        self.add_query(QueryKind::Players, responder, addr)
    }

    fn cancel_server_query(&mut self, query: ServerQuery) {
        self.queries.remove(&query);
    }

    fn release_responder(&mut self, responder: Responder) {
        self.lists.retain(|_, l| l.responder != responder);
        self.queries.retain(|_, q| q.responder != responder);
    }

    fn favorite_game_count(&self) -> i32 {
        self.store.count()
    }

    fn add_favorite_game(&mut self, app: u32, ip: u32, conn_port: u16, query_port: u16, flags: u32, time: u32) -> i32 {
        let i = self.store.add(&self.fs, flags, Entry { app, ip, conn_port, query_port, last_played: time });
        self.events.push(MmEvent::FavoritesChanged { ip, query_port: u32::from(query_port), conn_port: u32::from(conn_port), app_id: app, flags, add: true });
        i
    }

    fn remove_favorite_game(&mut self, app: u32, ip: u32, conn_port: u16, query_port: u16, flags: u32) -> bool {
        let removed = self.store.remove(&self.fs, flags, ip, conn_port);
        if removed {
            self.events.push(MmEvent::FavoritesChanged { ip, query_port: u32::from(query_port), conn_port: u32::from(conn_port), app_id: app, flags, add: false });
        }
        removed
    }

    fn is_vac_banned(&self) -> bool {
        false
    }

    fn run_callbacks(&mut self) -> Vec<MmEvent> {
        let now = Instant::now();
        self.budget = (self.budget + now.duration_since(self.last_tick).as_secs_f64() * SEND_RATE).min(MAX_BURST);
        self.last_tick = now;
        self.receive(now);
        self.advance_masters(now);
        self.send_queued(now);
        self.expire(now);
        self.complete(now);
        std::mem::take(&mut self.events)
    }
}

impl ServerQueries {
    fn add_query(&mut self, kind: QueryKind, responder: Responder, addr: SocketAddrV4) -> ServerQuery {
        let id = self.next_query;
        self.next_query = self.next_query.wrapping_add(1).max(0);
        self.queries.insert(id, Query { kind, responder, addr, sent: Instant::now(), challenges: 0 });
        id
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gs_gameui::GameFs;

    use super::*;

    #[derive(Default)]
    struct MemFs(RefCell<HashMap<String, Vec<u8>>>);

    impl GameFs for MemFs {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.borrow().get(path).cloned()
        }
        fn read_in(&self, path: &str, _: &str) -> Option<Vec<u8>> {
            self.read(path)
        }
        fn exists(&self, path: &str) -> bool {
            self.0.borrow().contains_key(path)
        }
        fn find(&self, _: &str, _: Option<&str>) -> Vec<(String, bool)> {
            Vec::new()
        }
        fn write_in(&self, path: &str, _: &str, data: &[u8]) -> bool {
            self.0.borrow_mut().insert(path.to_owned(), data.to_vec());
            true
        }
        fn add_search_path(&self, _: &str, _: &str) {}
        fn remove(&self, path: &str, _: Option<&str>) {
            self.0.borrow_mut().remove(path);
        }
        fn game_dir(&self) -> String {
            "cstrike".into()
        }
    }

    /// A server on loopback that wants a challenge before it answers info and players.
    struct FakeServer(UdpSocket);

    impl FakeServer {
        fn new() -> Self {
            let s = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            s.set_nonblocking(true).unwrap();
            Self(s)
        }

        fn port(&self) -> u16 {
            self.0.local_addr().unwrap().port()
        }

        fn serve(&self) {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = self.0.recv_from(&mut buf) {
                let q = &buf[..n];
                let reply: Vec<u8> = if q.starts_with(b"\xff\xff\xff\xffTSource Engine Query\0") && n == 25 {
                    b"\xff\xff\xff\xffA\x11\x22\x33\x44".to_vec()
                } else if q == b"\xff\xff\xff\xffTSource Engine Query\0\x11\x22\x33\x44" {
                    let mut d = b"\xff\xff\xff\xffI\x30Loopback\0de_dust2\0cstrike\0Counter-Strike\0\x0a\x00\x03\x20\x01dw\x00\x011.1.2.7\0\x80".to_vec();
                    d.extend_from_slice(&27099u16.to_le_bytes());
                    d
                } else if q == b"\xff\xff\xff\xffU\xff\xff\xff\xff" {
                    b"\xff\xff\xff\xffA\x55\x66\x77\x88".to_vec()
                } else if q == b"\xff\xff\xff\xffU\x55\x66\x77\x88" {
                    let mut d = b"\xff\xff\xff\xffD\x01\x00Lenya\0".to_vec();
                    d.extend_from_slice(&5i32.to_le_bytes());
                    d.extend_from_slice(&90.0f32.to_le_bytes());
                    d
                } else {
                    continue;
                };
                self.0.send_to(&reply, from).unwrap();
            }
        }
    }

    /// Pumps both sides until `done` holds for the events so far (or 5 s pass).
    fn pump(sq: &mut ServerQueries, server: &FakeServer, done: impl Fn(&[MmEvent]) -> bool) -> Vec<MmEvent> {
        let start = Instant::now();
        let mut events = Vec::new();
        while !done(&events) && start.elapsed() < Duration::from_secs(5) {
            server.serve();
            events.extend(sq.run_callbacks());
            std::thread::sleep(Duration::from_millis(5));
        }
        events
    }

    /// A favorite behind a challenge is listed with the game port of its reply and the time
    /// from the file; one nobody answers fails; then the refresh completes as responded.
    #[test]
    fn favorites_answer_through_the_challenge_and_silent_ones_fail() {
        let server = FakeServer::new();
        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let silent_port = silent.local_addr().unwrap().port();
        let fs = Rc::new(MemFs::default());
        let hist = format!(
            "\"Filters\"\n{{\n\"favorites\"\n{{\n\"1\"\n{{\n\"address\" \"127.0.0.1:{}\"\n\"LastPlayed\" \"1700000000\"\n\"appid\" \"10\"\n}}\n\"2\"\n{{\n\"address\" \"127.0.0.1:{silent_port}\"\n\"appid\" \"0\"\n}}\n}}\n}}\n",
            server.port()
        );
        fs.write_in("serverbrowser_hist.vdf", "CONFIG", hist.as_bytes());
        let mut sq = ServerQueries::new(fs);
        assert_eq!(sq.favorite_game_count(), 2);
        let h = sq.request_server_list(ListKind::Favorites, 10, &[], 7);
        let events = pump(&mut sq, &server, |e| e.iter().any(|e| matches!(e, MmEvent::RefreshComplete { .. })));
        assert_eq!(
            events,
            vec![
                MmEvent::ServerResponded { responder: 7, request: h, server: 0 },
                MmEvent::ServerFailedToRespond { responder: 7, request: h, server: 1 },
                MmEvent::RefreshComplete { responder: 7, request: h, response: RESPONDED },
            ]
        );
        let item = sq.server_details(h, 0).unwrap();
        assert_eq!((item.server_name.as_str(), item.conn_port, item.query_port, item.players, item.secure), ("Loopback", 27099, server.port(), 3, true));
        assert_eq!((item.time_last_played, item.had_successful_response), (1_700_000_000, true));
        assert!(!sq.server_details(h, 1).unwrap().had_successful_response);
        assert!(!sq.is_refreshing(h));
        drop(silent);
    }

    #[test]
    fn player_details_answer_the_challenge() {
        let server = FakeServer::new();
        let mut sq = ServerQueries::new(Rc::new(MemFs::default()));
        sq.player_details(u32::from(Ipv4Addr::LOCALHOST), server.port(), 3);
        let events = pump(&mut sq, &server, |e| !e.is_empty());
        assert_eq!(events, vec![MmEvent::AddPlayer { responder: 3, name: "Lenya".into(), score: 5, time: 90.0 }, MmEvent::PlayersComplete { responder: 3 }]);
    }

    /// History additions persist in Steam's layout and are announced; a repeat only moves
    /// the time; removal by address ignores the app ID it is asked with.
    #[test]
    fn history_persists_and_announces_changes() {
        let fs = Rc::new(MemFs::default());
        let mut sq = ServerQueries::new(fs.clone());
        let ip = u32::from(Ipv4Addr::new(203, 0, 113, 151));
        assert_eq!(sq.add_favorite_game(0, ip, 27018, 27018, HISTORY, 100), 0);
        assert_eq!(sq.add_favorite_game(0, ip, 27018, 27018, HISTORY, 200), 0);
        assert_eq!(sq.run_callbacks().len(), 2);
        let text = String::from_utf8_lossy(&fs.read("serverbrowser_hist.vdf").unwrap()).into_owned();
        assert!(text.contains("\"history\"") && text.contains("\"address\"\t\t\"203.0.113.151:27018\"") && text.contains("\"LastPlayed\"\t\t\"200\""), "{text}");
        assert_eq!(ServerQueries::new(fs.clone()).favorite_game_count(), 1);
        assert!(!sq.remove_favorite_game(10, ip, 27018, 27018, FAVORITE));
        assert!(sq.remove_favorite_game(10, ip, 27018, 27018, HISTORY));
        assert_eq!(sq.run_callbacks(), vec![MmEvent::FavoritesChanged { ip, query_port: 27018, conn_port: 27018, app_id: 10, flags: HISTORY, add: false }]);
        assert_eq!(ServerQueries::new(fs).favorite_game_count(), 0);
    }
}
