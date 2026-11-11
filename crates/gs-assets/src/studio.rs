//! Studio model loading with external textures (`<name>T.mdl`, §B.3.3) and sequence group files
//! (`seqgroup.name`, §B.3.4).

use std::sync::Arc;

use gs_fs::FileSystem;
use gs_studio::StudioModel;

use crate::AssetError;

/// The engine's cache_user array for sequence groups has 16 entries.
const MAX_SEQGROUPS: usize = 16;

pub fn load_studio(fs: &FileSystem, path: &str) -> Result<StudioModel, AssetError> {
    let data: Arc<[u8]> = fs.read(path)?.into();
    let studio_err = |source: gs_studio::StudioError, path: &str| AssetError::Decode { path: path.to_owned(), detail: source.to_string() };
    let mut model = StudioModel::parse(data).map_err(|e| studio_err(e, path))?;
    if model.needs_texture_model() {
        let stem = path.get(..path.len().saturating_sub(4)).unwrap_or(path);
        let tpath = format!("{stem}T.mdl");
        let tdata = fs.read(&tpath)?;
        let (textures, skins) = StudioModel::parse_texture_model(&tdata).map_err(|e| studio_err(e, &tpath))?;
        model.textures = textures;
        model.skins = skins;
    }
    for group in 1..model.seqgroups.len().min(MAX_SEQGROUPS) {
        let name = model.seqgroups[group].name.clone();
        match fs.read(&name) {
            Ok(bytes) => model.group_data[group] = Some(bytes.into()),
            Err(err) => log::warn!("{path}: sequence group {group} ({name}): {err}"),
        }
    }
    Ok(model)
}
