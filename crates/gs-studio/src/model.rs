//! `studiohdr_t` v10 parsing (docs/re/render.md §B.2–B.3). Everything is copied out of the file
//! with bounds checks; animation streams stay as raw bytes and are decoded on demand.

use std::sync::Arc;

use crate::StudioError;

const IDST: &[u8; 4] = b"IDST";
const VERSION: i32 = 10;
pub const MAX_BONES: usize = 128;
pub const MAX_ATTACHMENTS: usize = 4;

pub mod texture_flags {
    pub const FLATSHADE: i32 = 1;
    pub const CHROME: i32 = 2;
    pub const NOMIPS: i32 = 8;
    pub const ADDITIVE: i32 = 32;
    pub const MASKED: i32 = 64;
}

pub const STUDIO_LOOPING: i32 = 1;

struct Buf<'a>(&'a [u8]);

impl<'a> Buf<'a> {
    fn slice(&self, off: i32, len: usize) -> Result<&'a [u8], StudioError> {
        let start = usize::try_from(off).map_err(|_| StudioError::Truncated(off as i64))?;
        self.0.get(start..start + len).ok_or(StudioError::Truncated(start as i64))
    }

    fn i32(&self, off: i32) -> Result<i32, StudioError> {
        let s = self.slice(off, 4)?;
        Ok(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn i16(&self, off: i32) -> Result<i16, StudioError> {
        let s = self.slice(off, 2)?;
        Ok(i16::from_le_bytes([s[0], s[1]]))
    }

    fn f32(&self, off: i32) -> Result<f32, StudioError> {
        Ok(f32::from_bits(self.i32(off)? as u32))
    }

    fn vec3(&self, off: i32) -> Result<[f32; 3], StudioError> {
        Ok([self.f32(off)?, self.f32(off + 4)?, self.f32(off + 8)?])
    }

    fn name(&self, off: i32, len: usize) -> Result<String, StudioError> {
        let raw = self.slice(off, len)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(len);
        Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
    }

    fn count(&self, off: i32) -> Result<usize, StudioError> {
        Ok(usize::try_from(self.i32(off)?.max(0)).unwrap_or(0))
    }
}

#[derive(Debug, Clone)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    pub controller: [i32; 6],
    pub value: [f32; 6],
    pub scale: [f32; 6],
}

#[derive(Debug, Clone, Copy)]
pub struct BoneController {
    pub bone: i32,
    pub kind: i32,
    pub start: f32,
    pub end: f32,
    pub rest: i32,
    pub index: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct HitBox {
    pub bone: i32,
    pub group: i32,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Event {
    pub frame: i32,
    pub event: i32,
    pub kind: i32,
    pub options: String,
}

#[derive(Debug, Clone)]
pub struct SeqDesc {
    pub label: String,
    pub fps: f32,
    pub flags: i32,
    pub activity: i32,
    pub events: Vec<Event>,
    pub numframes: i32,
    pub motiontype: i32,
    pub motionbone: i32,
    pub linearmovement: [f32; 3],
    pub bbmin: [f32; 3],
    pub bbmax: [f32; 3],
    pub numblends: i32,
    pub animindex: i32,
    pub seqgroup: i32,
}

#[derive(Debug, Clone)]
pub struct SeqGroup {
    pub label: String,
    /// File name with backslashes, e.g. `models\hostage01.mdl`.
    pub name: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Attachment {
    pub bone: i32,
    pub org: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct StudioTexture {
    pub name: String,
    pub flags: i32,
    pub width: u32,
    pub height: u32,
    /// 8-bit indices.
    pub pixels: Vec<u8>,
    pub palette: Vec<[u8; 3]>,
}

/// One `tricmd` run: a strip (`fan = false`) or fan of `(vert, norm, s, t)` vertices.
#[derive(Debug, Clone)]
pub struct TriCmd {
    pub fan: bool,
    pub verts: Vec<[i16; 4]>,
}

#[derive(Debug, Clone)]
pub struct Mesh {
    pub skinref: i32,
    pub numnorms: i32,
    pub cmds: Vec<TriCmd>,
}

#[derive(Debug, Clone)]
pub struct SubModel {
    pub name: String,
    pub verts: Vec<[f32; 3]>,
    pub vert_bones: Vec<u8>,
    pub norms: Vec<[f32; 3]>,
    pub norm_bones: Vec<u8>,
    pub meshes: Vec<Mesh>,
}

#[derive(Debug, Clone)]
pub struct BodyPart {
    pub name: String,
    pub base: i32,
    pub models: Vec<SubModel>,
}

#[derive(Debug, Clone)]
pub struct StudioModel {
    pub name: String,
    pub eyeposition: [f32; 3],
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub bbmin: [f32; 3],
    pub bbmax: [f32; 3],
    pub flags: i32,
    pub bones: Vec<Bone>,
    pub controllers: Vec<BoneController>,
    pub hitboxes: Vec<HitBox>,
    pub sequences: Vec<SeqDesc>,
    pub seqgroups: Vec<SeqGroup>,
    pub bodyparts: Vec<BodyPart>,
    pub attachments: Vec<Attachment>,
    /// Textures of this file (empty when they live in `<name>T.mdl`).
    pub textures: Vec<StudioTexture>,
    /// `skinref[family][ref]`.
    pub skins: Vec<Vec<i16>>,
    /// Whole file; animation streams are decoded from it (sequence group 0).
    pub data: Arc<[u8]>,
    /// Loaded `<name>NN.mdl` group files, indexed by sequence group (0 unused).
    pub group_data: Vec<Option<Arc<[u8]>>>,
}

impl StudioModel {
    /// Header `textureindex == 0`: textures live in `<name>T.mdl`.
    pub fn needs_texture_model(&self) -> bool {
        self.textures.is_empty()
    }

    pub fn parse(data: Arc<[u8]>) -> Result<Self, StudioError> {
        let b = Buf(&data);
        if b.slice(0, 4)? != IDST {
            return Err(StudioError::BadHeader("not an IDST model".into()));
        }
        let version = b.i32(4)?;
        if version != VERSION {
            return Err(StudioError::BadHeader(format!("version {version}")));
        }
        let name = b.name(8, 64)?;

        let mut bones = Vec::new();
        let (n, at) = (b.count(140)?, b.i32(144)?);
        if n > MAX_BONES {
            return Err(StudioError::BadHeader(format!("{n} bones")));
        }
        for i in 0..n as i32 {
            let o = at + 112 * i;
            let mut controller = [0; 6];
            let mut value = [0.0; 6];
            let mut scale = [0.0; 6];
            for j in 0..6 {
                controller[j as usize] = b.i32(o + 40 + 4 * j)?;
                value[j as usize] = b.f32(o + 64 + 4 * j)?;
                scale[j as usize] = b.f32(o + 88 + 4 * j)?;
            }
            bones.push(Bone { name: b.name(o, 32)?, parent: b.i32(o + 32)?, controller, value, scale });
        }

        let (n, at) = (b.count(148)?, b.i32(152)?);
        let controllers = (0..n as i32)
            .map(|i| {
                let o = at + 24 * i;
                Ok(BoneController {
                    bone: b.i32(o)?,
                    kind: b.i32(o + 4)?,
                    start: b.f32(o + 8)?,
                    end: b.f32(o + 12)?,
                    rest: b.i32(o + 16)?,
                    index: b.i32(o + 20)?,
                })
            })
            .collect::<Result<_, StudioError>>()?;

        let (n, at) = (b.count(156)?, b.i32(160)?);
        let hitboxes = (0..n as i32)
            .map(|i| {
                let o = at + 32 * i;
                Ok(HitBox { bone: b.i32(o)?, group: b.i32(o + 4)?, mins: b.vec3(o + 8)?, maxs: b.vec3(o + 20)? })
            })
            .collect::<Result<_, StudioError>>()?;

        let (n, at) = (b.count(164)?, b.i32(168)?);
        let sequences = (0..n as i32)
            .map(|i| {
                let o = at + 176 * i;
                let (ne, ei) = (b.count(o + 48)?, b.i32(o + 52)?);
                let events = (0..ne as i32)
                    .map(|k| {
                        let e = ei + 76 * k;
                        Ok(Event { frame: b.i32(e)?, event: b.i32(e + 4)?, kind: b.i32(e + 8)?, options: b.name(e + 12, 64)? })
                    })
                    .collect::<Result<_, StudioError>>()?;
                Ok(SeqDesc {
                    label: b.name(o, 32)?,
                    fps: b.f32(o + 32)?,
                    flags: b.i32(o + 36)?,
                    activity: b.i32(o + 40)?,
                    events,
                    numframes: b.i32(o + 56)?,
                    motiontype: b.i32(o + 68)?,
                    motionbone: b.i32(o + 72)?,
                    linearmovement: b.vec3(o + 76)?,
                    bbmin: b.vec3(o + 96)?,
                    bbmax: b.vec3(o + 108)?,
                    numblends: b.i32(o + 120)?,
                    animindex: b.i32(o + 124)?,
                    seqgroup: b.i32(o + 156)?,
                })
            })
            .collect::<Result<_, StudioError>>()?;

        let (n, at) = (b.count(172)?, b.i32(176)?);
        let seqgroups = (0..n as i32)
            .map(|i| {
                let o = at + 104 * i;
                Ok(SeqGroup { label: b.name(o, 32)?, name: b.name(o + 32, 64)? })
            })
            .collect::<Result<Vec<_>, StudioError>>()?;

        let (textures, skins) = parse_textures(&b)?;

        let (n, at) = (b.count(204)?, b.i32(208)?);
        let bodyparts = (0..n as i32)
            .map(|i| {
                let o = at + 76 * i;
                let (nm, mi) = (b.count(o + 64)?, b.i32(o + 72)?);
                let models = (0..nm as i32).map(|m| parse_submodel(&b, mi + 112 * m)).collect::<Result<_, _>>()?;
                Ok(BodyPart { name: b.name(o, 64)?, base: b.i32(o + 68)?, models })
            })
            .collect::<Result<_, StudioError>>()?;

        let (n, at) = (b.count(212)?, b.i32(216)?);
        if n > MAX_ATTACHMENTS {
            return Err(StudioError::BadHeader(format!("Too many attachments on {name}")));
        }
        let attachments = (0..n as i32)
            .map(|i| {
                let o = at + 88 * i;
                Ok(Attachment { bone: b.i32(o + 36)?, org: b.vec3(o + 40)? })
            })
            .collect::<Result<_, StudioError>>()?;

        Ok(Self {
            name,
            eyeposition: b.vec3(76)?,
            min: b.vec3(88)?,
            max: b.vec3(100)?,
            bbmin: b.vec3(112)?,
            bbmax: b.vec3(124)?,
            flags: b.i32(136)?,
            bones,
            controllers,
            hitboxes,
            sequences,
            group_data: vec![None; seqgroups.len()],
            seqgroups,
            bodyparts,
            attachments,
            textures,
            skins,
            data,
        })
    }

    /// Textures and skin families from a `<name>T.mdl` file.
    pub fn parse_texture_model(data: &[u8]) -> Result<(Vec<StudioTexture>, Vec<Vec<i16>>), StudioError> {
        let b = Buf(data);
        if b.slice(0, 4)? != IDST {
            return Err(StudioError::BadHeader("texture model is not IDST".into()));
        }
        parse_textures(&b)
    }

    /// `R_StudioSetupModel`: the submodel `body` selects in a bodypart.
    pub fn submodel(&self, bodypart: usize, body: i32) -> Option<&SubModel> {
        self.bodyparts.get(bodypart)?.models.get(self.submodel_index(bodypart, body)?)
    }

    /// [`Self::submodel`]'s index in the bodypart's model list.
    pub fn submodel_index(&self, bodypart: usize, body: i32) -> Option<usize> {
        let part = self.bodyparts.get(bodypart)?;
        if part.models.is_empty() {
            return None;
        }
        let base = part.base.max(1);
        Some((body / base).rem_euclid(part.models.len() as i32) as usize)
    }
}

fn parse_textures(b: &Buf) -> Result<(Vec<StudioTexture>, Vec<Vec<i16>>), StudioError> {
    let (n, at) = (b.count(180)?, b.i32(184)?);
    let mut textures = Vec::new();
    if at != 0 {
        for i in 0..n as i32 {
            let o = at + 80 * i;
            let (w, h, index) = (b.i32(o + 68)?, b.i32(o + 72)?, b.i32(o + 76)?);
            let (Ok(width), Ok(height)) = (u32::try_from(w), u32::try_from(h)) else {
                return Err(StudioError::BadHeader(format!("texture size {w}x{h}")));
            };
            let len = (width * height) as usize;
            let pixels = b.slice(index, len)?.to_vec();
            let pal = b.slice(index + len as i32, 768)?;
            textures.push(StudioTexture {
                name: b.name(o, 64)?,
                flags: b.i32(o + 64)?,
                width,
                height,
                pixels,
                palette: pal.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            });
        }
    }
    let (nref, nfam, si) = (b.count(192)?, b.count(196)?, b.i32(200)?);
    let skins = (0..nfam as i32)
        .map(|f| (0..nref as i32).map(|r| b.i16(si + 2 * (f * nref as i32 + r))).collect::<Result<Vec<_>, _>>())
        .collect::<Result<_, _>>()?;
    Ok((textures, skins))
}

fn parse_submodel(b: &Buf, o: i32) -> Result<SubModel, StudioError> {
    let (nmesh, meshindex) = (b.count(o + 72)?, b.i32(o + 76)?);
    let (nverts, vinfo, vindex) = (b.count(o + 80)?, b.i32(o + 84)?, b.i32(o + 88)?);
    let (nnorms, ninfo, nindex) = (b.count(o + 92)?, b.i32(o + 96)?, b.i32(o + 100)?);
    let verts = (0..nverts as i32).map(|i| b.vec3(vindex + 12 * i)).collect::<Result<_, _>>()?;
    let norms = (0..nnorms as i32).map(|i| b.vec3(nindex + 12 * i)).collect::<Result<_, _>>()?;
    let vert_bones = b.slice(vinfo, nverts)?.to_vec();
    let norm_bones = b.slice(ninfo, nnorms)?.to_vec();
    let meshes = (0..nmesh as i32)
        .map(|m| {
            let mo = meshindex + 20 * m;
            let mut at = b.i32(mo + 4)?;
            let mut cmds = Vec::new();
            loop {
                let count = b.i16(at)?;
                at += 2;
                if count == 0 {
                    break;
                }
                let n = i32::from(count).unsigned_abs() as i32;
                let verts = (0..n)
                    .map(|k| {
                        let v = at + 8 * k;
                        Ok([b.i16(v)?, b.i16(v + 2)?, b.i16(v + 4)?, b.i16(v + 6)?])
                    })
                    .collect::<Result<_, StudioError>>()?;
                at += 8 * n;
                cmds.push(TriCmd { fan: count < 0, verts });
            }
            Ok(Mesh { skinref: b.i32(mo + 8)?, numnorms: b.i32(mo + 12)?, cmds })
        })
        .collect::<Result<_, StudioError>>()?;
    Ok(SubModel { name: b.name(o, 64)?, verts, vert_bones, norms, norm_bones, meshes })
}
