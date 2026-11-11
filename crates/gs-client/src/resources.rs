//! Resource list handling: which files must be downloaded and the `clc_fileconsistency` answer
//! (docs/re/engine-core.md §5, docs/re/net-messages.md §3.8/§6.5).

use gs_fs::FileSystem;
use gs_net::munge::{MungeTable, munge, unmunge};
use gs_protocol::MsgWriter;
use gs_protocol::clc::op;
use gs_protocol::messages::{Resource, ResourceList, resource_flags};
use md5::{Digest, Md5};

pub mod kind {
    pub const SOUND: u8 = 0;
    pub const SKIN: u8 = 1;
    pub const MODEL: u8 = 2;
    pub const DECAL: u8 = 3;
    pub const GENERIC: u8 = 4;
    pub const EVENTSCRIPT: u8 = 5;
}

/// Game path a resource refers to (`sound/` is implied for sounds).
pub fn resource_path(r: &Resource) -> String {
    if r.kind == kind::SOUND { format!("sound/{}", r.name) } else { r.name.clone() }
}

/// `CL_StartResourceDownloading` missing-file test.
pub fn is_missing(fs: &FileSystem, r: &Resource) -> bool {
    let needs_check = match r.kind {
        kind::SOUND => !r.name.starts_with('*') && !r.name.starts_with('!'),
        kind::SKIN | kind::GENERIC | kind::EVENTSCRIPT => true,
        kind::MODEL => !r.name.starts_with('*') && r.flags != 0,
        _ => false,
    };
    needs_check && !fs.exists(&resource_path(r))
}

/// `R_GetStudioBounds`: bounds of a `models/…​.mdl` file, `None` where the engine returns 0.
/// Files whose layout makes the engine read past the end are reported as failures.
pub fn studio_bounds(fs: &FileSystem, path: &str) -> Option<([f32; 3], [f32; 3])> {
    if !path.contains("models") || !path.contains(".mdl") {
        return None;
    }
    let data = fs.read(path).ok()?;
    gs_formats::studio_bounds::studio_bounds(&data)
        .inspect_err(|e| log::warn!("{path}: {e}"))
        .ok()
        .flatten()
}

/// `CL_SendConsistencyInfo`. Errors name the file the original would `Host_Error` on.
///
/// The MD5 lives in one stack buffer for the whole loop, and `MD5_Hash_File` only fills it for a
/// file that opens (`GAMECONFIG` first, then any search path) and is non-empty: a missing or
/// empty file repeats the previous entry's value (ReChecker's MISSING test relies on this). The
/// buffer's contents before the first success are whatever the stack held; zero here.
pub fn write_consistency(w: &mut MsgWriter, fs: &FileSystem, list: &ResourceList, servercount: i32) -> Result<(), String> {
    let mut digest = [0u8; 16];
    w.write_u8(op::FILECONSISTENCY);
    let len_at = w.len();
    w.write_i16(0);
    let start = w.len();
    w.start_bits();
    for &index in &list.consistency {
        let r = list
            .resources
            .get(usize::from(index))
            .ok_or_else(|| format!("consistency index {index} outside resource list"))?;
        w.write_bit(true);
        w.write_bits(u32::from(index), 12);
        let path = resource_path(r);
        let check_type = match (r.kind, r.reserved) {
            (kind::MODEL, Some(mut reserved)) if reserved.iter().any(|&b| b != 0) => {
                unmunge(&mut reserved, servercount, MungeTable::One);
                reserved[0]
            }
            _ => 0,
        };
        match check_type {
            0 => {
                let value = consistency_md5(fs, &path, &mut digest);
                log::debug!("consistency #{index} {path} (kind {}): {value:08x}", r.kind);
                w.write_bits(value, 32);
            }
            1..=3 => {
                let present = inside_game(&path) && fs.exists(&path);
                let (mins, maxs) = if check_type == 3 && !present {
                    ([-1.0; 3], [-1.0; 3])
                } else {
                    inside_game(&path).then(|| studio_bounds(fs, &path)).flatten().ok_or_else(|| format!("Unable to find {path}"))?
                };
                for v in mins.iter().chain(maxs.iter()) {
                    for b in v.to_le_bytes() {
                        w.write_bits(u32::from(b), 8);
                    }
                }
            }
            other => return Err(format!("Unknown consistency type {other}")),
        }
    }
    w.write_bit(false);
    w.end_bits();
    let len = w.len() - start;
    let bytes = w.as_bytes_mut();
    bytes[len_at..len_at + 2].copy_from_slice(&(len as i16).to_le_bytes());
    munge(&mut bytes[start..start + len], servercount, MungeTable::One);
    Ok(())
}

/// The server names consistency files, and its answer is a digest: a `..` component would let it
/// probe for and fingerprint any file on the disk (FileSystem_Stdio walks out of the search path),
/// so such a name counts as missing. docs/deviations.md "Security".
fn inside_game(path: &str) -> bool {
    !path.split(['/', '\\']).any(|part| part == "..")
}

/// `MD5_Hash_File` into the shared buffer, then its first four bytes as a little-endian u32.
fn consistency_md5(fs: &FileSystem, path: &str, digest: &mut [u8; 16]) -> u32 {
    let data = Some(path)
        .filter(|p| inside_game(p))
        .and_then(|p| fs.resolve_in(p, Some(gs_fs::PathId::GameConfig)).or_else(|_| fs.resolve(p)).ok())
        .and_then(|p| std::fs::read(p).ok())
        .filter(|d| !d.is_empty());
    if let Some(data) = data {
        *digest = Md5::digest(&data).into();
    }
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// Resources the server marks fatal-if-missing among `missing`.
pub fn fatal_missing<'a>(missing: &[&'a Resource]) -> Vec<&'a Resource> {
    missing.iter().copied().filter(|r| r.flags & resource_flags::FATAL_IF_MISSING != 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gs_fs::{PathId, SearchPath};

    #[test]
    fn missing_or_empty_files_repeat_the_previous_digest_and_gameconfig_wins() {
        let tmp = std::env::temp_dir().join(format!("gs-client-consistency-{}", std::process::id()));
        let (hd, game) = (tmp.join("cstrike_hd"), tmp.join("cstrike"));
        std::fs::create_dir_all(&hd).ok();
        std::fs::create_dir_all(&game).ok();
        std::fs::write(game.join("a.txt"), b"hello").ok();
        std::fs::write(game.join("empty.txt"), b"").ok();
        std::fs::write(hd.join("both.txt"), b"hd copy").ok();
        std::fs::write(game.join("both.txt"), b"config copy").ok();
        let sp = |dir: &std::path::Path, id| SearchPath { dir: dir.to_path_buf(), id, writable: false };
        let fs = FileSystem::new(vec![sp(&hd, PathId::Game), sp(&game, PathId::Game), sp(&game, PathId::GameConfig)]);
        let first4 = |data: &[u8]| {
            let d = Md5::digest(data);
            u32::from_le_bytes([d[0], d[1], d[2], d[3]])
        };
        let mut digest = [0u8; 16];
        assert_eq!(consistency_md5(&fs, "missing.txt", &mut digest), 0);
        assert_eq!(consistency_md5(&fs, "a.txt", &mut digest), first4(b"hello"));
        assert_eq!(consistency_md5(&fs, "missing.txt", &mut digest), first4(b"hello"));
        assert_eq!(consistency_md5(&fs, "empty.txt", &mut digest), first4(b"hello"));
        assert_eq!(consistency_md5(&fs, "both.txt", &mut digest), first4(b"config copy"));
        // A server cannot fingerprint files outside the game directories.
        std::fs::write(tmp.join("secret.txt"), b"secret").ok();
        assert_eq!(consistency_md5(&fs, "../secret.txt", &mut digest), first4(b"config copy"));
        assert_eq!(consistency_md5(&fs, "sound\\..\\..\\secret.txt", &mut digest), first4(b"config copy"));
        std::fs::remove_dir_all(&tmp).ok();
    }
}

