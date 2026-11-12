//! GoldSrc game filesystem.
//!
//! Content is resolved through an ordered list of directories (first hit wins), e.g.
//! `cstrike_downloads`, `cstrike`, `valve`. Game-relative paths use `/` or `\` and are
//! matched case-insensitively, like the original engine on every platform.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard};

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("invalid game path {0:?}")]
    InvalidPath(String),
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Environment variable overriding the Half-Life install directory.
pub const BASEDIR_ENV: &str = "GS_BASEDIR";
const STEAM_DEFAULT: &str = "C:/Program Files (x86)/Steam/steamapps/common/Half-Life";

/// Half-Life install directory: `$GS_BASEDIR`, else Steam's default location if present.
pub fn default_base_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(BASEDIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    steam_defaults().into_iter().find(|p| p.is_dir())
}

/// Where Steam installs Half-Life: its Windows default, or the Linux Steam roots in `$HOME`.
fn steam_defaults() -> Vec<PathBuf> {
    if cfg!(windows) {
        return vec![PathBuf::from(STEAM_DEFAULT)];
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    [".steam/steam", ".local/share/Steam"].iter().map(|root| home.join(root).join("steamapps/common/Half-Life")).collect()
}

/// Path IDs the engine tags search paths with; reads may be restricted to one ID and writes go
/// to the first writable path with the requested ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathId {
    Game,
    GameConfig,
    GameDownload,
    GameFallback,
    Base,
    DefaultGame,
    Platform,
    /// `CONFIG`: `platform\config`, mounted by GameUI (`InGameDialogConfig.vdf`,
    /// `ServerBrowser.vdf`).
    Config,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPath {
    pub dir: PathBuf,
    pub id: PathId,
    pub writable: bool,
}

/// Inputs of `FileSystem_SetGameDirectory` (docs/re/engine-core.md §1.3).
#[derive(Debug, Clone)]
pub struct GameLayout {
    /// Steam `Half-Life` directory.
    pub base_dir: PathBuf,
    /// Mod directory, e.g. `cstrike`.
    pub game: String,
    /// Default game directory (`valve`).
    pub default_game: String,
    /// Steam UI language; `english` (or empty) mounts no language directories.
    pub language: String,
    /// Mount `<dir>_hd` high-definition model packs.
    pub hd_models: bool,
    /// Mount `<dir>_addon` user content.
    pub addons: bool,
    /// Mount `<dir>_lv` low-violence content.
    pub low_violence: bool,
}

impl GameLayout {
    pub fn new(base_dir: impl Into<PathBuf>, game: impl Into<String>) -> Self {
        Self {
            base_dir: base_dir.into(),
            game: game.into(),
            default_game: "valve".to_owned(),
            language: "english".to_owned(),
            hd_models: false,
            addons: false,
            low_violence: false,
        }
    }

    fn localized(&self) -> bool {
        !self.language.is_empty() && !self.language.eq_ignore_ascii_case("english")
    }

    /// Search paths in lookup order, exactly as the engine mounts them. Directories need not exist.
    pub fn search_paths(&self) -> Vec<SearchPath> {
        let mut out = Vec::new();
        let mut add = |name: String, id, writable| out.push(SearchPath { dir: self.base_dir.join(name), id, writable });
        let (m, d, lang) = (self.game.as_str(), self.default_game.as_str(), self.language.as_str());
        let localized = self.localized();
        if self.low_violence {
            add(format!("{m}_lv"), PathId::Game, false);
        }
        if self.addons {
            add(format!("{m}_addon"), PathId::Game, false);
        }
        if localized {
            add(format!("{m}_{lang}"), PathId::Game, false);
        }
        if self.hd_models {
            add(format!("{m}_hd"), PathId::Game, false);
        }
        add(m.to_owned(), PathId::Game, true);
        add(m.to_owned(), PathId::GameConfig, true);
        add(format!("{m}_downloads"), PathId::GameDownload, true);
        let mut visited = vec![m.to_ascii_lowercase()];
        let mut current = m.to_owned();
        while let Some(fallback) = liblist_fallback_dir(&self.base_dir.join(&current)) {
            if visited.contains(&fallback.to_ascii_lowercase()) {
                break;
            }
            visited.push(fallback.to_ascii_lowercase());
            if self.low_violence {
                add(format!("{fallback}_lv"), PathId::GameFallback, false);
            }
            if self.addons {
                add(format!("{fallback}_addon"), PathId::GameFallback, false);
            }
            if localized {
                add(format!("{fallback}_{lang}"), PathId::GameFallback, true);
            }
            if self.hd_models {
                add(format!("{fallback}_hd"), PathId::GameFallback, false);
            }
            add(fallback.clone(), PathId::GameFallback, true);
            current = fallback;
        }
        // The engine nests the default-game variants under the language check (quirk preserved).
        if localized {
            if self.low_violence {
                add(format!("{d}_lv"), PathId::DefaultGame, false);
            }
            if self.addons {
                add(format!("{d}_addon"), PathId::DefaultGame, false);
            }
            add(format!("{d}_{lang}"), PathId::DefaultGame, false);
        }
        if self.hd_models {
            add(format!("{d}_hd"), PathId::DefaultGame, false);
        }
        out.push(SearchPath { dir: self.base_dir.clone(), id: PathId::Base, writable: true });
        out.push(SearchPath { dir: self.base_dir.join(d), id: PathId::DefaultGame, writable: false });
        out.push(SearchPath { dir: self.base_dir.join("platform"), id: PathId::Platform, writable: true });
        out
    }
}

/// `fallback_dir "<dir>"` from `<game_dir>/liblist.gam`: first line starting with the key, text
/// between the first two quotes, 1..=126 chars.
fn liblist_fallback_dir(game_dir: &Path) -> Option<String> {
    let text = fs::read(game_dir.join("liblist.gam")).ok()?;
    let text = String::from_utf8_lossy(&text);
    let line = text.lines().find(|l| l.get(..12).is_some_and(|k| k.eq_ignore_ascii_case("fallback_dir")))?;
    let value = line.split('"').nth(1)?;
    let valid = (1..=126).contains(&value.len()) && !value.contains(['/', '\\', ':']) && value != "..";
    valid.then(|| value.to_owned())
}

/// The game file system. The search list is shared by every clone, so a runtime mount
/// (`svc_sendextrainfo`'s fallback directory) reaches all users.
#[derive(Debug, Clone)]
pub struct FileSystem {
    search: Arc<RwLock<Vec<SearchPath>>>,
    /// The install directory, where `COM_AddDefaultDir` mounts relative names.
    base_dir: Option<PathBuf>,
    /// The Steam language when it is not English.
    language: Option<String>,
}

impl FileSystem {
    pub fn new(search: Vec<SearchPath>) -> Self {
        Self { search: Arc::new(RwLock::new(search)), base_dir: None, language: None }
    }

    pub fn from_layout(layout: &GameLayout) -> Self {
        Self {
            search: Arc::new(RwLock::new(layout.search_paths())),
            base_dir: Some(layout.base_dir.clone()),
            language: layout.localized().then(|| layout.language.clone()),
        }
    }

    /// The game language when it is not English (`ISteamApps::GetCurrentGameLanguage`).
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// `ILocalize::AddFile`'s `%language%` expansion (vgui2-core.md §5.2): the English file,
    /// then the game language's, whose tokens override it.
    pub fn language_files(&self, pattern: &str) -> Vec<String> {
        let mut files = vec![pattern.replace("%language%", "english")];
        files.extend(self.language.as_deref().map(|l| pattern.replace("%language%", l)));
        files
    }

    fn paths(&self) -> RwLockReadGuard<'_, Vec<SearchPath>> {
        self.search.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn search_paths(&self) -> Vec<SearchPath> {
        self.paths().clone()
    }

    /// `FileSystem_SetGameDirectory` again (`_sethdmodels`/`_setaddons_folder`):
    /// `RemoveAllSearchPaths`, then the layout's paths; paths added since (fallback game
    /// directories, GameUI's) are gone as in the original.
    pub fn remount(&self, layout: &GameLayout) {
        *self.search.write().unwrap_or_else(PoisonError::into_inner) = layout.search_paths();
    }

    /// `COM_AddDefaultDir` → `FileSystem_AddFallbackGameDir`: `<dir>_<language>` (not English)
    /// and `<dir>` under the install directory appended as writable `GAME` paths. The original
    /// mounts the server's string unchecked; empty names, `..`, `/`, `\` and `:` are refused.
    pub fn add_fallback_game_dir(&self, dir: &str) -> bool {
        let valid = !dir.is_empty() && !dir.contains("..") && !dir.contains(['/', '\\', ':']);
        let Some(base) = self.base_dir.as_ref().filter(|_| valid) else { return false };
        let mut search = self.search.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(lang) = &self.language {
            search.push(SearchPath { dir: base.join(format!("{dir}_{lang}")), id: PathId::Game, writable: true });
        }
        search.push(SearchPath { dir: base.join(dir), id: PathId::Game, writable: true });
        true
    }

    /// `AddSearchPath(path, pathID)` with `path` relative to the install directory (the working
    /// directory), appended as a writable path and created if missing (`CGameUI::Start`'s
    /// `_mkdir`).
    pub fn add_search_path(&self, path: &str, id: PathId) {
        let Some(base) = self.base_dir.as_ref() else { return };
        let dir = base.join(path);
        // `_mkdir`'s result is not checked.
        let _ = fs::create_dir_all(&dir);
        let mut search = self.search.write().unwrap_or_else(PoisonError::into_inner);
        if !search.iter().any(|s| s.id == id && s.dir == dir) {
            search.push(SearchPath { dir, id, writable: true });
        }
    }

    /// The mod directory (`com_gamedir` under the install directory): the writable `GAME`
    /// path, where client code `fopen`s its own files.
    pub fn game_dir(&self) -> Option<PathBuf> {
        let paths = self.paths();
        let game = |w: bool| paths.iter().find(|p| p.id == PathId::Game && (p.writable || !w));
        game(true).or_else(|| game(false)).map(|p| p.dir.clone())
    }

    /// Absolute path of the highest-priority file matching `game_path`, optionally restricted to one path ID.
    pub fn resolve_in(&self, game_path: &str, id: Option<PathId>) -> Result<PathBuf, FsError> {
        let parts = split_game_path(game_path)?;
        self.paths()
            .iter()
            .filter(|sp| id.is_none_or(|id| sp.id == id))
            .find_map(|sp| find_case_insensitive(&sp.dir, &parts))
            .ok_or_else(|| FsError::NotFound(game_path.to_owned()))
    }

    pub fn resolve(&self, game_path: &str) -> Result<PathBuf, FsError> {
        self.resolve_in(game_path, None)
    }

    pub fn exists(&self, game_path: &str) -> bool {
        self.resolve(game_path).is_ok()
    }

    pub fn read(&self, game_path: &str) -> Result<Vec<u8>, FsError> {
        let path = self.resolve(game_path)?;
        fs::read(&path).map_err(|source| FsError::Io { path, source })
    }

    /// The path `game_path` gets in the first writable search path with `id` (`FS_OpenPathID`
    /// for writing), with its directories created.
    pub fn write_path(&self, game_path: &str, id: PathId) -> Result<PathBuf, FsError> {
        let parts = split_game_path(game_path)?;
        // Writes stay inside the search path (server-supplied download names arrive here).
        if parts.contains(&"..") {
            return Err(FsError::InvalidPath(game_path.to_owned()));
        }
        let root = self
            .paths()
            .iter()
            .find(|sp| sp.id == id && sp.writable)
            .map(|sp| sp.dir.clone())
            .ok_or_else(|| FsError::NotFound(format!("writable {id:?} path for {game_path}")))?;
        let path = parts.iter().fold(root, |p, part| p.join(part));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| FsError::Io { path: path.clone(), source })?;
        }
        Ok(path)
    }

    /// Writes into the first writable search path with `id`, creating directories as needed.
    pub fn write(&self, game_path: &str, id: PathId, data: &[u8]) -> Result<PathBuf, FsError> {
        let path = self.write_path(game_path, id)?;
        fs::write(&path, data).map_err(|source| FsError::Io { path: path.clone(), source })?;
        Ok(path)
    }

    /// The install directory: the original's working directory, which its CRT file calls
    /// (`_unlink`, `rename`) resolve against.
    pub fn base_dir(&self) -> Option<&Path> {
        self.base_dir.as_deref()
    }

    /// Game paths (relative, `/`-separated, original case) of files in `dir` with extension `ext`,
    /// merged over all search paths, sorted and de-duplicated case-insensitively.
    pub fn list(&self, dir: &str, ext: &str) -> Result<Vec<String>, FsError> {
        let parts = split_game_path(dir)?;
        let mut out: Vec<String> = Vec::new();
        for sp in self.paths().iter() {
            let Some(abs) = find_case_insensitive(&sp.dir, &parts) else { continue };
            let Ok(entries) = fs::read_dir(&abs) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let matches = Path::new(&name)
                    .extension()
                    .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext));
                if matches && entry.path().is_file() {
                    let mut game_path = parts.join("/");
                    if !game_path.is_empty() {
                        game_path.push('/');
                    }
                    game_path.push_str(&name);
                    out.push(game_path);
                }
            }
        }
        out.sort_by_key(|p| p.to_ascii_lowercase());
        out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Ok(out)
    }

    /// `FindFirst`/`FindNext` (FileSystem_Stdio): `pattern` is `dir/<wildcard>`; every search
    /// path (with `id`, or all) in order, each directory in the OS listing order, so a name
    /// present in several paths repeats. Returns the names (no directory) and whether each is
    /// a directory.
    pub fn find(&self, pattern: &str, id: Option<PathId>) -> Vec<(String, bool)> {
        let Ok(parts) = split_game_path(pattern) else { return Vec::new() };
        let Some((wild, dir)) = parts.split_last() else { return Vec::new() };
        let mut out = Vec::new();
        for sp in self.paths().iter().filter(|sp| id.is_none_or(|id| sp.id == id)) {
            let Some(abs) = find_case_insensitive(&sp.dir, dir) else { continue };
            let Ok(entries) = fs::read_dir(&abs) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if wildcard_match(wild, &name) {
                    out.push((name, entry.path().is_dir()));
                }
            }
        }
        out
    }

    /// `RemoveFile(path, pathID)`: the file in the first search path (with `id`, or any) that
    /// holds it is deleted; `false` when none does.
    pub fn remove(&self, game_path: &str, id: Option<PathId>) -> bool {
        self.resolve_in(game_path, id).is_ok_and(|p| fs::remove_file(p).is_ok())
    }
}

/// Win32 wildcard match (`*`, `?`), case-insensitive; `*.*` matches every name.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    if pattern == "*.*" || pattern == "*" {
        return true;
    }
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let n: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// The components of a relative game path. FileSystem_Stdio joins each search path and the
/// name, so `..` walks out of the search path (`../demoplayer.dll` from `cstrike/` is the
/// install directory's file); absolute paths and drive letters are refused.
fn split_game_path(game_path: &str) -> Result<Vec<&str>, FsError> {
    if game_path.contains(':') || game_path.starts_with(['/', '\\']) {
        return Err(FsError::InvalidPath(game_path.to_owned()));
    }
    Ok(game_path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect())
}

fn find_case_insensitive(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let direct: PathBuf = parts.iter().fold(root.to_path_buf(), |p, part| p.join(part));
    if direct.exists() {
        return Some(direct);
    }
    // Windows file systems already match names case-insensitively, so a direct miss is final
    // and listing directories would only cost time (every search path a file is not in).
    if cfg!(windows) {
        return None;
    }
    let mut current = root.to_path_buf();
    for part in parts {
        if *part == ".." {
            current.pop();
            continue;
        }
        let entry = fs::read_dir(&current)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        current = entry.path();
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_absolute_paths() {
        for bad in ["/etc/passwd", "c:/windows", "\\x"] {
            assert!(matches!(split_game_path(bad), Err(FsError::InvalidPath(_))), "{bad}");
        }
    }

    #[test]
    fn parent_components_resolve_for_reads_but_not_writes() {
        let tmp = std::env::temp_dir().join(format!("gs-fs-parent-{}", std::process::id()));
        let game = tmp.join("cstrike");
        fs::create_dir_all(&game).ok();
        fs::write(tmp.join("DemoPlayer.dll"), b"module").ok();
        let fs_ = FileSystem::new(vec![SearchPath { dir: game.clone(), id: PathId::Game, writable: true }]);
        assert_eq!(fs_.read("../demoplayer.dll").ok().as_deref(), Some(&b"module"[..]));
        assert!(matches!(fs_.write("../evil.cfg", PathId::Game, b"x"), Err(FsError::InvalidPath(_))));
        assert!(!tmp.join("evil.cfg").exists());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn normalizes_separators() {
        assert_eq!(split_game_path("maps\\de_dust2.bsp").ok(), Some(vec!["maps", "de_dust2.bsp"]));
        assert_eq!(split_game_path("./gfx//env/").ok(), Some(vec!["gfx", "env"]));
    }

    #[test]
    fn earlier_search_path_wins_and_case_is_ignored() {
        let tmp = std::env::temp_dir().join(format!("gs-fs-test-{}", std::process::id()));
        let high = tmp.join("cstrike_downloads");
        let low = tmp.join("cstrike");
        fs::create_dir_all(high.join("Maps")).ok();
        fs::create_dir_all(low.join("maps")).ok();
        fs::write(high.join("Maps").join("A.bsp"), b"high").ok();
        fs::write(low.join("maps").join("a.bsp"), b"low").ok();
        fs::write(low.join("maps").join("b.bsp"), b"only-low").ok();

        let path = |dir: &PathBuf, id| SearchPath { dir: dir.clone(), id, writable: true };
        let fs_ = FileSystem::new(vec![path(&high, PathId::GameDownload), path(&low, PathId::Game)]);
        assert_eq!(fs_.read("maps/a.BSP").ok().as_deref(), Some(&b"high"[..]));
        assert_eq!(fs_.read("MAPS\\b.bsp").ok().as_deref(), Some(&b"only-low"[..]));
        assert!(matches!(fs_.read("maps/c.bsp"), Err(FsError::NotFound(_))));
        let listed = fs_.list("maps", "bsp").unwrap_or_default();
        assert_eq!(listed, vec!["maps/A.bsp".to_owned(), "maps/b.bsp".to_owned()]);
        assert_eq!(fs_.resolve_in("maps/a.bsp", Some(PathId::Game)).ok(), Some(low.join("maps").join("a.bsp")));
        let written = fs_.write("maps/new.bsp", PathId::GameDownload, b"dl").ok();
        assert_eq!(written, Some(high.join("maps").join("new.bsp")));
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn english_layout_matches_engine_order() {
        let layout = GameLayout { hd_models: true, ..GameLayout::new("/nonexistent/hl", "cstrike") };
        let order: Vec<(String, PathId)> = layout
            .search_paths()
            .into_iter()
            .map(|sp| (sp.dir.strip_prefix("/nonexistent/hl").map_or(String::new(), |p| p.display().to_string()), sp.id))
            .collect();
        let expect = [
            ("cstrike_hd", PathId::Game),
            ("cstrike", PathId::Game),
            ("cstrike", PathId::GameConfig),
            ("cstrike_downloads", PathId::GameDownload),
            ("valve_hd", PathId::DefaultGame),
            ("", PathId::Base),
            ("valve", PathId::DefaultGame),
            ("platform", PathId::Platform),
        ];
        assert_eq!(order, expect.map(|(d, id)| (d.to_owned(), id)));
    }

    #[test]
    fn localized_layout_adds_language_dirs_in_engine_positions() {
        let layout = GameLayout { language: "russian".to_owned(), ..GameLayout::new("/hl", "cstrike") };
        let names: Vec<String> =
            layout.search_paths().iter().map(|sp| sp.dir.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())).collect();
        assert_eq!(names, ["cstrike_russian", "cstrike", "cstrike", "cstrike_downloads", "valve_russian", "hl", "valve", "platform"]);
    }
}
