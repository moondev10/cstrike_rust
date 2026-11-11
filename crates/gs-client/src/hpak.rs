//! `custom.hpk` (docs/re/engine-core.md §5.8): spray-logo lumps keyed by MD5 in an HPAK
//! version 1 file (header, lumps back to back, a directory of `resource_t` + offset + length
//! sorted by MD5), and the in-memory queue of received logos (`gp_hpak_queue`).

use gs_fs::{FileSystem, PathId};
use gs_protocol::messages::Resource;
use md5::{Digest, Md5};

/// The client's pack (`HPAK_*("custom.hpk")`).
pub const CUSTOM_HPK: &str = "custom.hpk";
/// `HPAK_AddLump`'s size limits.
const MIN_LUMP: u32 = 1024;
const MAX_LUMP: u32 = 131_072;
/// A directory entry: `resource_t` (136 bytes) + offset + length.
const ENTRY_SIZE: usize = 144;
const RESOURCE_SIZE: usize = 136;

/// `resource_t` as stored on disk (pointers zeroed).
fn resource_bytes(r: &Resource) -> [u8; RESOURCE_SIZE] {
    let mut b = [0u8; RESOURCE_SIZE];
    let name = gs_common::bytestr::encode(&r.name);
    let n = name.len().min(63);
    b[..n].copy_from_slice(&name[..n]);
    b[64..68].copy_from_slice(&i32::from(r.kind).to_le_bytes());
    b[68..72].copy_from_slice(&i32::from(r.index).to_le_bytes());
    b[72..76].copy_from_slice(&r.download_size.to_le_bytes());
    b[76] = r.flags;
    b[77..93].copy_from_slice(&r.md5.unwrap_or_default());
    if let Some(reserved) = r.reserved {
        b[94..126].copy_from_slice(&reserved);
    }
    b
}

fn parse_resource(b: &[u8]) -> Option<Resource> {
    let b = b.get(..RESOURCE_SIZE)?;
    let name_end = b[..64].iter().position(|&c| c == 0).unwrap_or(64);
    let i32_at = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut md5 = [0u8; 16];
    md5.copy_from_slice(&b[77..93]);
    Some(Resource {
        kind: i32_at(64) as u8,
        name: gs_common::bytestr::decode(&b[..name_end]),
        index: i32_at(68) as u16,
        download_size: i32_at(72) as u32,
        flags: b[76],
        md5: Some(md5),
        reserved: None,
    })
}

/// One directory entry.
#[derive(Debug, Clone)]
struct Entry {
    resource: Resource,
    offset: u32,
    length: u32,
}

/// The directory of an HPAK image (`None` unless "HPAK", version 1 and 1..32768 entries).
fn read_directory(data: &[u8]) -> Option<Vec<Entry>> {
    if data.get(..4)? != b"HPAK" {
        return None;
    }
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if u32_at(4)? != 1 {
        return None;
    }
    let dir = u32_at(8)? as usize;
    let count = u32_at(dir)? as usize;
    if !(1..=32768).contains(&count) {
        return None;
    }
    (0..count)
        .map(|i| {
            let at = dir + 4 + i * ENTRY_SIZE;
            let resource = parse_resource(data.get(at..at + RESOURCE_SIZE)?)?;
            Some(Entry { resource, offset: u32_at(at + RESOURCE_SIZE)?, length: u32_at(at + RESOURCE_SIZE + 4)? })
        })
        .collect()
}

fn lump_data<'a>(data: &'a [u8], e: &Entry) -> Option<&'a [u8]> {
    data.get(e.offset as usize..(e.offset as usize).checked_add(e.length as usize)?)
}

fn md5_of(data: &[u8]) -> [u8; 16] {
    Md5::digest(data).into()
}

/// The MD5 as the 32 lowercase hex digits of `!MD5…` names.
pub fn md5_hex(md5: &[u8; 16]) -> String {
    md5.iter().map(|b| format!("{b:02x}")).collect()
}

/// `!MD5<hex>` back to the digest.
pub fn md5_from_name(name: &str) -> Option<[u8; 16]> {
    let hex = name.strip_prefix("!MD5")?;
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// `HPAK_*` over `custom.hpk` plus the queue of logos received this session.
#[derive(Debug, Default)]
pub struct Hpak {
    queue: Vec<(Resource, Vec<u8>)>,
}

impl Hpak {
    /// `HPAK_GetDataPointer`: the lump with `md5`, from the queue or the file.
    pub fn get(&self, fs: &FileSystem, md5: &[u8; 16]) -> Option<Vec<u8>> {
        if let Some((_, data)) = self.queue.iter().find(|(r, _)| r.md5.as_ref() == Some(md5)) {
            return Some(data.clone());
        }
        let data = fs.read(CUSTOM_HPK).ok()?;
        let dir = read_directory(&data)?;
        let e = dir.iter().find(|e| e.resource.md5.as_ref() == Some(md5))?;
        lump_data(&data, e).map(<[u8]>::to_vec)
    }

    /// The stored resource of the lump with `md5` (`HPAK_ResourceForHash`).
    pub fn resource(&self, fs: &FileSystem, md5: &[u8; 16]) -> Option<Resource> {
        if let Some((r, _)) = self.queue.iter().find(|(r, _)| r.md5.as_ref() == Some(md5)) {
            return Some(r.clone());
        }
        let data = fs.read(CUSTOM_HPK).ok()?;
        read_directory(&data)?.into_iter().find(|e| e.resource.md5.as_ref() == Some(md5)).map(|e| e.resource)
    }

    /// `HPAK_AddLump`'s checks: 1024..131072 bytes and the data's MD5 matching the resource.
    fn valid_lump(resource: &Resource, data: &[u8]) -> bool {
        (MIN_LUMP..=MAX_LUMP).contains(&resource.download_size) && resource.md5 == Some(md5_of(data))
    }

    /// `HPAK_AddLump(queue = 1)`: kept in memory until the next flush.
    pub fn add_queued(&mut self, resource: &Resource, data: &[u8]) -> bool {
        if !Self::valid_lump(resource, data) {
            log::warn!("HPAK_AddLump called with bogus lump for {}", resource.name);
            return false;
        }
        if !self.queue.iter().any(|(r, _)| r.md5 == resource.md5) {
            self.queue.push((resource.clone(), data.to_vec()));
        }
        true
    }

    /// `HPAK_FlushHostQueue`: the queued lumps written into the file.
    pub fn flush(&mut self, fs: &FileSystem) {
        for (r, data) in std::mem::take(&mut self.queue) {
            if let Err(e) = Self::add(fs, &r, &data) {
                log::warn!("{CUSTOM_HPK}: {e}");
            }
        }
    }

    /// `HPAK_AddLump(queue = 0)`: the lump appended and the directory rewritten sorted by MD5
    /// (`HPAK_CreatePak` without a file); an MD5 already stored is left alone.
    pub fn add(fs: &FileSystem, resource: &Resource, data: &[u8]) -> Result<(), String> {
        if !Self::valid_lump(resource, data) {
            return Err(format!("HPAK_AddLump called with bogus lump for {}", resource.name));
        }
        let md5 = resource.md5.unwrap_or_default();
        let old = fs.read(CUSTOM_HPK).ok();
        let (mut out, mut entries) = match old.as_deref().and_then(|d| read_directory(d).map(|dir| (d, dir))) {
            Some((d, dir)) => {
                if dir.iter().any(|e| e.resource.md5 == Some(md5)) {
                    return Ok(());
                }
                let dir_offset = u32::from_le_bytes([d[8], d[9], d[10], d[11]]) as usize;
                (d[..dir_offset.min(d.len())].to_vec(), dir)
            }
            None => {
                let mut header = b"HPAK".to_vec();
                header.extend_from_slice(&1u32.to_le_bytes());
                header.extend_from_slice(&0u32.to_le_bytes());
                (header, Vec::new())
            }
        };
        let entry = Entry { resource: resource.clone(), offset: out.len() as u32, length: data.len() as u32 };
        out.extend_from_slice(data);
        let at = entries.iter().position(|e| e.resource.md5.unwrap_or_default() > md5).unwrap_or(entries.len());
        entries.insert(at, entry);
        let dir_offset = out.len() as u32;
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for e in &entries {
            out.extend_from_slice(&resource_bytes(&e.resource));
            out.extend_from_slice(&e.offset.to_le_bytes());
            out.extend_from_slice(&e.length.to_le_bytes());
        }
        out[8..12].copy_from_slice(&dir_offset.to_le_bytes());
        fs.write(CUSTOM_HPK, PathId::GameConfig, &out).map(|_| ()).map_err(|e| e.to_string())
    }

    fn remove_file(fs: &FileSystem) {
        if let Ok(path) = fs.write_path(CUSTOM_HPK, PathId::GameConfig)
            && path.exists()
            && let Err(e) = std::fs::remove_file(&path)
        {
            log::warn!("{}: {e}", path.display());
        }
    }

    /// `HPAK_CheckIntegrity` / `HPAK_ValidatePak`: every lump 1..131071 bytes and matching its
    /// MD5, else the whole file is deleted.
    pub fn check_integrity(fs: &FileSystem) {
        let Ok(data) = fs.read(CUSTOM_HPK) else { return };
        let valid = read_directory(&data).is_some_and(|dir| {
            dir.iter().all(|e| (1..MAX_LUMP).contains(&e.length) && lump_data(&data, e).is_some_and(|d| e.resource.md5 == Some(md5_of(d))))
        });
        if !valid {
            log::warn!("{CUSTOM_HPK} is corrupt, removing");
            Self::remove_file(fs);
        }
    }

    /// `HPAK_CheckSize`: the file is deleted once it reaches `hpk_maxsize` MB (0 disables).
    pub fn check_size(fs: &FileSystem, max_mb: f32) {
        if max_mb <= 0.0 {
            return;
        }
        let Ok(path) = fs.resolve(CUSTOM_HPK) else { return };
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        if size as f64 >= f64::from(max_mb) * 1_000_000.0 {
            log::info!("{CUSTOM_HPK} is {size} bytes, over hpk_maxsize; deleting");
            Self::remove_file(fs);
        }
    }

    /// `upload !MD5…`: the lump, after `cmd_upload`'s checks of the stored MD5s (a mismatch
    /// removes the lump; here the whole file is validated instead).
    pub fn upload_data(&self, fs: &FileSystem, md5: &[u8; 16]) -> Option<Vec<u8>> {
        let data = self.get(fs, md5)?;
        (md5_of(&data) == *md5).then_some(data)
    }

    /// `hpklist`, `hpkremove`, `hpkval` and `hpkextract` (after the caller's queue flush):
    /// the console text they print.
    pub fn command(fs: &FileSystem, name: &str, argv: &[&str]) -> String {
        let arg = |i: usize| argv.get(i).copied().unwrap_or("");
        let pak = default_ext(arg(1));
        let mut out = String::new();
        match name {
            "hpklist" => {
                out.push_str(&format!("Contents for {pak}.\n"));
                let Some((_, dir)) = open_pak(fs, &pak, "HPAK_List:  version mismatch\n", &mut out) else { return out };
                out.push_str(&format!("# of Entries:  {}\n# Type Size FileName : MD5 Hash\n", dir.len()));
                for (k, e) in dir.iter().enumerate() {
                    out.push_str(&format!("{}: {}\n  :  {}\n", k + 1, entry_columns(e), md5_hex(&e.resource.md5.unwrap_or_default())));
                }
            }
            "hpkremove" => {
                if argv.len() != 3 {
                    return "Usage:  hpkremove <hpk> <index>\n".to_owned();
                }
                let index = gs_console_atoi(arg(2));
                let Some((data, dir)) = open_pak(fs, &pak, "HPAK_List:  version mismatch\n", &mut out) else {
                    out.push_str(&format!("Could not locate resource {index} in {}\n", arg(1)));
                    return out;
                };
                if index < 1 || index as usize > dir.len() {
                    out.push_str(&format!("ERROR: HPAK bogus directory entry request:  {index}\nCould not locate resource {index} in {}\n", arg(1)));
                    return out;
                }
                let target = dir[index as usize - 1].resource.clone();
                if dir.len() == 1 {
                    out.push_str(&format!("Removing final lump from HPAK, deleting HPAK:\n  {pak}\n"));
                    fs.remove(&pak, None);
                    return out;
                }
                out.push_str(&format!("Removing {} from HPAK {pak}.\n", target.name));
                let mut file = data[..12].to_vec();
                let mut kept = Vec::new();
                for e in dir.iter().filter(|e| e.resource.md5 != target.md5) {
                    let lump = lump_data(&data, e).unwrap_or_default();
                    kept.push(Entry { resource: e.resource.clone(), offset: file.len() as u32, length: e.length });
                    file.extend_from_slice(lump);
                }
                let dir_offset = file.len() as u32;
                file.extend_from_slice(&(kept.len() as u32).to_le_bytes());
                for e in &kept {
                    file.extend_from_slice(&resource_bytes(&e.resource));
                    file.extend_from_slice(&e.offset.to_le_bytes());
                    file.extend_from_slice(&e.length.to_le_bytes());
                }
                file[8..12].copy_from_slice(&dir_offset.to_le_bytes());
                if fs.write(&pak, PathId::GameConfig, &file).is_err() {
                    out.push_str(&format!("ERROR: couldn't create {}.hp2.\n", pak.rsplit_once('.').map_or(pak.as_str(), |(b, _)| b)));
                }
            }
            "hpkval" => {
                if argv.len() != 2 {
                    return "Usage:  hpkval hpkname\n".to_owned();
                }
                out.push_str(&format!("Validating {pak}.\n"));
                let Some((data, dir)) = open_pak(fs, &pak, "hpkval:  version mismatch\n", &mut out) else { return out };
                out.push_str(&format!("# of Entries:  {}\n# Type Size FileName : MD5 Hash\n", dir.len()));
                for (k, e) in dir.iter().enumerate() {
                    out.push_str(&format!("{}: {}:  ", k + 1, entry_columns(e)));
                    if e.length.wrapping_sub(1) > 0x1FFFE {
                        out.push_str(&format!("Unable to MD5 hash data, size invalid:  {}\n", e.length as i32));
                        continue;
                    }
                    let actual = md5_of(lump_data(&data, e).unwrap_or_default());
                    let stored = e.resource.md5.unwrap_or_default();
                    if actual == stored {
                        out.push_str(" OK\n");
                    } else {
                        out.push_str(&format!(
                            " MISMATCHED\n--------------------\n File  :  {}\n Actual:  {}\n--------------------\n",
                            md5_hex(&stored),
                            md5_hex(&actual)
                        ));
                    }
                }
            }
            "hpkextract" => {
                if argv.len() != 3 {
                    return "Usage:  hpkextract hpkname [all | single index]\n".to_owned();
                }
                let want = if arg(2).eq_ignore_ascii_case("all") { -1 } else { gs_console_atoi(arg(2)) };
                if want == -1 {
                    out.push_str(&format!("Extracting all lumps from {pak}.\n"));
                } else {
                    out.push_str(&format!("Extracting lump {want} from {pak}\n"));
                }
                let Some((data, dir)) = open_pak(fs, &pak, "hpkextract:  version mismatch\n", &mut out) else { return out };
                out.push_str(&format!("# of Entries:  {}\n# Type Size FileName : MD5 Hash\n", dir.len()));
                for (k, e) in dir.iter().enumerate().filter(|(k, _)| want == -1 || want as usize == *k) {
                    out.push_str(&format!("Extracting {}: {}\n", k + 1, entry_columns(e)));
                    if e.length.wrapping_sub(1) > 0x1FFFE {
                        out.push_str("Unable to extract data, size invalid:  (null)\n");
                        continue;
                    }
                    let path = format!("hpklmps/lmp{k:04}.wad");
                    if fs.write(&path, PathId::GameConfig, lump_data(&data, e).unwrap_or_default()).is_err() {
                        out.push_str(&format!("Error creating lump file {}\n", path.replace('/', "\\")));
                    }
                }
            }
            _ => {}
        }
        out
    }
}

/// `Q_atoi` (decimal and `0x` hex, optional `-`).
fn gs_console_atoi(s: &str) -> i32 {
    let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let v = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        hex.chars().map_while(|c| c.to_digit(16)).fold(0i32, |a, d| a.wrapping_mul(16).wrapping_add(d as i32))
    } else {
        s.chars().map_while(|c| c.to_digit(10)).fold(0i32, |a, d| a.wrapping_mul(10).wrapping_add(d as i32))
    };
    if neg { v.wrapping_neg() } else { v }
}

/// `Q_snprintf(256)` + `COM_DefaultExtension(".hpk")`: appended unless a `.` follows the last
/// `/` (index 0 never tested).
fn default_ext(name: &str) -> String {
    let name = gs_common::bytestr::truncate_encoded(name, 255).into_owned();
    let b = name.as_bytes();
    let has_ext = (1..b.len()).rev().take_while(|&i| b[i] != b'/').any(|i| b[i] == b'.');
    if has_ext { name } else { format!("{name}.hpk") }
}

/// `type size name` columns: `%10s %.2fK %s` with `COM_FileBase` of the resource name.
fn entry_columns(e: &Entry) -> String {
    let kind = match e.resource.kind {
        0 => "sound",
        1 => "skin",
        2 => "model",
        3 => "decal",
        4 => "generic",
        5 => "event",
        _ => "?",
    };
    let size = f64::from(e.resource.download_size as i32 as f32 * (1.0f32 / 1024.0));
    let file = e.resource.name.rsplit(['/', '\\']).next().unwrap_or("");
    let base = file.rsplit_once('.').map_or(file, |(b, _)| b);
    format!("{kind:>10} {size:.2}K {base}")
}

/// The shared open and header checks of the HPAK commands (messages appended to `out`).
fn open_pak(fs: &FileSystem, name: &str, version_msg: &str, out: &mut String) -> Option<(Vec<u8>, Vec<Entry>)> {
    let Ok(data) = fs.read(name) else {
        out.push_str(&format!("ERROR: couldn't open {name}.\n"));
        return None;
    };
    if data.get(..4) != Some(b"HPAK".as_slice()) || data.len() < 12 {
        out.push_str(&format!("{name} is not an HPAK file\n"));
        return None;
    }
    let i32_at = |o: usize| data.get(o..o + 4).map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if i32_at(4) != 1 {
        out.push_str(version_msg);
        return None;
    }
    let dir = i32_at(8) as u32 as usize;
    let n = i32_at(dir);
    if (n as u32).wrapping_sub(1) > 0x7FFF {
        out.push_str(&format!("ERROR: HPAK had bogus # of directory entries:  {n}\n"));
        return None;
    }
    // A short file reads as zeros past its end (the original's FS_Read leaves them unset).
    let entries = (0..n as usize)
        .filter_map(|i| {
            let at = dir + 4 + i * ENTRY_SIZE;
            let mut raw = [0u8; ENTRY_SIZE];
            if let Some(src) = data.get(at..) {
                let len = src.len().min(ENTRY_SIZE);
                raw[..len].copy_from_slice(&src[..len]);
            }
            let u32_at = |o: usize| u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
            Some(Entry { resource: parse_resource(&raw)?, offset: u32_at(RESOURCE_SIZE), length: u32_at(RESOURCE_SIZE + 4) })
        })
        .collect();
    Some((data, entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_fs(tag: &str) -> (FileSystem, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("gs_hpak_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(dir.join("cstrike"));
        let fs = FileSystem::new(vec![gs_fs::SearchPath { dir: dir.join("cstrike"), id: PathId::GameConfig, writable: true }]);
        (fs, dir)
    }

    fn logo(byte: u8) -> (Resource, Vec<u8>) {
        let data = vec![byte; 2000];
        let r = Resource { kind: 3, name: "tempdecal.wad".into(), index: 0, download_size: 2000, flags: 4, md5: Some(md5_of(&data)), reserved: None };
        (r, data)
    }

    #[test]
    fn lumps_are_found_by_md5_and_the_directory_stays_sorted() {
        let (fs, dir) = temp_fs("add");
        let (a, da) = logo(1);
        let (b, db) = logo(2);
        assert!(Hpak::add(&fs, &a, &da).is_ok());
        assert!(Hpak::add(&fs, &b, &db).is_ok());
        assert!(Hpak::add(&fs, &a, &da).is_ok(), "a stored MD5 is not added twice");
        let file = fs.read(CUSTOM_HPK).unwrap_or_default();
        let entries = read_directory(&file).unwrap_or_default();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].resource.md5 < entries[1].resource.md5, "ascending by MD5");
        let hpak = Hpak::default();
        assert_eq!(hpak.get(&fs, &b.md5.unwrap_or_default()), Some(db));
        Hpak::check_integrity(&fs);
        assert!(fs.exists(CUSTOM_HPK), "a valid pack survives the integrity check");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bogus_lumps_are_refused_and_names_carry_the_hex_md5() {
        let (mut r, data) = logo(3);
        r.download_size = 100;
        assert!(!Hpak::default().add_queued(&r, &data), "below 1024 bytes");
        let md5 = md5_of(&data);
        let name = format!("!MD5{}", md5_hex(&md5));
        assert_eq!(md5_from_name(&name), Some(md5));
        assert_eq!(md5_from_name("!MD5zz"), None);
    }
}
