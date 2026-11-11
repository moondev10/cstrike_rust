//! The MOTD's HTML control (`CClientMOTD` over chromehtml's CEF 18, docs/re/audit-ac-checks.md
//! A6, client-hud.md §MOTD): an `http://` message is navigated to, anything else is loaded from
//! `motd_temp.html`. On Windows with a visible window the page is a locked-down WebView2 over
//! the panel ([`page`]). Elsewhere, or without the WebView2 runtime, pages are fetched here with
//! the CEF user agent and a cookie jar that persists across runs, the sub-resources a browser
//! would load (images, scripts, style sheets, frames) are requested too, and the page is shown
//! as text.

#[cfg(windows)]
mod page;
#[cfg(any(windows, test))]
mod proxy;

use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cookie_store::CookieStore;

/// chromehtml's `Mozilla/5.0 (%s; U; %s; en-US; %s/%llu; %s) …` with "Windows", the
/// unmanifested `GetVersionEx` answer (6.2 from Windows 8 on), the identifier hw.dll passes to
/// `CreateBrowser` ("Valve Half-Life"), the never-set client build id (0) and an empty field.
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows; U; Windows NT 6.2; en-US; Valve Half-Life/0; ) AppleWebKit/535.15 (KHTML, like Gecko) Chrome/18.0.989.0 Safari/535.11";

/// Chrome 18's request headers besides the user agent. [INFERENCE: Chrome 18 defaults]
const ACCEPT_DOCUMENT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.8";
const ACCEPT_CHARSET: &str = "ISO-8859-1,utf-8;q=0.7,*;q=0.3";
/// Largest page read.
const MAX_PAGE: u64 = 4 * 1024 * 1024;
/// Chrome's redirect limit.
const MAX_REDIRECTS: usize = 20;

/// The HTTP agent (redirects handled here, so every `Set-Cookie` is stored) and the jar.
#[derive(Clone)]
struct Http {
    agent: ureq::Agent,
    cookies: Arc<Mutex<CookieStore>>,
    /// The game server is on a local address: the page may load from the local network too.
    server_local: bool,
}

/// The browser of the MOTD panel.
pub struct MotdBrowser {
    http: Http,
    /// The cookie jar file (CEF keeps `htmlcookies` next to hl.exe).
    jar: PathBuf,
    tx: Sender<(u64, String)>,
    rx: Receiver<(u64, String)>,
    /// The latest navigation; pages of older ones are dropped.
    generation: u64,
    /// The window the page is made in at the first navigation (taken then: one attempt).
    #[cfg(windows)]
    parent: Option<std::sync::Arc<winit::window::Window>>,
    #[cfg(windows)]
    page: Option<page::Page>,
}

impl MotdBrowser {
    /// The text browser, its cookie jar (`htmlcookies.json`) next to the executable.
    pub fn new() -> Self {
        let jar = exe_dir().join("htmlcookies.json");
        let config = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .build();
        let store = std::fs::File::open(&jar)
            .ok()
            .and_then(|f| cookie_store::serde::json::load(BufReader::new(f)).map_err(|e| log::debug!("{}: {e}", jar.display())).ok())
            .unwrap_or_default();
        let (tx, rx) = channel();
        Self {
            http: Http { agent: config.into(), cookies: Arc::new(Mutex::new(store)), server_local: false },
            jar,
            tx,
            rx,
            generation: 0,
            #[cfg(windows)]
            parent: None,
            #[cfg(windows)]
            page: None,
        }
    }

    /// Pages go to a WebView2 over `window` (Windows), its profile in `htmlcache` next to the
    /// executable.
    pub fn attach(&mut self, window: &std::sync::Arc<winit::window::Window>) {
        #[cfg(windows)]
        {
            self.parent = Some(std::sync::Arc::clone(window));
        }
        #[cfg(not(windows))]
        let _ = window;
    }

    /// The WebView2 page, made at the first call after [`Self::attach`].
    #[cfg(windows)]
    fn page(&mut self) -> Option<&mut page::Page> {
        if self.page.is_none()
            && let Some(window) = self.parent.take()
        {
            match page::Page::new(&window, exe_dir().join("htmlcache")) {
                Ok(page) => self.page = Some(page),
                Err(e) => log::warn!("MOTD browser: {e}; pages are shown as text"),
            }
        }
        self.page.as_mut()
    }

    /// Whether pages show in the WebView2 (the panel draws no text of its own).
    pub fn shows_pages(&self) -> bool {
        #[cfg(windows)]
        let shows = self.page.is_some();
        #[cfg(not(windows))]
        let shows = false;
        shows
    }

    /// Navigates to `url`. Unless the server is local (`server_local`), nothing loads from this
    /// machine or its network; the text browser's page arrives through [`Self::poll`].
    pub fn navigate(&mut self, url: &str, server_local: bool) {
        #[cfg(windows)]
        if let Some(page) = self.page() {
            page.load_url(url, server_local);
            return;
        }
        self.start(Source::Url(url.to_owned()), server_local);
    }

    /// Loads a local page (`file:///…/motd_temp.html`): only its absolute `http(s)` resources go
    /// to the network (public ones unless `server_local`).
    pub fn load_html(&mut self, html: &str, server_local: bool) {
        #[cfg(windows)]
        if let Some(page) = self.page() {
            page.load_html(html, server_local);
            return;
        }
        self.start(Source::Html(html.to_owned()), server_local);
    }

    /// Shows the WebView2 page at `rect` (window pixels) or hides it.
    pub fn place(&mut self, rect: Option<(i32, i32, i32, i32)>) {
        #[cfg(windows)]
        if let Some(page) = &mut self.page {
            page.place(rect);
        }
        #[cfg(not(windows))]
        let _ = rect;
    }

    /// Keyboard focus back from the page to the game window.
    pub fn focus_game(&self) {
        #[cfg(windows)]
        if let Some(page) = &self.page {
            page.focus_game();
        }
    }

    fn start(&mut self, source: Source, server_local: bool) {
        self.generation += 1;
        let http = Http { server_local, ..self.http.clone() };
        let (generation, tx, jar) = (self.generation, self.tx.clone(), self.jar.clone());
        std::thread::spawn(move || {
            let (page, base) = match source {
                Source::Url(url) => match fetch_document(&http, &url, None) {
                    Some((page, final_url)) => (Some(page), Some(final_url)),
                    None => (None, None),
                },
                Source::Html(html) => (Some(html), None),
            };
            if let Some(page) = &page {
                let _ = tx.send((generation, page.clone()));
                load_resources(&http, page, base.as_ref(), 1);
            }
            http.save(&jar);
        });
    }

    /// The text browser's page of the latest navigation, once it arrived.
    pub fn poll(&mut self) -> Option<String> {
        let mut page = None;
        while let Ok((generation, text)) = self.rx.try_recv() {
            if generation == self.generation {
                page = Some(text);
            }
        }
        page
    }
}

/// The executable's directory (the working directory if it is unknown).
fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default()
}

enum Source {
    Url(String),
    Html(String),
}

impl Http {
    fn save(&self, jar: &PathBuf) {
        let store = self.cookies.lock().unwrap_or_else(PoisonError::into_inner);
        let res = std::fs::File::create(jar).map_err(|e| e.to_string()).and_then(|mut f| cookie_store::serde::json::save(&store, &mut f).map_err(|e| e.to_string()));
        if let Err(e) = res {
            log::debug!("{}: {e}", jar.display());
        }
    }

    /// A GET with the browser's headers and cookies (`name=value; …`); redirects are followed
    /// up to Chrome's limit and every response's `Set-Cookie` is stored. Returns the final
    /// response and URL.
    fn get(&self, url: &url::Url, accept: &str, referer: Option<&str>) -> Option<(ureq::http::Response<ureq::Body>, url::Url)> {
        let mut url = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            let cookie = {
                let store = self.cookies.lock().unwrap_or_else(PoisonError::into_inner);
                store.get_request_values(&url).map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; ")
            };
            let mut req = self.agent.get(url.as_str()).header("Accept", accept).header("Accept-Language", ACCEPT_LANGUAGE).header("Accept-Charset", ACCEPT_CHARSET);
            if let Some(r) = referer {
                req = req.header("Referer", r);
            }
            if !cookie.is_empty() {
                req = req.header("Cookie", &cookie);
            }
            if !url.host_str().is_some_and(|h| url.port_or_known_default().is_some_and(|p| gs_common::net::request_allowed(h, p, self.server_local))) {
                log::warn!("MOTD {url}: refusing a local or unresolvable address");
                return None;
            }
            let resp = req.call().map_err(|e| log::debug!("MOTD {url}: {e}")).ok()?;
            {
                let mut store = self.cookies.lock().unwrap_or_else(PoisonError::into_inner);
                for value in resp.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()) {
                    if let Err(e) = store.parse(value, &url) {
                        log::debug!("MOTD cookie from {url}: {e}");
                    }
                }
            }
            let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_owned);
            match location.filter(|_| matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308)) {
                Some(next) => url = url.join(&next).ok()?,
                None => return Some((resp, url)),
            }
        }
        None
    }
}

/// A page as text (bytes kept losslessly) and the URL it finally came from.
fn fetch_document(http: &Http, url: &str, referer: Option<&str>) -> Option<(String, url::Url)> {
    let parsed = url::Url::parse(url).map_err(|e| log::debug!("MOTD {url}: {e}")).ok()?;
    let (mut resp, final_url) = http.get(&parsed, ACCEPT_DOCUMENT, referer)?;
    let mut body = Vec::new();
    resp.body_mut().as_reader().take(MAX_PAGE).read_to_end(&mut body).map_err(|e| log::debug!("MOTD {url}: {e}")).ok()?;
    Some((gs_common::bytestr::decode(&body), final_url))
}

/// The resources a page references: `src` of img/script/iframe/frame/embed, `href` of link,
/// `background` of body/td/table, and a `<meta http-equiv="refresh">` target.
fn resources(page: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let lower = page.to_ascii_lowercase();
    let mut at = 0;
    while let Some(start) = lower[at..].find('<').map(|i| at + i) {
        let end = lower[start..].find('>').map_or(lower.len(), |i| start + i);
        let tag = &page[start + 1..end];
        let tag_lower = &lower[start + 1..end];
        let name: String = tag_lower.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let attrs: &[(&str, bool)] = match name.as_str() {
            "img" | "script" | "embed" | "input" => &[("src", false)],
            "iframe" | "frame" => &[("src", true)],
            "link" => &[("href", false)],
            "body" | "td" | "table" => &[("background", false)],
            "meta" if tag_lower.contains("http-equiv") && tag_lower.contains("refresh") => &[("content", true)],
            _ => &[],
        };
        for &(attr, document) in attrs {
            if let Some(value) = attribute(tag, tag_lower, attr) {
                let value = if name == "meta" {
                    let v = value.to_ascii_lowercase();
                    match v.find("url=") {
                        Some(i) => value[i + 4..].trim().trim_matches(['\'', '"']).to_owned(),
                        None => continue,
                    }
                } else {
                    value
                };
                out.push((value, document));
            }
        }
        at = end.min(lower.len());
        if at >= lower.len() {
            break;
        }
    }
    out
}

/// The value of `attr` in a tag (quoted or bare).
fn attribute(tag: &str, tag_lower: &str, attr: &str) -> Option<String> {
    let mut search = 0;
    while let Some(i) = tag_lower[search..].find(attr).map(|i| search + i) {
        search = i + attr.len();
        let before_ok = i == 0 || tag_lower.as_bytes()[i - 1].is_ascii_whitespace();
        let rest = tag[search..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest.split(|c: char| c.is_ascii_whitespace() || c == '>').next().unwrap_or(""),
        };
        return Some(value.replace("&amp;", "&"));
    }
    None
}

/// Requests every resource of `page`; frames (and a refresh target) are loaded as documents
/// with their own resources, `depth` levels deep.
fn load_resources(http: &Http, page: &str, base: Option<&url::Url>, depth: u32) {
    for (link, document) in resources(page) {
        let resolved = match base {
            Some(b) => b.join(&link).ok(),
            None => url::Url::parse(&link).ok(),
        };
        let Some(url) = resolved.filter(|u| matches!(u.scheme(), "http" | "https")) else { continue };
        let referer = base.map(url::Url::as_str);
        if document {
            if let Some((sub, sub_url)) = fetch_document(http, url.as_str(), referer)
                && depth > 0
            {
                load_resources(http, &sub, Some(&sub_url), depth - 1);
            }
        } else if let Some((mut resp, _)) = http.get(&url, "*/*", referer) {
            let _ = std::io::copy(&mut resp.body_mut().as_reader().take(MAX_PAGE), &mut std::io::sink());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_cover_the_tags_a_browser_loads() {
        let page = r#"<html><body background="bg.png"><img src="/a.gif"><IMG SRC='b.jpg'>
            <script src=x.js></script><link rel="stylesheet" href="s.css?a=1&amp;b=2">
            <iframe src="http://t.example/f"></iframe><meta http-equiv="refresh" content="0; url=next.php">
            <a href="nope.html">x</a></body></html>"#;
        let got: Vec<(String, bool)> = resources(page);
        let want = [
            ("bg.png", false),
            ("/a.gif", false),
            ("b.jpg", false),
            ("x.js", false),
            ("s.css?a=1&b=2", false),
            ("http://t.example/f", true),
            ("next.php", true),
        ];
        assert_eq!(got, want.map(|(s, d)| (s.to_owned(), d)));
    }
}
