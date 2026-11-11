//! `CHudSpectator`'s overview map (docs/re/spectator-overview.md): the map image cut into
//! 128×128 tiles (`cle_LoadMapSprite`), the per-frame entity icons and the death skulls
//! (`m_OverviewEntities`), `DrawOverview`'s layer, icons, height beams and view cone as client
//! triangles (`HUD_DrawNormalTriangles`), and the player names on the map.

use super::health::team_color;
use super::spectator::Overview;
use super::{Hud, HudEngine};

/// The slots of `m_OverviewEntities`.
const MAX_ENTRIES: usize = 128;

/// One triangle vertex: world position and sprite coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriVertex {
    pub pos: [f32; 3],
    pub st: [f32; 2],
}

/// The texture of an overview batch (`tri.SpriteTexture`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewTexture {
    /// A frame of the map image (`m_MapSprite`).
    Map(u32),
    /// Frame 0 of a client sprite.
    Sprite(&'static str),
}

/// Triangles drawn with one texture, render mode and colour.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewBatch {
    pub texture: OverviewTexture,
    /// `RenderMode(5)`: additive without depth write; else `RenderMode(2)`: alpha blend with
    /// the depth write left on.
    pub additive: bool,
    /// The premultiplied `Color4f` (`r·a, g·a, b·a, 1`).
    pub color: [f32; 4],
    /// Triangle list (each quad as two triangles).
    pub vertices: Vec<TriVertex>,
}

impl OverviewBatch {
    fn new(texture: OverviewTexture, additive: bool, rgba: [f32; 4]) -> Self {
        let a = rgba[3];
        Self { texture, additive, color: [rgba[0] * a, rgba[1] * a, rgba[2] * a, 1.0], vertices: Vec::new() }
    }

    /// `Begin(TRI_QUADS)` with four vertices.
    fn quad(&mut self, v: [([f32; 3], [f32; 2]); 4]) {
        for i in [0, 1, 2, 0, 2, 3] {
            self.vertices.push(TriVertex { pos: v[i].0, st: v[i].1 });
        }
    }
}

/// What one view pass hands `DrawOverview`.
#[derive(Debug, Clone, Copy)]
pub struct OverviewPass {
    /// `m_iDrawCycle`: 0 for the main view, 1 for the inset.
    pub cycle: i32,
    /// The pass's `v_angles` (height flattening).
    pub angles: [f32; 3],
    /// The pass's world-to-screen matrix, column-major (`tri.WorldToScreen`).
    pub world_to_screen: [f32; 16],
    /// The view cone's camera (origin, angles), when the cone is drawn.
    pub cone: Option<([f32; 3], [f32; 3])>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    sprite: &'static str,
    entity: i32,
    kill_time: f64,
}

/// `m_OverviewEntities`, `m_vPlayerPos` and the map sprite.
#[derive(Debug, Clone)]
pub struct OverviewState {
    entries: [Option<Entry>; MAX_ENTRIES],
    /// `m_vPlayerPos`: name anchors in screen pixels; z is the valid flag.
    player_pos: [[f32; 3]; 64],
    /// `m_OverviewData.map`: the level the data was parsed for.
    map: String,
    /// The map image `Reset` asks for, until the host loads it.
    pending_image: Option<String>,
    /// `m_MapSprite->numframes` (`None`: no map sprite, the tile fallback).
    frames: Option<u32>,
}

impl Default for OverviewState {
    fn default() -> Self {
        Self { entries: [None; MAX_ENTRIES], player_pos: [[0.0, 0.0, -1.0]; 64], map: String::new(), pending_image: None, frames: None }
    }
}

/// `AngleVectors`: forward, right.
fn forward_right(angles: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    (forward, right)
}

/// `tri.WorldToScreen`: normalised device x/y, and whether the point is behind the view.
fn world_to_screen(m: &[f32; 16], p: [f32; 3]) -> ([f32; 2], bool) {
    let row = |r: usize| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r];
    let (x, y, w) = (row(0), row(1), row(3));
    let s = if w != 0.0 { [x / w, y / w] } else { [0.0, 0.0] };
    (s, 1.0 / w <= 0.0)
}

/// `cle_LoadMapSprite`'s BMP reader: 8-bit uncompressed single-plane files only, rows from the
/// top, every pixel opaque (the green key comes with the tiles).
pub fn decode_map_bmp(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if data.get(..2)? != b"BM" {
        return None;
    }
    let bits_offset = u32_at(10)? as usize;
    let info_size = u32_at(14)? as usize;
    let width = i32::from_le_bytes(data.get(18..22)?.try_into().ok()?);
    let height = i32::from_le_bytes(data.get(22..26)?.try_into().ok()?);
    if u16_at(26)? != 1 || u16_at(28)? != 8 || u32_at(30)? != 0 || width <= 0 || height == 0 {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let colors = match u32_at(46)? {
        0 => 256,
        n => (n as usize).min(256),
    };
    let palette = data.get(14 + info_size..14 + info_size + colors * 4)?;
    let stride = w.div_ceil(4) * 4;
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        // Bottom-up unless the height is negative.
        let src_row = if height > 0 { h - 1 - y } else { y };
        let row = data.get(bits_offset + src_row * stride..bits_offset + src_row * stride + w)?;
        for (x, &index) in row.iter().enumerate() {
            let p = palette.get(usize::from(index) * 4..usize::from(index) * 4 + 4).unwrap_or(&[0; 4]);
            rgba[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    Some((w as u32, h as u32, rgba))
}

/// `cle_LoadMapSprite`'s tiles: pure green made transparent black, then 128×128 tiles row-major
/// from the top-left. `None` unless both sides are non-zero multiples of 128 and the image fits
/// the 0x300000-byte work buffer.
pub fn map_tiles(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<(u32, u32, Vec<u8>)>> {
    if width == 0 || height == 0 || !width.is_multiple_of(128) || !height.is_multiple_of(128) || u64::from(width) * u64::from(height) * 4 > 0x30_0000 {
        return None;
    }
    if rgba.len() < (width * height * 4) as usize {
        return None;
    }
    let mut tiles = Vec::with_capacity(((width / 128) * (height / 128)) as usize);
    for ty in 0..height / 128 {
        for tx in 0..width / 128 {
            let mut tile = Vec::with_capacity(128 * 128 * 4);
            for y in 0..128 {
                let start = (((ty * 128 + y) * width + tx * 128) * 4) as usize;
                for px in rgba[start..start + 128 * 4].chunks_exact(4) {
                    if px[..3] == [0, 255, 0] {
                        tile.extend_from_slice(&[0, 0, 0, 0]);
                    } else {
                        tile.extend_from_slice(px);
                    }
                }
            }
            tiles.push((128, 128, tile));
        }
    }
    Some(tiles)
}

impl Hud {
    /// `CHudSpectator::Reset`'s overview part: a new level's `overviews/<map>.txt` (with the
    /// map camera from it when it parses) and its map image, the inset rectangle for this
    /// width, and an empty entity list.
    pub(super) fn overview_reset(&mut self, eng: &mut dyn HudEngine) {
        let level = eng.level_name();
        if level != self.overview_map.map {
            self.overview_map.map.clone_from(&level);
            let base = level.trim_start_matches("maps/");
            let base = base.get(..base.len().saturating_sub(4)).unwrap_or(base);
            let path = format!("overviews/{base}.txt");
            let width = eng.screen().width;
            self.overview = match eng.read_file(&path) {
                Some(data) => {
                    let o = Overview::parse(&data, width);
                    self.observer.map_zoom = o.zoom;
                    self.observer.map_origin = o.origin;
                    o
                }
                None => {
                    eng.console_print(&format!("Couldn't open file {path}. Using default values for overiew mode.\n"));
                    Overview::new(width)
                }
            };
            // [INFERENCE] no layer, no LoadMapSprite (the tile fallback).
            self.overview_map.frames = None;
            self.overview_map.pending_image = self.overview.layer.as_ref().map(|(image, _)| image.clone()).filter(|i| !i.is_empty());
        }
        self.overview_map.entries = [None; MAX_ENTRIES];
    }

    /// The map image `Reset` wants loaded (once).
    pub fn take_overview_image(&mut self) -> Option<String> {
        self.overview_map.pending_image.take()
    }

    /// The loaded map sprite's frame count (`None` when it failed).
    pub fn set_overview_frames(&mut self, frames: Option<u32>) {
        self.overview_map.frames = frames;
    }

    /// The host reloaded the level's sprites: the map image is uploaded again (a new level's
    /// `Reset` asks for its own).
    pub fn overview_level_loaded(&mut self) {
        self.overview_map.frames = None;
        self.overview_map.pending_image = self.overview.layer.as_ref().map(|(image, _)| image.clone()).filter(|i| !i.is_empty());
    }

    /// `AddOverviewEntityToList`: the first free slot; dropped when all 128 are used.
    fn overview_add(&mut self, sprite: &'static str, entity: i32, kill_time: f64) {
        if let Some(slot) = self.overview_map.entries.iter_mut().find(|e| e.is_none()) {
            *slot = Some(Entry { sprite, entity, kill_time });
        }
    }

    /// `CHudSpectator::AddOverviewEntity` from `HUD_AddEntity` (`kind` 0 normal, 1 player):
    /// live players by team (VIP, then C4 over it), the bomb, the backpack and hostages.
    pub fn overview_add_entity(&mut self, kind: i32, index: i32, solid: bool, model: &str, eng: &dyn HudEngine) {
        let sprite = match kind {
            1 => {
                if !solid {
                    return;
                }
                let p = usize::try_from(index).ok().and_then(|i| self.players.get(i));
                let mut s = match p.map_or(0, |p| p.teamnumber) {
                    1 => "sprites/iplayerred.spr",
                    2 => "sprites/iplayerblue.spr",
                    _ => "sprites/iplayer.spr",
                };
                if p.is_some_and(|p| p.vip) {
                    s = "sprites/iplayervip.spr";
                }
                if p.is_some_and(|p| p.has_c4) {
                    s = "sprites/iplayerc4.spr";
                }
                s
            }
            0 if model == "models/w_c4.mdl" => "sprites/ic4.spr",
            0 if model == "models/w_backpack.mdl" => "sprites/ibackpack.spr",
            0 if model.get(..14) == Some("models/hostage") || model.get(..16) == Some("models/scientist") => "sprites/ihostage.spr",
            _ => return,
        };
        let now = f64::from(eng.client_time());
        self.overview_add(sprite, index, now - 1.0);
    }

    /// `CHudSpectator::DeathMessage`: a skull over a dead player for 4 s of overview frames.
    pub(super) fn overview_death(&mut self, victim: i32, eng: &dyn HudEngine) {
        if eng.entity(victim).is_some_and(|e| e.player) {
            let now = eng.client_time();
            self.overview_add("sprites/iplayerdead.spr", victim, f64::from(now + 4.0));
        }
    }

    /// Whether `DrawOverview` draws in this pass (the cone camera is wanted then).
    pub fn overview_drawn(&self, cycle: i32, eng: &dyn HudEngine) -> bool {
        match (self.state.user1, cycle) {
            (0, _) => false,
            (u, 0) => matches!(u, 5 | 6),
            _ => eng.cvar("spec_pip") >= 3.0 && self.viewport.force_camera == 0,
        }
    }

    /// The view cone is drawn with the map (`spec_pip` and `spec_drawcone_internal` on).
    pub fn overview_cone_wanted(&self, eng: &dyn HudEngine) -> bool {
        eng.cvar("spec_pip") != 0.0 && eng.cvar("spec_drawcone_internal") != 0.0
    }

    /// `CHudSpectator::DrawOverview` at `HUD_DrawNormalTriangles`: the layer, the entity icons
    /// with the players' height beams and name anchors, the view cone, then the expired
    /// entries purged.
    pub fn draw_overview(&mut self, pass: &OverviewPass, eng: &mut dyn HudEngine) -> Vec<OverviewBatch> {
        let user1 = self.state.user1;
        if user1 == 0 {
            return Vec::new();
        }
        if pass.cycle == 0 {
            if !matches!(user1, 5 | 6) {
                return Vec::new();
            }
        } else {
            if eng.cvar("spec_pip") < 3.0 {
                return Vec::new();
            }
            if self.viewport.force_camera != 0 {
                eng.cvar_set_value("spec_pip", 0.0);
                return Vec::new();
            }
        }
        let screen = eng.screen();
        let (sw, sh) = (screen.width as f32, screen.height as f32);
        // The overview images are drawn for 4:3. The original lays the tiles out with the
        // screen's aspect, which squeezes and shifts the map under the icons on wide screens;
        // laid out at 4:3 the map lines up with the world at any resolution (deviations.md).
        let mut out = self.overview_layer(pass, 4.0 / 3.0);
        self.overview_entities(pass, (sw, sh), eng, &mut out);
        let now = f64::from(eng.client_time());
        for slot in &mut self.overview_map.entries {
            if slot.is_some_and(|e| now > e.kill_time) {
                *slot = None;
            }
        }
        out
    }

    /// `DrawOverviewLayer`: the map's tiles (or `tile.spr` 8×6) as world quads at the layer
    /// height, from the file's origin and zoom with the aspect `a` (4:3, see `draw_overview`).
    fn overview_layer(&self, pass: &OverviewPass, a: f32) -> Vec<OverviewBatch> {
        let o = &self.overview;
        let (z, origin) = (o.zoom, o.origin);
        let height = o.layer.as_ref().map_or(0.0, |(_, h)| *h);
        let zl = (90.0 - pass.angles[0]) / 90.0 * height;
        let (x_tiles, y_tiles, map) = match self.overview_map.frames {
            Some(n) => {
                let k = f64::from(n / 12).sqrt() as u32;
                (4 * k, 3 * k, true)
            }
            None => (8, 6, false),
        };
        let mut out: Vec<OverviewBatch> = Vec::new();
        let mut frame = 0u32;
        for r in 0..y_tiles {
            for c in 0..x_tiles {
                let texture = if map { OverviewTexture::Map(frame) } else { OverviewTexture::Sprite("sprites/tile.spr") };
                let (cf, rf) = (c as f32, r as f32);
                let quad = if o.rotated {
                    let d_x = 8192.0 / (z * x_tiles as f32);
                    let d_y = -8192.0 / (z * a * y_tiles as f32);
                    let (x, y) = (origin[0] - 4096.0 / z + cf * d_x, origin[1] + 4096.0 / (z * a) + rf * d_y);
                    [([x, y, zl], [0.0, 0.0]), ([x + d_x, y, zl], [1.0, 0.0]), ([x + d_x, y + d_y, zl], [1.0, 1.0]), ([x, y + d_y, zl], [0.0, 1.0])]
                } else {
                    let d_x = -8192.0 / (z * x_tiles as f32);
                    let d_y = -8192.0 / (z * a * y_tiles as f32);
                    let (x, y) = (origin[0] + 4096.0 / (z * a) + rf * d_x, origin[1] + 4096.0 / z + cf * d_y);
                    [([x, y, zl], [0.0, 0.0]), ([x + d_x, y, zl], [0.0, 1.0]), ([x + d_x, y + d_y, zl], [1.0, 1.0]), ([x, y + d_y, zl], [1.0, 0.0])]
                };
                match out.last_mut().filter(|b| b.texture == texture) {
                    Some(batch) => batch.quad(quad),
                    None => {
                        let mut batch = OverviewBatch::new(texture, false, [1.0; 4]);
                        batch.quad(quad);
                        out.push(batch);
                    }
                }
                frame += 1;
            }
        }
        out
    }

    /// `DrawOverviewEntities`: per slot a 128-unit icon square turned with the entity, for
    /// players a height beam down to the layer and the name anchor below the icon; then the
    /// view cone of the world view.
    fn overview_entities(&mut self, pass: &OverviewPass, (sw, sh): (f32, f32), eng: &dyn HudEngine, out: &mut Vec<OverviewBatch>) {
        let zs = (90.0 - pass.angles[0]) / 90.0;
        let height = self.overview.layer.as_ref().map_or(0.0, |(_, h)| *h);
        let zl = zs * height;
        for p in &mut self.overview_map.player_pos {
            p[2] = -1.0;
        }
        let entries: Vec<Entry> = self.overview_map.entries.iter().flatten().copied().collect();
        for entry in entries {
            let Some(e) = eng.entity(entry.entity) else { continue };
            let p = e.origin;
            let (f, r) = forward_right(e.angles);
            let v = |sr: f32, sf: f32| [(p[0] + sr * 64.0 * r[0] + sf * 64.0 * f[0]), (p[1] + sr * 64.0 * r[1] + sf * 64.0 * f[1]), (p[2] + sr * 64.0 * r[2] + sf * 64.0 * f[2]) * zs];
            let mut icon = OverviewBatch::new(OverviewTexture::Sprite(entry.sprite), false, [1.0; 4]);
            icon.quad([(v(1.0, 1.0), [1.0, 0.0]), (v(1.0, -1.0), [0.0, 0.0]), (v(-1.0, -1.0), [0.0, 1.0]), (v(-1.0, 1.0), [1.0, 1.0])]);
            out.push(icon);
            if !e.player {
                continue;
            }
            let pz = zs * p[2];
            let mut beam = OverviewBatch::new(OverviewTexture::Sprite("sprites/laserbeam.spr"), true, [1.0, 0.627_451, 0.0, 0.3]);
            let top = pz - zs;
            beam.quad([
                ([p[0] + 4.0, p[1] + 4.0, top], [1.0, 0.0]),
                ([p[0] - 4.0, p[1] - 4.0, top], [0.0, 0.0]),
                ([p[0] - 4.0, p[1] - 4.0, zl], [0.0, 1.0]),
                ([p[0] + 4.0, p[1] + 4.0, zl], [1.0, 1.0]),
            ]);
            beam.quad([
                ([p[0] - 4.0, p[1] + 4.0, top], [1.0, 0.0]),
                ([p[0] + 4.0, p[1] - 4.0, top], [0.0, 0.0]),
                ([p[0] + 4.0, p[1] - 4.0, zl], [0.0, 1.0]),
                ([p[0] - 4.0, p[1] + 4.0, zl], [1.0, 1.0]),
            ]);
            out.push(beam);
            let (s, behind) = world_to_screen(&pass.world_to_screen, [p[0], p[1], pz]);
            if !behind {
                let (sx, sy) = ((s[0] + 1.0) * sw * 0.5, (1.0 - s[1]) * sh * 0.5);
                let (o, _) = world_to_screen(&pass.world_to_screen, [p[0] + 32.0, p[1] + 32.0, pz]);
                let (dx, dy) = ((o[0] + 1.0) * sw * 0.5 - sx, (1.0 - o[1]) * sh * 0.5 - sy);
                let d = (dx * dx + dy * dy).sqrt();
                if let Some(slot) = usize::try_from(e.index - 1).ok().and_then(|i| self.overview_map.player_pos.get_mut(i)) {
                    *slot = [sx, sy + d, 1.0];
                }
            }
        }
        if let Some((origin, mut angles)) = pass.cone {
            angles[0] = 0.0;
            let (f, _) = forward_right(angles);
            let fwd = f.map(|v| v * 512.0);
            let (s45, c45) = 45f32.to_radians().sin_cos();
            let rp = [fwd[0] * c45 - fwd[1] * s45, fwd[0] * s45 + fwd[1] * c45, fwd[2]];
            let rm = [fwd[0] * c45 + fwd[1] * s45, -fwd[0] * s45 + fwd[1] * c45, fwd[2]];
            let at = |d: [f32; 3]| [origin[0] + d[0], origin[1] + d[1], (origin[2] + d[2]) * zs];
            let mut cone = OverviewBatch::new(OverviewTexture::Sprite("sprites/camera.spr"), true, [1.0, 0.627_451, 0.0, 1.0]);
            cone.vertices.extend([TriVertex { pos: at(rp), st: [0.0, 0.0] }, TriVertex { pos: at([0.0; 3]), st: [0.0, 1.0] }, TriVertex { pos: at(rm), st: [1.0, 1.0] }]);
            out.push(cone);
        }
    }

    /// `CHudSpectator::Draw`'s names: in a main map view, each anchored player's name in its
    /// team colour (EngineFont), skipping anchors inside the PIP inset.
    pub(super) fn overview_names(&mut self, eng: &mut dyn HudEngine) {
        if self.state.user1 < 5 || eng.cvar("spec_drawnames_internal") == 0.0 {
            return;
        }
        let pip = eng.cvar("spec_pip") != 0.0;
        let [ix, iy, iw, ih] = self.overview.window.map(|v| v as f32);
        for i in 1..=64 {
            let pos = self.overview_map.player_pos[i - 1];
            if pos[2] < 0.0 || (pip && pos[0] > ix && pos[1] > iy && pos[0] < ix + iw && pos[1] < iy + ih) {
                continue;
            }
            let Some(info) = eng.player_info(i as i32) else { continue };
            let c = team_color(self.players.get(i).map_or(0, |p| p.teamnumber));
            let screen = eng.screen();
            screen.set_text_color(c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
            screen.draw_console_string((pos[0] - 3.0 * info.name.len() as f32) as i32, pos[1] as i32, &info.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_images_tile_row_major_with_green_transparent() {
        let (w, h) = (256u32, 128u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // Left tile red, right tile pure green.
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(if x < 128 { &[255, 0, 0, 255] } else { &[0, 255, 0, 255] });
            }
        }
        let tiles = map_tiles(w, h, &rgba).unwrap_or_default();
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].2[..4], [255, 0, 0, 255]);
        assert_eq!(tiles[1].2[..4], [0, 0, 0, 0], "pure green is cut out");
        assert!(map_tiles(200, 128, &rgba).is_none(), "sides must be multiples of 128");
        assert!(map_tiles(2048, 768, &vec![0; 2048 * 768 * 4]).is_none(), "larger than the work buffer");
    }

    #[test]
    fn bmp_rows_come_out_top_first() {
        // 2×2 8-bit BMP, palette 0 = blue, 1 = white; bottom row first in the file.
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&(14 + 40 + 8u32).to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&2i32.to_le_bytes());
        bmp.extend_from_slice(&2i32.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&8u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 16]);
        bmp.extend_from_slice(&2u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&[255, 0, 0, 0, 255, 255, 255, 0]);
        bmp.extend_from_slice(&[0, 0, 0, 0]); // bottom row: blue blue
        bmp.extend_from_slice(&[1, 1, 0, 0]); // top row: white white
        let (w, h, rgba) = decode_map_bmp(&bmp).unwrap_or_default();
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba[..4], [255, 255, 255, 255], "top row first");
        assert_eq!(rgba[8..12], [0, 0, 255, 255]);
    }

    #[test]
    fn world_to_screen_reports_points_behind_the_view() {
        let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        assert_eq!(world_to_screen(&identity, [0.5, -0.5, 0.0]), ([0.5, -0.5], false));
        let mut flip = identity;
        flip[15] = -1.0;
        assert!(world_to_screen(&flip, [0.0; 3]).1);
    }
}
