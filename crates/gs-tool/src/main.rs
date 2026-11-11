//! `gstool`: developer CLI for inspecting game content against our parsers.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use gs_formats::bsp::Bsp;
use gs_formats::wad::Wad;
use gs_formats::{entities, wad};
use gs_fs::{FileSystem, GameLayout};

#[derive(Parser)]
#[command(about = "Inspect GoldSrc content with the cstrike_rust parsers")]
struct Cli {
    /// Half-Life install directory (default: $GS_BASEDIR or the Steam default path).
    #[arg(long, global = true)]
    basedir: Option<PathBuf>,
    /// Mod directory.
    #[arg(long, global = true, default_value = "cstrike")]
    game: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the search path order.
    Paths,
    /// Summarize a map (`de_dust2` or `maps/de_dust2.bsp`).
    Bsp { map: String },
    /// Parse every map under maps/ and report failures.
    BspAll,
    /// List lumps of a WAD (`cstrike.wad`).
    Wad { name: String },
    /// Parse every texture of every WAD in the mod and fallback directories.
    WadAll,
    /// Consistency bounds (`R_GetStudioBounds`) of models, as decimal and wire hex.
    MdlBounds { models: Vec<String> },
    /// Summarize a studio model (bones, sequences, bodyparts, textures).
    Mdl { path: String },
    /// Load every studio model under models/ and pose every sequence; report failures.
    MdlAll,
    /// Parse every sprite under sprites/ and report type/format counts and failures.
    SprAll,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let base = cli.basedir.or_else(gs_fs::default_base_dir).context("Half-Life directory not found; pass --basedir")?;
    let fs = FileSystem::from_layout(&GameLayout::new(base, cli.game));
    match cli.cmd {
        Cmd::MdlBounds { models } => {
            for path in &models {
                let data = fs.read(path)?;
                match gs_formats::studio_bounds::studio_bounds(&data) {
                    Ok(Some((mins, maxs))) => {
                        let hex = |v: [f32; 3]| v.map(|f| format!("{:08X}", f.to_bits())).join(" ");
                        println!("{path}: mins {mins:?} maxs {maxs:?}\n  {} | {}", hex(mins), hex(maxs));
                    }
                    Ok(None) => println!("{path}: not an IDST model"),
                    Err(e) => println!("{path}: {e}"),
                }
            }
        }
        Cmd::Mdl { path } => mdl_summary(&fs, &path)?,
        Cmd::MdlAll => mdl_all(&fs)?,
        Cmd::SprAll => {
            let sprites = fs.list("sprites", "spr")?;
            let mut kinds = std::collections::BTreeMap::new();
            let mut failed = 0;
            for path in &sprites {
                match gs_formats::sprite::Sprite::parse(&fs.read(path)?) {
                    Ok(s) => *kinds.entry(format!("{:?}+{:?}", s.kind, s.format)).or_insert(0) += 1,
                    Err(e) => {
                        failed += 1;
                        println!("FAIL {path}: {e}");
                    }
                }
            }
            for (k, n) in &kinds {
                println!("{n:5} {k}");
            }
            println!("{} sprites, {failed} failed", sprites.len());
        }
        Cmd::Paths => {
            for sp in fs.search_paths() {
                let state = if sp.dir.is_dir() { "" } else { " (missing)" };
                println!("{:<13} {}{}{state}", format!("{:?}", sp.id), sp.dir.display(), if sp.writable { " [w]" } else { "" });
            }
        }
        Cmd::Bsp { map } => bsp_summary(&fs, &map_path(&map))?,
        Cmd::BspAll => {
            let maps = fs.list("maps", "bsp")?;
            let mut failed = 0;
            for map in &maps {
                if let Err(err) = load_bsp(&fs, map) {
                    failed += 1;
                    println!("FAIL {map}: {err:#}");
                }
            }
            println!("{} maps, {failed} failed", maps.len());
            if failed > 0 {
                bail!("{failed} maps failed to parse");
            }
        }
        Cmd::Wad { name } => {
            let wad = Wad::parse(fs.read(&name)?).with_context(|| name.clone())?;
            for lump in wad.lumps() {
                println!("{:#04x} {:16} {:>8}", lump.kind, lump.name, wad.lump_data(lump).len());
            }
        }
        Cmd::WadAll => {
            let wads = fs.list("", "wad")?;
            let (mut textures, mut failed) = (0, 0);
            for name in &wads {
                let wad = Wad::parse(fs.read(name)?).with_context(|| name.clone())?;
                for lump in wad.lumps().iter().filter(|l| l.kind == wad::lump_type::MIPTEX) {
                    textures += 1;
                    if let Some(Err(err)) = wad.miptex(&lump.name) {
                        failed += 1;
                        println!("FAIL {name}:{}: {err}", lump.name);
                    }
                }
            }
            println!("{} wads, {textures} textures, {failed} failed", wads.len());
        }
    }
    Ok(())
}

fn map_path(map: &str) -> String {
    if map.contains('/') || map.contains('\\') { map.to_owned() } else { format!("maps/{map}.bsp") }
}

fn load_bsp(fs: &FileSystem, path: &str) -> Result<Bsp> {
    let data = fs.read(path)?;
    let bsp = Bsp::parse(&data).with_context(|| path.to_owned())?;
    entities::parse(&bsp.entities).with_context(|| format!("{path}: entities"))?;
    Ok(bsp)
}

fn bsp_summary(fs: &FileSystem, path: &str) -> Result<()> {
    let bsp = load_bsp(fs, path)?;
    let ents = entities::parse(&bsp.entities)?;
    let world = ents.iter().find(|e| e.classname() == "worldspawn");
    println!("{path}");
    println!(
        "  planes {} verts {} nodes {} leafs {} faces {} clipnodes {} models {} vis {}B light {}B",
        bsp.planes.len(),
        bsp.vertices.len(),
        bsp.nodes.len(),
        bsp.leafs.len(),
        bsp.faces.len(),
        bsp.clipnodes.len(),
        bsp.models.len(),
        bsp.visibility.len(),
        bsp.lighting.len()
    );
    let embedded = bsp.textures.iter().flatten().filter(|t| t.data.is_some()).count();
    println!("  textures {} ({} embedded)", bsp.textures.len(), embedded);
    println!("  entities {}", ents.len());
    if let Some(world) = world {
        for key in ["wad", "skyname", "mapversion"] {
            println!("  worldspawn.{key} = {:?}", world.get(key).unwrap_or(""));
        }
    }
    let mut max_lm = (0, 0);
    for face in &bsp.faces {
        let (w, h) = bsp.surface_extents(face).lightmap_size();
        max_lm = (max_lm.0.max(w), max_lm.1.max(h));
    }
    println!("  largest lightmap {}x{}", max_lm.0, max_lm.1);
    Ok(())
}

fn mdl_summary(fs: &FileSystem, path: &str) -> Result<()> {
    let m = gs_assets::load_studio(fs, path)?;
    println!("{path}: {:?} flags {:#x} eye {:?}", m.name, m.flags, m.eyeposition);
    println!("  bones {} controllers {} hitboxes {} attachments {}", m.bones.len(), m.controllers.len(), m.hitboxes.len(), m.attachments.len());
    for (i, s) in m.sequences.iter().enumerate() {
        println!(
            "  seq {i:3} {:24} frames {:4} fps {:5.1} blends {} group {} flags {} act {} events {} move {:?}",
            s.label, s.numframes, s.fps, s.numblends, s.seqgroup, s.flags, s.activity, s.events.len(), s.linearmovement
        );
    }
    for p in &m.bodyparts {
        println!("  bodypart {:16} base {}", p.name, p.base);
        for sm in &p.models {
            let tris: usize = sm.meshes.iter().flat_map(|me| &me.cmds).map(|c| c.verts.len().saturating_sub(2)).sum();
            println!("    {:32} verts {} norms {} meshes {} tris {tris}", sm.name, sm.verts.len(), sm.norms.len(), sm.meshes.len());
        }
    }
    for t in &m.textures {
        println!("  texture {:32} {}x{} flags {:#x}", t.name, t.width, t.height, t.flags);
    }
    println!("  skin families {}", m.skins.len());
    Ok(())
}

fn studio_paths(fs: &FileSystem) -> Vec<String> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mdl"))
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    for sp in fs.search_paths() {
        walk(&sp.dir, &sp.dir.join("models"), &mut out);
    }
    out.sort_by_key(|p| p.to_ascii_lowercase());
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}

fn mdl_all(fs: &FileSystem) -> Result<()> {
    use gs_studio::pose::{AnimState, PoseParams, entity_rotation, setup_bones};
    let paths = studio_paths(fs);
    let (mut models, mut poses, mut failed) = (0, 0, 0);
    let params = PoseParams { time: 0.0, interpolate: false, mirror: false };
    for path in &paths {
        let data = fs.read(path)?;
        // Texture-only (`T.mdl`) and sequence-group (`IDSQ`) files are loaded through their owners.
        if data.get(..4) != Some(b"IDST") {
            continue;
        }
        if data.get(4..8) != Some(&10i32.to_le_bytes()) {
            println!("SKIP {path}: not version 10 (the engine rejects it too)");
            continue;
        }
        if gs_studio::StudioModel::parse(data.into()).is_ok_and(|m| m.bones.is_empty()) {
            continue;
        }
        let model = match gs_assets::load_studio(fs, path) {
            Ok(m) => m,
            Err(err) => {
                failed += 1;
                println!("FAIL {path}: {err}");
                continue;
            }
        };
        models += 1;
        for (i, seq) in model.sequences.iter().enumerate() {
            for frame in [0.0, 100.0, 255.0] {
                let state = AnimState { sequence: i as i32, frame, framerate: 1.0, blending: [64, 200], ..AnimState::default() };
                let bones = setup_bones(&model, &state, &entity_rotation([0.0; 3], [0.0; 3]), &params).bones;
                poses += 1;
                if bones.iter().flatten().flatten().any(|v| !v.is_finite() || v.abs() > 1.0e5) {
                    failed += 1;
                    println!("FAIL {path}: seq {i} ({}) frame {frame}: degenerate bones", seq.label);
                }
            }
        }
    }
    println!("{models} models, {poses} poses, {failed} failures");
    if failed > 0 {
        bail!("{failed} failures");
    }
    Ok(())
}
