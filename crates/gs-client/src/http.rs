//! `sv_downloadurl` downloads (docs/re/engine-core.md §5.7, engine-misc.md §4): the engine's
//! `CDownloadManager` over ISteamHTTP, here plain HTTP GETs on worker threads. At most five
//! requests run at once; a body is saved raw to GAMEDOWNLOAD when the file is still missing;
//! once everything finished the client `retry`s, and the server+map key keeps HTTP from being
//! tried again for it this process. The loading dialog follows along: each start moves the
//! "Http" progress to `total − queued`, and the running requests' average percentage drives
//! the secondary bar.

use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use gs_fs::{FileSystem, PathId};

/// `CDownloadManager`'s parallel request limit.
const MAX_ACTIVE: usize = 5;
/// Largest body read (the request is otherwise unbounded).
const MAX_BODY: u64 = 512 * 1024 * 1024;
/// Redirects followed per request (each hop is checked like the first URL).
const MAX_REDIRECTS: usize = 10;

/// One queued download: base URL, relative file name, whether the server is local.
#[derive(Debug, Clone)]
struct Request {
    base: String,
    file: String,
    /// The game server is on this machine or its local network: so may the files be.
    server_local: bool,
}

/// A running request: its id, the request, and `GetHTTPDownloadProgressPct` (an `f32` percent
/// in bits, written by the worker).
#[derive(Debug)]
struct Active {
    id: u64,
    req: Request,
    pct: Arc<AtomicU32>,
}

/// What a worker reports: request id and the body of a successful (200/304, non-empty) reply.
type Completion = (u64, Option<Vec<u8>>);

/// `TheDownloadManager`.
#[derive(Debug)]
pub struct HttpDownloads {
    /// `"<servername>:<levelname>"` keys already tried (`CL_MarkMapAsUsingHTTPDownload`).
    tried: HashSet<String>,
    queue: VecDeque<Request>,
    active: Vec<Active>,
    next_id: u64,
    tx: Sender<Completion>,
    rx: Receiver<Completion>,
    /// Requests ever queued in this batch (for the progress count).
    total: usize,
    /// The integer average percentage last pushed to the loading dialog.
    last_pct: i32,
    /// The file the secondary progress text names (the latest started).
    current: String,
}

impl Default for HttpDownloads {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { tried: HashSet::new(), queue: VecDeque::new(), active: Vec::new(), next_id: 0, tx, rx, total: 0, last_pct: -1, current: String::new() }
    }
}

/// What one `CL_HTTPUpdate` produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HttpOutput {
    /// Lines the manager prints (`developer`-only ones flagged).
    pub lines: Vec<(bool, String)>,
    /// `UpdateProgress`: the running requests' average percentage as a fraction, when its
    /// integer part changed, and the current file.
    pub progress: Option<(f32, String)>,
    /// `StartQueuedDownloads`: per started request, the "Http" progress point
    /// (`total − queued`) and its file.
    pub started: Vec<(i32, String)>,
    /// Everything finished: `NotifyOfServerDisconnect`, then `Cbuf_AddText("retry\n")`.
    pub retry: bool,
}

impl HttpDownloads {
    /// `CL_CanUseHTTPDownload`.
    pub fn can_use(&self, base_url: &str, key: &str) -> bool {
        !base_url.is_empty() && !self.tried.contains(key)
    }

    /// `CL_QueueHTTPDownload` (not for a `local` server).
    pub fn queue(&mut self, base: &str, file: &str, server: &str) {
        if server == "local" {
            return;
        }
        let server_local = server.parse::<std::net::SocketAddrV4>().is_ok_and(|a| gs_common::net::is_local(std::net::IpAddr::V4(*a.ip())));
        self.queue.push_back(Request { base: base.to_owned(), file: file.to_owned(), server_local });
        self.total += 1;
    }

    /// Requests queued in this batch (`StartProgressBar`'s step count).
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// `CL_MarkMapAsUsingHTTPDownload`.
    pub fn mark_tried(&mut self, key: &str) {
        self.tried.insert(key.to_owned());
    }

    /// Something is queued or running.
    pub fn busy(&self) -> bool {
        !self.queue.is_empty() || !self.active.is_empty()
    }

    /// `CL_HTTPUpdate`: `UpdateProgress` (completions saved, the average percentage, the `retry`
    /// once the last one ended), then `StartQueuedDownloads` (up to five running).
    pub fn update(&mut self, fs: &FileSystem) -> HttpOutput {
        let mut out = HttpOutput::default();
        let mut finished = false;
        while let Ok((id, body)) = self.rx.try_recv() {
            let Some(at) = self.active.iter().position(|a| a.id == id) else { continue };
            let req = self.active.remove(at).req;
            finished = true;
            match body {
                Some(data) => {
                    if !fs.exists(&req.file)
                        && let Err(e) = fs.write(&req.file, PathId::GameDownload, &data)
                    {
                        out.lines.push((false, format!("{}: {e}\n", req.file)));
                    }
                    out.lines.push((true, format!("Saved {}{} to disk\n", req.base, req.file)));
                }
                None => out.lines.push((true, format!("Failed HTTP download of {}{}.\n", req.base, req.file))),
            }
        }
        if !self.active.is_empty() {
            // Σ(pct·0.01)/n·100, pushed when its integer part moves.
            let sum: f32 = self.active.iter().map(|a| f32::from_bits(a.pct.load(Ordering::Relaxed)) * 0.01).sum();
            let pct = sum / self.active.len() as f32 * 100.0;
            if pct as i32 != self.last_pct {
                self.last_pct = pct as i32;
                out.progress = Some((pct * 0.01, self.current.clone()));
            }
        }
        if finished && !self.busy() {
            self.total = 0;
            self.last_pct = -1;
            out.retry = true;
        }
        while self.active.len() < MAX_ACTIVE
            && let Some(req) = self.queue.pop_front()
        {
            let url = format!("{}{}", req.base, req.file).replace('\\', "/");
            out.lines.push((true, format!("Requesting HTTP download of {}{}.\n", req.base, req.file)));
            let (id, tx) = (self.next_id, self.tx.clone());
            self.next_id += 1;
            let pct = Arc::new(AtomicU32::new(0f32.to_bits()));
            let worker_pct = pct.clone();
            let server_local = req.server_local;
            std::thread::spawn(move || {
                let _ = tx.send((id, fetch(&url, &worker_pct, server_local)));
            });
            out.started.push(((self.total - self.queue.len()) as i32, req.file.clone()));
            self.current = req.file.clone();
            self.active.push(Active { id, req, pct });
        }
        out
    }

    /// `httpstop`: every request dropped, the queue emptied and the tried list cleared.
    pub fn stop(&mut self) -> Vec<String> {
        let lines = self.active.drain(..).map(|a| format!("Aborting download of {}\n", a.req.file)).collect();
        self.queue.clear();
        self.tried.clear();
        self.total = 0;
        self.last_pct = -1;
        // Late completions of the dropped requests are ignored (no matching id).
        lines
    }
}

/// One GET with Steam's headers; the body on status 200 or 304 with data. `pct` follows the
/// body as a percentage of `Content-Length` (0 while the length is unknown). Redirects are
/// followed here so every hop, like the first URL, stays off this machine and its network
/// unless the server is local (`server_local`).
fn fetch(url: &str, pct: &AtomicU32, server_local: bool) -> Option<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).build().into();
    let mut target = url::Url::parse(url).map_err(|e| log::debug!("{url}: {e}")).ok()?;
    let mut resp = None;
    for _ in 0..=MAX_REDIRECTS {
        let allowed = matches!(target.scheme(), "http" | "https")
            && target.host_str().is_some_and(|h| target.port_or_known_default().is_some_and(|p| gs_common::net::request_allowed(h, p, server_local)));
        if !allowed {
            log::warn!("HTTP download {target}: refusing a local or unresolvable address");
            return None;
        }
        let r = agent.get(target.as_str()).header("Cache-Control", "no-cache").header("User-Agent", "Half-Life").call().map_err(|e| log::debug!("{target}: {e}")).ok()?;
        let location = r.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_owned);
        match location.filter(|_| matches!(r.status().as_u16(), 301 | 302 | 303 | 307 | 308)) {
            Some(next) => target = target.join(&next).ok()?,
            None => {
                resp = Some(r);
                break;
            }
        }
    }
    let mut resp = resp?;
    if !matches!(resp.status().as_u16(), 200 | 304) {
        return None;
    }
    let length = resp.body().content_length().filter(|&n| n > 0);
    let mut reader = resp.body_mut().as_reader().take(MAX_BODY);
    let mut body = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| log::debug!("{url}: {e}")).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
        if let Some(len) = length {
            let p = (body.len() as f64 * 100.0 / len as f64).min(100.0) as f32;
            pct.store(p.to_bits(), Ordering::Relaxed);
        }
    }
    (!body.is_empty()).then_some(body)
}
