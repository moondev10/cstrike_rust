//! `COM_SetupDirectories` (docs/re/engine-core.md §2–3): the default and game directories from
//! the command line, and the registry's HD, add-on and low-violence switches.

use std::path::{Path, PathBuf};

use gs_fs::GameLayout;

use crate::{cmdline, registry};

/// `COM_ParseDirectoryFromCmd`: the argument after `parm` unless it is missing or another
/// switch, cut to `size - 1` bytes, one trailing slash stripped.
fn parse_directory_from_cmd(parm: &str, size: usize, default: &str) -> String {
    let argv = cmdline::argv();
    let i = cmdline::check_parm_index(parm);
    let mut d = match argv.get(i + 1) {
        Some(v) if i != 0 && !v.starts_with(['-', '+']) => v.clone(),
        _ => default.to_owned(),
    };
    let mut cut = d.len().min(size - 1);
    while !d.is_char_boundary(cut) {
        cut -= 1;
    }
    d.truncate(cut);
    if d.ends_with(['/', '\\']) {
        d.pop();
    }
    d
}

/// `ISteamApps::GetCurrentGameLanguage` read without the Steam API (the search paths are set
/// before it starts): the language picked for Half-Life in Steam (`UserConfig/language` of
/// `steamapps/appmanifest_70.acf`), else the Steam client's (`HKCU\Software\Valve\Steam\Language`),
/// else English.
fn steam_language(base: &Path) -> String {
    let manifest = base.parent().and_then(Path::parent).map(|steamapps| steamapps.join("appmanifest_70.acf"));
    let per_game = manifest.and_then(|p| std::fs::read_to_string(p).ok()).and_then(|text| {
        let kv = gs_formats::keyvalues::parse(&text);
        kv.block("AppState")?.block("UserConfig")?.string("language").map(str::to_owned)
    });
    per_game
        .filter(|l| !l.is_empty())
        .or_else(|| registry::steam_string("Language").filter(|l| !l.is_empty()))
        .unwrap_or_else(|| "english".to_owned())
}

/// The search-path inputs of `FileSystem_SetGameDirectory`.
pub fn layout(base: PathBuf) -> GameLayout {
    let default_game = parse_directory_from_cmd("-basedir", 512, "valve");
    let game = parse_directory_from_cmd("-game", 260, &default_game);
    let language = steam_language(&base);
    let mut layout = GameLayout::new(base, game);
    layout.default_game = default_game;
    layout.language = language;
    layout.low_violence = !registry::read_string("User Token 2", "").is_empty() || !registry::read_string("User Token 3", "").is_empty();
    layout.hd_models = !cmdline::check_parm("-nohdmodels") && registry::read_int("hdmodels", 0) > 0;
    layout.addons = cmdline::check_parm("-addons") || registry::read_int("addons_folder", 0) > 0;
    layout
}
