//! `sound/materials.txt` texture types (`PM_InitTextureTypes`, `PM_FindTextureType`,
//! docs/re/movement.md §A.4).

const CTEXTURESMAX: usize = 1024;
const CBTEXTURENAMEMAX: usize = 17;

#[derive(Debug, Clone, Default)]
pub struct TextureTypes {
    names: Vec<Vec<u8>>,
    types: Vec<u8>,
}

fn cmp_ci(a: &[u8], b: &[u8], n: usize) -> std::cmp::Ordering {
    let lower = |s: &[u8]| s.iter().take(n).map(u8::to_ascii_lowercase).collect::<Vec<_>>();
    lower(a).cmp(&lower(b))
}

impl TextureTypes {
    /// Parses the file contents exactly like `PM_InitTextureTypes` (lines are `memfgets` lines of
    /// at most 511 characters).
    pub fn parse(buf: &[u8]) -> Self {
        let mut names: Vec<Vec<u8>> = Vec::new();
        let mut types: Vec<u8> = Vec::new();
        let mut pos = 0usize;
        while pos < buf.len() && names.len() < CTEXTURESMAX {
            // memfgets: up to and including '\n', at most 511 characters.
            let rest = &buf[pos..];
            let mut len = rest.iter().position(|&c| c == b'\n').map_or(rest.len(), |i| i + 1);
            len = len.min(511);
            let line = &rest[..len];
            pos += len;
            let at = |i: usize| line.get(i).copied().unwrap_or(0);
            let mut i = 0;
            while at(i) != 0 && at(i).is_ascii_whitespace() {
                i += 1;
            }
            if at(i) == 0 || at(i) == b'/' || !at(i).is_ascii_alphabetic() {
                continue;
            }
            let kind = at(i).to_ascii_uppercase();
            i += 1;
            while at(i) != 0 && at(i).is_ascii_whitespace() {
                i += 1;
            }
            if at(i) == 0 {
                continue;
            }
            let mut j = i;
            while at(j) != 0 && !at(j).is_ascii_whitespace() {
                j += 1;
            }
            if at(j) == 0 {
                continue;
            }
            let j = j.min(i + CBTEXTURENAMEMAX - 1);
            names.push(line[i..j].to_vec());
            types.push(kind);
        }
        // O(n²) exchange sort, case-insensitive, as the original.
        for i in 0..names.len() {
            for j in i + 1..names.len() {
                if cmp_ci(&names[i], &names[j], usize::MAX) == std::cmp::Ordering::Greater {
                    names.swap(i, j);
                    types.swap(i, j);
                }
            }
        }
        Self { names, types }
    }

    /// Any entry of material `kind` (`PM_IsThereGrassTexture` scans the type bytes for `'X'`).
    pub fn contains_type(&self, kind: u8) -> bool {
        self.types.contains(&kind)
    }

    /// `PM_FindTextureType`: binary search on the first 16 characters; `'C'` when unknown.
    pub fn find(&self, name: &str) -> u8 {
        let name = name.as_bytes();
        let (mut lo, mut hi) = (0i32, self.names.len() as i32 - 1);
        while lo <= hi {
            let mid = (lo + hi) / 2;
            match cmp_ci(name, &self.names[mid as usize], CBTEXTURENAMEMAX - 1) {
                std::cmp::Ordering::Equal => return self.types[mid as usize],
                std::cmp::Ordering::Greater => lo = mid + 1,
                std::cmp::Ordering::Less => hi = mid - 1,
            }
        }
        b'C'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sorts_and_finds_like_the_original() {
        let t = TextureTypes::parse(b"// comment\nM METAL1\nd dirtfloor\n  G grate_extremely_long_name_x\nT tile_no_newline");
        assert_eq!(t.find("metal1"), b'M');
        assert_eq!(t.find("DIRTFLOOR"), b'D');
        assert_eq!(t.find("grate_extremely_"), b'G', "names are cut to 16 characters");
        assert_eq!(t.find("grate_extremely_long_name_x"), b'G', "lookups compare 16 characters");
        assert_eq!(t.find("tile_no_newline"), b'C', "a name must be followed by whitespace");
        assert_eq!(t.find("unknown"), b'C');
    }
}
