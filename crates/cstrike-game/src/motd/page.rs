//! The MOTD page in WebView2 (current Edge Chromium, sandboxed) instead of chromehtml's CEF 18:
//! a child window over the panel's `Message` control, locked down (requested change,
//! deviations.md): no script, no sound, every request through [`super::proxy`], `http(s)`
//! navigations only, no popups, downloads, permission or login prompts, context menu, browser
//! shortcuts or external protocols.

use std::path::PathBuf;
use std::sync::LazyLock;

use winit::window::Window;
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{NewWindowResponse, PermissionResponse, Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

use super::USER_AGENT;
use super::proxy::Proxy;

/// One proxy for the process: WebView2 keeps the browser arguments (the proxy's port) of the
/// first environment made on a data folder.
static PROXY: LazyLock<Option<Proxy>> = LazyLock::new(|| Proxy::start().map_err(|e| log::warn!("MOTD proxy: {e}")).ok());

pub struct Page {
    webview: WebView,
    _context: WebContext,
    /// The rectangle shown (window pixels); `None` while hidden.
    shown: Option<(i32, i32, i32, i32)>,
}

impl Page {
    /// A hidden page over `window`, its profile (cookies, cache) in `data` (CEF's `htmlcache`).
    /// Without the proxy there is no page.
    pub fn new(window: &Window, data: PathBuf) -> Result<Self, String> {
        let proxy = PROXY.as_ref().ok_or("no proxy")?;
        let mut context = WebContext::new(Some(data));
        // wry's defaults (no Edge mini menu, PDF UI or SmartScreen calls), the proxy for every
        // host, sound off, and Chrome 18's Accept-Language.
        let args = format!(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --proxy-server=http://127.0.0.1:{} --proxy-bypass-list=<-loopback> --mute-audio --accept-lang=en-US,en",
            proxy.port()
        );
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_visible(false)
            .with_focused(false)
            .with_user_agent(USER_AGENT)
            .with_javascript_disabled()
            .with_devtools(false)
            .with_hotkeys_zoom(false)
            .with_clipboard(false)
            .with_autoplay(false)
            .with_general_autofill_enabled(false)
            .with_navigation_handler(|url| navigation_allowed(&url))
            // BrowserPopupHTMLWindow / OnOpenNewTab open nothing.
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|url, _| {
                log::warn!("MOTD: refusing to download {url}");
                false
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_additional_browser_args(args)
            .with_browser_accelerator_keys(false)
            .with_default_context_menus(false)
            .build_as_child(window)
            .map_err(|e| e.to_string())?;
        crate::ffi::lock_down_webview(&webview.webview(), &webview.controller(), navigation_allowed).map_err(|e| e.to_string())?;
        Ok(Self { webview, _context: context, shown: None })
    }

    /// `OpenURL(url)`.
    pub fn load_url(&mut self, url: &str, server_local: bool) {
        set_server_local(server_local);
        if let Err(e) = self.webview.load_url(url) {
            log::debug!("MOTD {url}: {e}");
        }
    }

    /// The `motd_temp.html` page, loaded from the string: nothing it names relative to the game
    /// directory is read.
    pub fn load_html(&mut self, html: &str, server_local: bool) {
        set_server_local(server_local);
        if let Err(e) = self.webview.load_html(html) {
            log::debug!("MOTD page: {e}");
        }
    }

    /// Shows the page at `rect` (window pixels) or hides it.
    pub fn place(&mut self, rect: Option<(i32, i32, i32, i32)>) {
        if rect == self.shown {
            return;
        }
        let res = match rect {
            Some((x, y, w, h)) => {
                let bounds = Rect { position: PhysicalPosition::new(x, y).into(), size: PhysicalSize::new(w.max(1) as u32, h.max(1) as u32).into() };
                self.webview.set_bounds(bounds).and_then(|()| if self.shown.is_none() { self.webview.set_visible(true) } else { Ok(()) })
            }
            None => self.webview.set_visible(false),
        };
        if let Err(e) = res {
            log::debug!("MOTD page placement: {e}");
        }
        self.shown = rect;
    }

    /// Keyboard focus back to the game window.
    pub fn focus_game(&self) {
        if let Err(e) = self.webview.focus_parent() {
            log::debug!("MOTD page focus: {e}");
        }
    }
}

fn set_server_local(server_local: bool) {
    if let Some(proxy) = PROXY.as_ref() {
        proxy.set_server_local(server_local);
    }
}

/// Where the page and its frames may navigate: the web, and the blank and string documents the
/// panel loads; not `file:`, `steam:` or any other application's protocol.
fn navigation_allowed(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or_default();
    let allowed = ["http", "https", "about", "data"].iter().any(|s| scheme.eq_ignore_ascii_case(s));
    if !allowed {
        log::warn!("MOTD: refusing to open {url}");
    }
    allowed
}
