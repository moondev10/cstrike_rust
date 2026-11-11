//! A locally loaded map for viewing without a server: world, visible brush entities, spawn view.

use anyhow::{Context, Result};
use glam::Vec3;
use gs_assets::MapAssets;
use gs_formats::entities::Entity;
use gs_fs::FileSystem;
use gs_render::{BrushInstance, RenderMode};

/// Brush entities the server never sends to clients as visible geometry.
const INVISIBLE_BRUSH_CLASSES: [&str; 8] = [
    "func_buyzone",
    "func_bomb_target",
    "func_hostage_rescue",
    "func_escapezone",
    "func_vip_safetyzone",
    "func_ladder",
    "func_mortar_field",
    "env_bubbles",
];
/// Player eye height above the hull origin when standing (`VEC_VIEW`).
const VIEW_HEIGHT: f32 = 17.0;

/// A map loaded from disk with the brush entities a server would show.
pub struct LocalMap {
    pub map: MapAssets,
    /// Brush entities only.
    pub draw: gs_render::Scene,
    pub spawn_origin: Vec3,
    pub spawn_angles: Vec3,
}

impl LocalMap {
    pub fn load(fs: &FileSystem, map_name: &str) -> Result<Self> {
        let map = MapAssets::load(fs, map_name).with_context(|| format!("loading map {map_name}"))?;
        let brushes = map.entities.iter().filter_map(visible_brush).collect();
        let spawn = map
            .entities
            .iter()
            .find(|e| matches!(e.classname(), "info_player_start" | "info_player_deathmatch"));
        let spawn_origin = spawn.and_then(|e| e.vec3("origin")).map_or(Vec3::ZERO, Vec3::from) + Vec3::Z * VIEW_HEIGHT;
        let spawn_angles = spawn.and_then(|e| e.vec3("angles")).map_or(Vec3::ZERO, Vec3::from);
        Ok(Self { map, draw: gs_render::Scene { brushes, ..Default::default() }, spawn_origin, spawn_angles })
    }
}

fn visible_brush(e: &Entity) -> Option<BrushInstance> {
    let model = e.brush_model().filter(|&m| m > 0)?;
    let class = e.classname();
    if class.starts_with("trigger_") || INVISIBLE_BRUSH_CLASSES.contains(&class) {
        return None;
    }
    let rendermode = e.get("rendermode").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
    let renderamt = e.get("renderamt").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
    if rendermode != 0 && renderamt == 0 {
        return None;
    }
    let color = e.vec3("rendercolor").unwrap_or([255.0; 3]).map(|c| c.clamp(0.0, 255.0) as u8);
    Some(BrushInstance {
        model,
        origin: e.vec3("origin").map_or(Vec3::ZERO, Vec3::from),
        angles: e.vec3("angles").map_or(Vec3::ZERO, Vec3::from),
        rendermode: RenderMode::from_i32(rendermode),
        blend: if rendermode == 0 { 1.0 } else { renderamt.clamp(0, 255) as f32 / 255.0 },
        color,
    })
}
