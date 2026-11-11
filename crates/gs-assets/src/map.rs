//! A map with everything needed to draw and simulate its world model.

use gs_formats::bsp::Bsp;
use gs_formats::entities::{self, Entity};
use gs_formats::miptex::MipTex;
use gs_formats::wad::Wad;
use gs_fs::FileSystem;

use crate::{AssetError, RgbaImage};

/// Sky used when worldspawn has no `skyname`.
pub const DEFAULT_SKY: &str = "desert";
/// Skybox face suffixes in the order stored in [`SkyBox::faces`].
pub const SKY_SUFFIXES: [&str; 6] = ["rt", "bk", "lf", "ft", "up", "dn"];

#[derive(Debug, Clone)]
pub struct SkyBox {
    pub name: String,
    /// Indexed like [`SKY_SUFFIXES`].
    pub faces: [RgbaImage; 6],
}

impl SkyBox {
    pub fn load(fs: &FileSystem, name: &str) -> Result<Self, AssetError> {
        let load = |suffix: &str| RgbaImage::load(fs, &format!("gfx/env/{name}{suffix}.tga"));
        Ok(Self {
            name: name.to_owned(),
            faces: [load("rt")?, load("bk")?, load("lf")?, load("ft")?, load("up")?, load("dn")?],
        })
    }
}

#[derive(Debug, Clone)]
pub struct MapAssets {
    /// Map name without directory/extension, e.g. `de_dust2`.
    pub name: String,
    pub bsp: Bsp,
    pub entities: Vec<Entity>,
    /// Indexed like `bsp.textures`: embedded or WAD-resolved texture, `None` when unresolvable.
    pub textures: Vec<Option<MipTex>>,
    pub sky: Option<SkyBox>,
}

impl MapAssets {
    /// Loads `maps/<name>.bsp`, its WAD textures and skybox.
    pub fn load(fs: &FileSystem, name: &str) -> Result<Self, AssetError> {
        let path = format!("maps/{name}.bsp");
        let data = fs.read(&path)?;
        let bsp = Bsp::parse(&data).map_err(|source| AssetError::Format { path: path.clone(), source })?;
        let entities = entities::parse(&bsp.entities).map_err(|source| AssetError::Format { path: path.clone(), source })?;
        let world = entities.iter().find(|e| e.classname() == "worldspawn");

        let wads = load_wads(fs, world.and_then(|w| w.get("wad")).unwrap_or(""));
        let textures = bsp
            .textures
            .iter()
            .map(|tex| {
                let tex = tex.as_ref()?;
                if tex.data.is_some() {
                    return Some(tex.clone());
                }
                let found = wads.iter().find_map(|(wad_name, wad)| match wad.miptex(&tex.name)? {
                    Ok(t) => Some(t),
                    Err(err) => {
                        log::warn!("{wad_name}: texture {}: {err}", tex.name);
                        None
                    }
                });
                if found.is_none() {
                    log::warn!("{name}: texture {} not found in any WAD", tex.name);
                }
                found
            })
            .collect();

        let sky_name = world.and_then(|w| w.get("skyname")).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_SKY);
        let sky = SkyBox::load(fs, sky_name)
            .inspect_err(|err| log::warn!("{name}: skybox {sky_name}: {err}"))
            .ok();

        Ok(Self { name: name.to_owned(), bsp, entities, textures, sky })
    }
}

/// WADs named by worldspawn's `wad` key (`;`-separated, absolute author paths); only the file
/// name is used and looked up through the search paths.
fn load_wads(fs: &FileSystem, list: &str) -> Vec<(String, Wad)> {
    let mut out = Vec::new();
    for entry in list.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let file = entry.rsplit(['/', '\\']).next().unwrap_or(entry);
        match fs.read(file).map_err(AssetError::from).and_then(|data| {
            Wad::parse(data).map_err(|source| AssetError::Format { path: file.to_owned(), source })
        }) {
            Ok(wad) => out.push((file.to_owned(), wad)),
            Err(err) => log::warn!("wad {file}: {err}"),
        }
    }
    out
}
