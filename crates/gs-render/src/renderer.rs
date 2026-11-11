use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use gs_assets::MapAssets;
use gs_formats::bsp::Bsp;
use gs_formats::sprite::{Sprite, SpriteFrameSlot};
use gs_studio::StudioModel;
use gs_studio::model::texture_flags;
use gs_world::World;
use wgpu::util::DeviceExt;

use crate::camera::{self, NORMAL_STYLE, NUM_LIGHTSTYLES, View};
use crate::gamma::{GammaSettings, GammaTables};
use crate::lightmap::{DynamicLight, Lightmaps};
use crate::overlay::OverlayPass;
use crate::sprite::{SpriteDraw, SpriteHandle, SpriteSource, SpriteView};
use crate::studio::{
    AlphaTest, Blend, ChromeView, LightEnv, LightTarget, MeshDraw, PipelineKey, PointLight, StudioDraw, StudioHandle,
    StudioLight, StudioSource, StudioVertex, TexEnv,
};
use crate::decal::{DecalShot, DecalWad, Decals};
use crate::world::{Batch, SurfaceKind, WorldMesh, WorldVertex};
use crate::{RenderError, texture};

/// Everything one frame draws besides the world.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub brushes: Vec<BrushInstance>,
    pub studios: Vec<StudioDraw>,
    pub sprites: Vec<SpriteDraw>,
    /// CS sprite shadows (`StudioRenderShadow`), drawn with the opaque entities.
    pub shadows: Vec<EffectBatch>,
    /// `HUD_DrawTransparentTriangles` (the particle manager's smoke), drawn right after the
    /// translucent entities.
    pub transparent: Vec<EffectBatch>,
    /// `R_DrawParticles` triangles (particle dot texture), drawn after translucent entities.
    pub particles: Vec<StudioVertex>,
    /// Tracers and beams, drawn after the particles.
    pub effects: Vec<EffectBatch>,
    /// `HUD_DrawNormalTriangles` (the spectator overview), drawn after the opaque entities
    /// with the fog off.
    pub normal: Vec<EffectBatch>,
    /// `r_refdef.onlyClientDraws`: no world, entities, particles or view model; only the
    /// client's triangles.
    pub only_client_draw: bool,
    /// Live dynamic lights, added into the world lightmaps.
    pub dlights: Vec<DynamicLight>,
    /// The 2D overlay (HUD), drawn last in screen space.
    pub overlay: Vec<gs_draw::DrawCmd>,
}

/// TriAPI render mode of an [`EffectBatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectBlend {
    /// `kRenderNormal`: opaque, depth write.
    Opaque,
    /// `kRenderTransAlpha`: SRC_ALPHA/ONE_MINUS_SRC_ALPHA, no depth write.
    Alpha,
    /// `kRenderTransColor` / `kRenderTransTexture` (TriAPI 1/2): SRC_ALPHA/ONE_MINUS_SRC_ALPHA
    /// with the depth write left on.
    AlphaDepth,
    /// `kRenderTransAdd`: ONE,ONE, no depth write.
    Additive,
}

/// Triangles textured with one sprite frame (`tri.SpriteTexture`), no culling or alpha test.
#[derive(Debug, Clone)]
pub struct EffectBatch {
    pub sprite: SpriteHandle,
    pub frame: u32,
    pub blend: EffectBlend,
    pub vertices: Vec<StudioVertex>,
}

/// The 8×8 particle texture of `R_Init`: white, opaque on the 4×4 dot pattern
/// (docs/re/render.md §D.8.3).
fn particle_texture() -> Vec<u8> {
    const DOT: [[u8; 4]; 4] = [[0, 1, 1, 0], [1, 1, 1, 1], [1, 1, 1, 1], [0, 1, 1, 0]];
    (0..64)
        .flat_map(|i| {
            let on = DOT.get(i % 8).and_then(|row| row.get(i / 8)) == Some(&1);
            [255, 255, 255, if on { 255 } else { 0 }]
        })
        .collect()
}

/// GL state of `R_DrawParticles`.
const PARTICLE_KEY: PipelineKey = PipelineKey {
    blend: Blend::Alpha,
    depth_write: true,
    depth_test: true,
    alpha_test: AlphaTest::NotZero,
    texenv: TexEnv::Modulate,
    cull: false,
};

/// GL state of an effect batch (shadows, tracers, beams).
fn effect_key(blend: EffectBlend) -> PipelineKey {
    PipelineKey {
        blend: match blend {
            EffectBlend::Opaque => Blend::Opaque,
            EffectBlend::Alpha | EffectBlend::AlphaDepth => Blend::Alpha,
            EffectBlend::Additive => Blend::Additive,
        },
        depth_write: matches!(blend, EffectBlend::Opaque | EffectBlend::AlphaDepth),
        depth_test: true,
        alpha_test: AlphaTest::None,
        texenv: TexEnv::Modulate,
        cull: false,
    }
}

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Uniform slots for brush models drawn in one frame (slot 0 is the world).
const MAX_BRUSH_INSTANCES: usize = 1024;
const MODEL_SLOT: u64 = 256;

/// `kRender*` entity render modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    #[default]
    Normal,
    TransColor,
    TransTexture,
    Glow,
    TransAlpha,
    TransAdd,
}

impl RenderMode {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::TransColor,
            2 => Self::TransTexture,
            3 => Self::Glow,
            4 => Self::TransAlpha,
            5 => Self::TransAdd,
            _ => Self::Normal,
        }
    }
}

/// A brush submodel placed in the world (doors, func_walls, ...).
#[derive(Debug, Clone, Copy)]
pub struct BrushInstance {
    /// BSP model index, `>= 1`.
    pub model: usize,
    pub origin: Vec3,
    pub angles: Vec3,
    pub rendermode: RenderMode,
    /// `CL_FxBlend / 255`.
    pub blend: f32,
    pub color: [u8; 3],
}

impl BrushInstance {
    pub fn at(model: usize, origin: Vec3) -> Self {
        Self { model, origin, angles: Vec3::ZERO, rendermode: RenderMode::Normal, blend: 1.0, color: [255; 3] }
    }

    /// `R_RotateForEntity`: translate, then yaw about Z, pitch about Y (positive), roll about X.
    fn transform(&self) -> Mat4 {
        Mat4::from_translation(self.origin)
            * Mat4::from_rotation_z(self.angles.y.to_radians())
            * Mat4::from_rotation_y(self.angles.x.to_radians())
            * Mat4::from_rotation_x(self.angles.z.to_radians())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    params: [f32; 4],
    /// `vpn` (eye-space depth for fog).
    forward: [f32; 4],
    fog_color: [f32; 4],
    /// x: 0 off, 1 GL_LINEAR, 2 GL_EXP2; y: density; z: start; w: end.
    fog: [f32; 4],
}

/// Which GL fog state a draw sees (one frame-uniform slot each, `MODEL_SLOT` apart).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FogSlot {
    /// World and opaque entities: underwater linear fog, else the user fog.
    Opaque = 0,
    /// The `tri_Fog` user fog only (translucent brushes, the client's triangles, particles,
    /// the view model).
    User = 1,
    /// Fog disabled (translucent sprites and studio models, additive/glow brushes).
    Off = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ModelUniform {
    transform: [[f32; 4]; 4],
    blend: [f32; 4],
    color: [f32; 4],
}

struct Pipelines {
    lit: wgpu::RenderPipeline,
    lit_alpha_test: wgpu::RenderPipeline,
    water: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    trans_color: wgpu::RenderPipeline,
    trans_texture: wgpu::RenderPipeline,
    additive: wgpu::RenderPipeline,
    water_blend: wgpu::RenderPipeline,
    decal: wgpu::RenderPipeline,
}

struct WorldGpu {
    bsp: Bsp,
    spatial: World,
    lightmaps: Lightmaps,
    lightmap_texture: wgpu::Texture,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    materials: Vec<Material>,
    models: Vec<Vec<Batch>>,
    /// `waterFogColor` (`pal[3]`) and `waterFogDensity` (`pal[4].r`) of the world's last water
    /// texture: the engine takes them from the last `EmitWaterPolys`.
    water_fog: Option<([u8; 3], u8)>,
}

struct DepthTarget {
    size: (u32, u32),
    view: wgpu::TextureView,
}

struct StudioGpu {
    source: StudioSource,
    materials: Vec<Material>,
}

struct SpriteGpu {
    /// The sprite file (`None` for frames uploaded as RGBA tiles, drawn only through batches).
    source: Option<SpriteSource>,
    /// One material per frame slot (`None` for group frames, which never draw).
    frames: Vec<Option<Material>>,
}

/// The sampler a material is drawn with, from the shared groups bound at [`SAMPLER_GROUP`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SamplerKind {
    /// Mipmapped: `gl_texturemode`'s filters and `gl_ansio` as last set.
    Diffuse,
    /// No mips: `gl_filter_max` (`nearest` or linear) and `gl_ansio` of the mode in force at
    /// upload, `upload_groups[generation]`.
    Upload { generation: usize, nearest: bool },
    /// No mips, clamped to the edge.
    Clamp,
}

/// The bind group slot of the shared samplers. The material groups hold only the texture:
/// D3D12 gives every bind group with a sampler its own 64 KiB index buffer in the upload
/// heap, and one per texture kept hundreds of MiB reserved between freed staging copies.
const SAMPLER_GROUP: u32 = 3;

/// An uploaded texture's bind group and the sampler it is drawn with.
struct Material {
    group: wgpu::BindGroup,
    sampler: SamplerKind,
}

/// `gl_texturemode`'s modes (render.md §A.13.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureMode {
    Nearest,
    Linear,
    NearestMipmapNearest,
    LinearMipmapNearest,
    NearestMipmapLinear,
    LinearMipmapLinear,
}

impl TextureMode {
    /// `Draw_TextureMode_f`'s table lookup (`Q_strcasecmp`).
    pub fn from_name(name: &str) -> Option<Self> {
        const TABLE: [(&str, TextureMode); 6] = [
            ("GL_NEAREST", TextureMode::Nearest),
            ("GL_LINEAR", TextureMode::Linear),
            ("GL_NEAREST_MIPMAP_NEAREST", TextureMode::NearestMipmapNearest),
            ("GL_LINEAR_MIPMAP_NEAREST", TextureMode::LinearMipmapNearest),
            ("GL_NEAREST_MIPMAP_LINEAR", TextureMode::NearestMipmapLinear),
            ("GL_LINEAR_MIPMAP_LINEAR", TextureMode::LinearMipmapLinear),
        ];
        TABLE.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, m)| m)
    }

    /// `gl_filter_max`: magnification, and the one filter of textures uploaded without mips.
    fn mag(self) -> wgpu::FilterMode {
        match self {
            Self::Nearest | Self::NearestMipmapNearest | Self::NearestMipmapLinear => wgpu::FilterMode::Nearest,
            Self::Linear | Self::LinearMipmapNearest | Self::LinearMipmapLinear => wgpu::FilterMode::Linear,
        }
    }

    /// `gl_filter_min`'s mipmap filter; `None` samples level 0 only.
    fn mipmap(self) -> Option<wgpu::MipmapFilterMode> {
        match self {
            Self::Nearest | Self::Linear => None,
            Self::NearestMipmapNearest | Self::LinearMipmapNearest => Some(wgpu::MipmapFilterMode::Nearest),
            Self::NearestMipmapLinear | Self::LinearMipmapLinear => Some(wgpu::MipmapFilterMode::Linear),
        }
    }
}

/// `GL_TEXTURE_MAX_ANISOTROPY = gl_ansio` as wgpu takes it: 1, 2, 4, 8 or 16 (the driver
/// clamps to 16; a value below 1 is refused, leaving 1).
fn anisotropy(ansio: f32) -> u16 {
    let a = if ansio >= 1.0 { ansio.min(16.0) as u16 } else { 1 };
    1 << (15 - a.leading_zeros())
}

/// A repeating sampler: `mag` for magnification and minification, `mipmap` between levels
/// (`None`: level 0 only). wgpu allows anisotropy only when every filter is linear.
fn texture_sampler(device: &wgpu::Device, label: &str, mag: wgpu::FilterMode, mipmap: Option<wgpu::MipmapFilterMode>, ansio: f32) -> wgpu::Sampler {
    let linear = mag == wgpu::FilterMode::Linear && mipmap != Some(wgpu::MipmapFilterMode::Nearest);
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: mag,
        min_filter: mag,
        mipmap_filter: mipmap.unwrap_or(wgpu::MipmapFilterMode::Linear),
        lod_max_clamp: if mipmap.is_some() { 32.0 } else { 0.0 },
        anisotropy_clamp: if linear { anisotropy(ansio) } else { 1 },
        ..Default::default()
    })
}

fn bind_material(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, label: &str, view: &wgpu::TextureView) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }],
    })
}

/// A shared sampler group (bound at [`SAMPLER_GROUP`]).
fn bind_sampler(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sampler"),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(sampler) }],
    })
}

/// The groups of the samplers of textures uploaded without mips: linear, nearest.
fn upload_sampler_groups(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, ansio: f32) -> [wgpu::BindGroup; 2] {
    [wgpu::FilterMode::Linear, wgpu::FilterMode::Nearest].map(|f| bind_sampler(device, layout, &texture_sampler(device, "upload", f, None, ansio)))
}

/// CPU-built geometry drawn with per-vertex colour: studio models and sprites.
struct StudioPass {
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    models: Vec<StudioGpu>,
    sprites: Vec<SpriteGpu>,
    vertices: Option<wgpu::Buffer>,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    color_format: wgpu::TextureFormat,
    gamma: GammaTables,
    frame_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    model_buffer: wgpu::Buffer,
    model_group: wgpu::BindGroup,
    /// The shared sampler groups' layout ([`SAMPLER_GROUP`]).
    sampler_layout: wgpu::BindGroupLayout,
    /// Mipmapped textures: `gl_filter_min`/`gl_filter_max` and `gl_ansio` of the mode last set.
    diffuse_group: wgpu::BindGroup,
    /// Textures uploaded without mips take `gl_filter_max` and `gl_ansio` at upload: one pair
    /// (linear, nearest) per texture mode set so far.
    upload_groups: Vec<[wgpu::BindGroup; 2]>,
    texture_mode: TextureMode,
    /// No mipmaps, clamped to the edge (the overview tiles, the VGUI surface's images).
    clamp_group: wgpu::BindGroup,
    lightmap_sampler: wgpu::Sampler,
    sky_sampler: wgpu::Sampler,
    pipelines: Pipelines,
    studio: StudioPass,
    overlay: OverlayPass,
    /// Frame materials of the 2D sprites, by overlay texture id.
    overlay_sprites: Vec<Vec<Option<Material>>>,
    depth: Option<DepthTarget>,
    /// The 4× colour target the whole frame (3D and 2D) is drawn into, resolved into the
    /// caller's target (engine-video.md §5.3.9); `samples == 1` draws directly.
    samples: u32,
    msaa: Option<DepthTarget>,
    world: Option<WorldGpu>,
    /// The particle dot texture (`particletexture`).
    particle_material: Option<Material>,
    /// The merged `decals.wad`.
    decal_wad: DecalWad,
    /// Spray logos: each player's decal list entry and the logo's MD5.
    custom_decals: HashMap<usize, (usize, [u8; 16])>,
    /// Uploaded decal textures by decal list index.
    decal_materials: HashMap<usize, Material>,
    decals: Decals,
    decal_vertices: Option<wgpu::Buffer>,
    /// `(brush model, decal texture, vertex range)` in draw order.
    decal_draws: Vec<(usize, usize, std::ops::Range<u32>)>,
}

impl Renderer {
    /// `samples`: 4 for the windowed default framebuffer and the fullscreen MSAA FBO, 1 when
    /// MSAA is off (`-nomsaa` fullscreen, the exclusive `-nofbo` mode).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, color_format: wgpu::TextureFormat, samples: u32) -> Self {
        let tex_entry = |binding, dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform_entry = |binding, dynamic| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                uniform_entry(0, true),
                tex_entry(1, wgpu::TextureViewDimension::D2),
                sampler_entry(2),
                tex_entry(3, wgpu::TextureViewDimension::D2Array),
                sampler_entry(4),
            ],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[tex_entry(0, wgpu::TextureViewDimension::D2)],
        });
        let sampler_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("sampler"), entries: &[sampler_entry(0)] });
        let model_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model"),
            entries: &[uniform_entry(0, true)],
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniform"),
            size: MODEL_SLOT * 3,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model uniforms"),
            size: MODEL_SLOT * MAX_BRUSH_INSTANCES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model"),
            layout: &model_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &model_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<ModelUniform>() as u64),
                }),
            }],
        });
        // gl_texturemode GL_LINEAR_MIPMAP_LINEAR and gl_ansio 16 until the game applies the cvars.
        let texture_mode = TextureMode::LinearMipmapLinear;
        let diffuse_group = bind_sampler(device, &sampler_layout, &texture_sampler(device, "diffuse", texture_mode.mag(), texture_mode.mipmap(), 16.0));
        let upload_groups = vec![upload_sampler_groups(device, &sampler_layout, 16.0)];
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let clamp_group = bind_sampler(device, &sampler_layout, &clamp_sampler);
        let linear = |label| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        };
        let lightmap_sampler = linear("lightmap");
        let sky_sampler = linear("sky");

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/world.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world"),
            bind_group_layouts: &[Some(&frame_layout), Some(&material_layout), Some(&model_layout), Some(&sampler_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32];
        let pipeline = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>, depth_write: bool, bias: wgpu::DepthBiasState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<WorldVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    // Faces are emitted clockwise as seen from the front (glCullFace(GL_FRONT) on CCW).
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias,
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: color_format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let additive = Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::OVER,
        });
        let flat = wgpu::DepthBiasState::default();
        // glPolygonOffset(-1, -gl_polyoffset) with gl_polyoffset 4.
        let decal_bias = wgpu::DepthBiasState { constant: -4, slope_scale: -1.0, clamp: 0.0 };
        let pipelines = Pipelines {
            lit: pipeline("lit", "vs_main", "fs_lit", None, true, flat),
            lit_alpha_test: pipeline("lit alpha test", "vs_main", "fs_lit_alpha_test", None, true, flat),
            water: pipeline("water", "vs_water", "fs_water", None, true, flat),
            water_blend: pipeline("water blend", "vs_water", "fs_water", alpha, false, flat),
            sky: pipeline("sky", "vs_main", "fs_sky", None, true, flat),
            trans_color: pipeline("trans color", "vs_main", "fs_trans_color", alpha, true, flat),
            trans_texture: pipeline("trans texture", "vs_main", "fs_trans_texture", alpha, false, flat),
            additive: pipeline("additive", "vs_main", "fs_additive", additive, false, flat),
            decal: pipeline("decal", "vs_main", "fs_decal", alpha, false, decal_bias),
        };
        let overlay = OverlayPass::new(device, color_format, DEPTH_FORMAT, &material_layout, &sampler_layout, samples);
        let studio = StudioPass {
            shader: device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("studio.wgsl"),
                source: wgpu::ShaderSource::Wgsl(include_str!("shaders/studio.wgsl").into()),
            }),
            layout: device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("studio"),
                bind_group_layouts: &[Some(&frame_layout), Some(&material_layout), None, Some(&sampler_layout)],
                immediate_size: 0,
            }),
            pipelines: HashMap::new(),
            models: Vec::new(),
            sprites: Vec::new(),
            vertices: None,
        };

        let mut renderer = Self {
            device: device.clone(),
            queue: queue.clone(),
            color_format,
            gamma: GammaTables::new(GammaSettings::default()),
            frame_layout,
            material_layout,
            frame_buffer,
            model_buffer,
            model_group,
            sampler_layout,
            diffuse_group,
            upload_groups,
            texture_mode,
            clamp_group,
            lightmap_sampler,
            sky_sampler,
            pipelines,
            studio,
            overlay,
            overlay_sprites: Vec::new(),
            depth: None,
            samples,
            msaa: None,
            world: None,
            particle_material: None,
            decal_wad: DecalWad::default(),
            custom_decals: HashMap::new(),
            decal_materials: HashMap::new(),
            decals: Decals::default(),
            decal_vertices: None,
            decal_draws: Vec::new(),
        };
        renderer.particle_material = Some(renderer.material("particletexture", 8, 8, &particle_texture(), false));
        renderer.warm_studio_pipelines();
        renderer
    }

    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
    }

    pub fn gamma(&self) -> &GammaTables {
        &self.gamma
    }

    /// Drops every uploaded studio model and sprite; previously returned handles become invalid.
    pub fn clear_models(&mut self) {
        self.studio.models.clear();
        self.studio.sprites.clear();
    }

    /// Uploads `rgba` as a texture drawn with the sampler of its kind: a mipmapped one with the
    /// `gl_texturemode` sampler, one without mips with the mode's `gl_filter_max` as of now.
    fn material(&self, label: &str, width: u32, height: u32, rgba: &[u8], mipmaps: bool) -> Material {
        let sampler = if mipmaps {
            SamplerKind::Diffuse
        } else {
            SamplerKind::Upload { generation: self.upload_groups.len().saturating_sub(1), nearest: self.texture_mode.mag() == wgpu::FilterMode::Nearest }
        };
        self.material_with(label, width, height, rgba, mipmaps, sampler)
    }

    fn material_with(&self, label: &str, width: u32, height: u32, rgba: &[u8], mipmaps: bool, sampler: SamplerKind) -> Material {
        let gpu = texture::upload_rgba(&self.device, &self.queue, label, width, height, rgba, mipmaps);
        let group = bind_material(&self.device, &self.material_layout, label, &gpu.create_view(&Default::default()));
        Material { group, sampler }
    }

    /// The shared group of a material's sampler.
    fn sampler_group(&self, kind: SamplerKind) -> &wgpu::BindGroup {
        match kind {
            SamplerKind::Diffuse => &self.diffuse_group,
            SamplerKind::Upload { generation, nearest } => {
                self.upload_groups.get(generation).or(self.upload_groups.last()).map_or(&self.clamp_group, |g| &g[usize::from(nearest)])
            }
            SamplerKind::Clamp => &self.clamp_group,
        }
    }

    /// Binds `material`'s texture and its sampler.
    fn bind(&self, pass: &mut wgpu::RenderPass, material: &Material) {
        pass.set_bind_group(1, &material.group, &[]);
        pass.set_bind_group(SAMPLER_GROUP, self.sampler_group(material.sampler), &[]);
    }

    /// `Draw_TextureMode_f`: `gl_filter_min`/`gl_filter_max` from `mode` and anisotropy
    /// `ansio` for every mipmapped texture uploaded so far and to come; textures without mips
    /// keep the filter they were uploaded with (render.md §A.13.3).
    pub fn set_texture_mode(&mut self, mode: TextureMode, ansio: f32) {
        self.texture_mode = mode;
        let diffuse = texture_sampler(&self.device, "diffuse", mode.mag(), mode.mipmap(), ansio);
        self.diffuse_group = bind_sampler(&self.device, &self.sampler_layout, &diffuse);
        self.upload_groups.push(upload_sampler_groups(&self.device, &self.sampler_layout, ansio));
    }

    /// Uploads the 2D glyph atlas region changed since the last call.
    pub fn update_glyphs(&mut self, atlas: &mut gs_draw::GlyphAtlas) {
        self.overlay.update_glyphs(&self.device, &self.queue, atlas);
    }

    /// Sends the queued uploads (a level's textures and models) to the GPU and waits for
    /// them, so the first frame that draws them does not pay for the copies (13 ms at 1440p).
    pub fn finish_uploads(&self) {
        self.queue.submit([]);
        if let Err(e) = self.device.poll(wgpu::PollType::wait_indefinitely()) {
            log::warn!("finishing uploads: {e}");
        }
    }

    /// Materials for every single frame of a sprite: world sprites mipmapped (`GLT_SPRITE`),
    /// HUD sprites without mips (`GLT_HUDSPRITE`).
    fn sprite_frames(&self, name: &str, sprite: &Sprite, mipmaps: bool) -> Vec<Option<Material>> {
        sprite
            .frames
            .iter()
            .enumerate()
            .map(|(i, slot)| match slot {
                SpriteFrameSlot::Single(f) => {
                    let rgba = crate::sprite::frame_rgba(sprite, f, &self.gamma);
                    Some(self.material(&format!("{name}_{i}"), f.width, f.height, &rgba, mipmaps))
                }
                SpriteFrameSlot::Group(_) => None,
            })
            .collect()
    }

    /// Uploads a world sprite for this level.
    pub fn upload_sprite(&mut self, name: &str, sprite: Arc<Sprite>) -> SpriteHandle {
        let frames = self.sprite_frames(name, &sprite, true);
        let handle = SpriteHandle(self.studio.sprites.len() as u32);
        self.studio.sprites.push(SpriteGpu { source: Some(SpriteSource { sprite }), frames });
        handle
    }

    /// Uploads RGBA frames (`cle_LoadMapSprite`'s 128×128 overview tiles: no mipmaps, clamped)
    /// as a level sprite usable by triangle batches.
    pub fn upload_tiles(&mut self, name: &str, tiles: &[(u32, u32, Vec<u8>)]) -> SpriteHandle {
        let frames = tiles.iter().enumerate().map(|(i, (w, h, rgba))| Some(self.material_with(&format!("{name}_{i}"), *w, *h, rgba, false, SamplerKind::Clamp))).collect();
        let handle = SpriteHandle(self.studio.sprites.len() as u32);
        self.studio.sprites.push(SpriteGpu { source: None, frames });
        handle
    }

    /// Uploads a 2D (HUD) sprite; these live for the session, across levels (the engine's HUD
    /// sprite table is never cleared). Returns the overlay texture id.
    pub fn upload_overlay_sprite(&mut self, name: &str, sprite: &Sprite) -> u32 {
        let frames = self.sprite_frames(name, sprite, false);
        self.overlay_sprites.push(frames);
        (self.overlay_sprites.len() - 1) as u32
    }

    /// Uploads a 2D VGUI image (RGBA8 as decoded) for the session. Returns the overlay texture
    /// id; the image is its frame 0. Sampled like `EngineSurface::DrawSetTextureRGBA`'s
    /// `GL_CLAMP_TO_EDGE`: tiled images (the menu background) show no seams.
    pub fn upload_overlay_image(&mut self, name: &str, width: u32, height: u32, rgba: &[u8]) -> u32 {
        let material = self.material_with(name, width, height, rgba, false, SamplerKind::Clamp);
        self.overlay_sprites.push(vec![Some(material)]);
        (self.overlay_sprites.len() - 1) as u32
    }

    /// Replaces overlay image `slot` (a forced reload); a new slot when `slot` is unknown.
    pub fn replace_overlay_image(&mut self, slot: u32, name: &str, width: u32, height: u32, rgba: &[u8]) -> u32 {
        let material = self.material_with(name, width, height, rgba, false, SamplerKind::Clamp);
        match self.overlay_sprites.get_mut(slot as usize) {
            Some(s) => {
                *s = vec![Some(material)];
                slot
            }
            None => {
                self.overlay_sprites.push(vec![Some(material)]);
                (self.overlay_sprites.len() - 1) as u32
            }
        }
    }

    /// Uploads a studio model's textures (`Mod_LoadStudioModel` §B.3.1) and keeps its geometry.
    pub fn upload_studio(&mut self, model: Arc<StudioModel>) -> StudioHandle {
        let mut textures = Vec::with_capacity(model.textures.len());
        let mut materials = Vec::with_capacity(model.textures.len());
        for tex in &model.textures {
            let masked = tex.flags & texture_flags::MASKED != 0;
            let rgba = texture::indexed_rgba(&tex.pixels, &tex.palette, tex.width, tex.height, masked, &self.gamma);
            let mipmaps = tex.flags & texture_flags::NOMIPS == 0;
            let label = format!("{}{}", model.name, tex.name);
            materials.push(self.material(&label, tex.width, tex.height, &rgba, mipmaps));
            textures.push((tex.width, tex.height, tex.flags));
        }
        let handle = StudioHandle(self.studio.models.len() as u32);
        self.studio.models.push(StudioGpu { source: StudioSource { model, textures }, materials });
        handle
    }

    /// `StudioDynamicLight` against the loaded world (`None` without a map).
    pub fn studio_light(
        &self,
        target: &LightTarget,
        styles: &[i32; NUM_LIGHTSTYLES],
        sky_color: [f32; 3],
        sky_vec: [f32; 3],
        dlights: &[PointLight],
    ) -> Option<StudioLight> {
        let world = self.world.as_ref()?;
        let env = LightEnv { world: &world.spatial, styles, sky_color, sky_vec, dlights };
        Some(crate::studio::dynamic_light(&env, target))
    }

    /// Every studio pipeline a frame can ask for, made up front: Direct3D 12 compiles a
    /// pipeline's shaders when it is made (FXC, 10-20 ms each), which mid-game dropped frames the
    /// first time a kind of mesh, sprite or effect showed.
    fn warm_studio_pipelines(&mut self) {
        use texture_flags::{ADDITIVE, MASKED};
        const MODES: [RenderMode; 6] =
            [RenderMode::Normal, RenderMode::TransColor, RenderMode::TransTexture, RenderMode::Glow, RenderMode::TransAlpha, RenderMode::TransAdd];
        let mut keys = vec![PARTICLE_KEY];
        for mirrored in [false, true] {
            for mode in MODES {
                keys.extend([0, MASKED, ADDITIVE, MASKED | ADDITIVE].map(|flags| crate::studio::mesh_key(mode, flags, mirrored)));
            }
            keys.extend([0, MASKED].map(|flags| crate::studio::chrome_key(flags, mirrored)));
        }
        keys.extend(MODES.map(crate::sprite::key));
        keys.extend([EffectBlend::Opaque, EffectBlend::Alpha, EffectBlend::AlphaDepth, EffectBlend::Additive].map(effect_key));
        let start = std::time::Instant::now();
        for key in keys {
            self.studio_pipeline(key);
        }
        log::info!("studio pipelines: {} made in {:.0} ms", self.studio.pipelines.len(), start.elapsed().as_secs_f64() * 1000.0);
    }

    fn studio_pipeline(&mut self, key: PipelineKey) -> &wgpu::RenderPipeline {
        let (device, pass, format, samples) = (&self.device, &mut self.studio, self.color_format, self.samples);
        pass.pipelines.entry(key).or_insert_with(|| {
            let blend = match key.blend {
                Blend::Opaque => None,
                Blend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
                Blend::Additive => Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
            };
            let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("studio"),
                layout: Some(&pass.layout),
                vertex: wgpu::VertexState {
                    module: &pass.shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<StudioVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    // glCullFace(GL_FRONT) with the default CCW front face, as for brushes.
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: key.cull.then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(key.depth_write),
                    depth_compare: Some(if key.depth_test { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Always }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &pass.shader,
                    entry_point: Some(match (key.texenv, key.alpha_test) {
                        (TexEnv::Replace, _) => "fs_replace_nonzero",
                        (TexEnv::Modulate, AlphaTest::None) => "fs_main",
                        (TexEnv::Modulate, AlphaTest::Greater05) => "fs_alpha_test",
                        (TexEnv::Modulate, AlphaTest::NotZero) => "fs_alpha_nonzero",
                    }),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub(crate) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Uploads the map's brush models, textures, lightmaps and sky. Replaces any previous map.
    pub fn load_map(&mut self, map: &MapAssets) -> Result<(), RenderError> {
        let bsp = map.bsp.clone();
        let lightmaps = Lightmaps::build(&bsp, &self.gamma, &[NORMAL_STYLE; NUM_LIGHTSTYLES])?;
        let mesh = WorldMesh::build(&bsp, &lightmaps);
        let (device, queue) = (&self.device, &self.queue);
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("world vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("world indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let lightmap_texture = texture::upload_rgba(device, queue, "lightmaps", lightmaps.width, lightmaps.height, &lightmaps.pixels, false);
        let sky = self.upload_sky(map)?;
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.frame_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(size_of::<FrameUniform>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lightmap_texture.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.lightmap_sampler) },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&sky.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2Array),
                        ..Default::default()
                    })),
                },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sky_sampler) },
            ],
        });
        let materials = map
            .textures
            .iter()
            .enumerate()
            .map(|(i, tex)| {
                let gpu = match tex.as_ref().and_then(|t| Some((t, texture::miptex_rgba(t, &self.gamma)?))) {
                    Some((t, rgba)) => texture::upload_rgba(device, queue, &t.name, t.width, t.height, &rgba, true),
                    None => {
                        let (w, h, rgba) = missing_texture();
                        texture::upload_rgba(device, queue, &format!("missing #{i}"), w, h, &rgba, true)
                    }
                };
                let group = bind_material(device, &self.material_layout, "material", &gpu.create_view(&Default::default()));
                Material { group, sampler: SamplerKind::Diffuse }
            })
            .collect();
        log::info!(
            "{}: {} vertices, {} triangles, lightmaps {}x{}",
            map.name,
            mesh.vertices.len(),
            mesh.indices.len() / 3,
            lightmaps.width,
            lightmaps.height
        );
        let spatial = World::from_bsp(&bsp);
        let water_fog = mesh.models.first().and_then(|batches| batches.iter().rev().find(|b| b.kind == SurfaceKind::Water)).and_then(|b| {
            let palette = &map.textures.get(b.texture as usize)?.as_ref()?.data.as_ref()?.palette;
            Some((*palette.get(3)?, palette.get(4)?[0]))
        });
        self.world = Some(WorldGpu {
            bsp,
            spatial,
            lightmaps,
            lightmap_texture,
            vertices,
            indices,
            frame_group,
            materials,
            models: mesh.models,
            water_fog,
        });
        self.decals.clear();
        Ok(())
    }

    /// Installs the merged decal list (`Decal_Init`).
    pub fn set_decal_wad(&mut self, wad: DecalWad) {
        self.decal_wad = wad;
        self.decal_materials.clear();
        self.decals.clear();
        self.custom_decals.clear();
    }

    /// `Draw_DecalIndexFromName`: the decal list index of `name` (exact match; appended when
    /// unknown).
    pub fn decal_index(&mut self, name: &str) -> usize {
        self.decal_wad.index(name)
    }

    /// `r_decals`: how many recyclable decals exist at once.
    pub fn set_decal_limit(&mut self, limit: usize) {
        self.decals.limit = limit;
    }

    /// `R_DecalShoot(texture, entity, model, pos, flags)`: `model` is the brush model the
    /// entity uses (0 = world); `origin`/`angles` are the entity's.
    #[allow(clippy::too_many_arguments)]
    pub fn decal_shoot(
        &mut self,
        texture: usize,
        model: usize,
        entity: i32,
        origin: [f32; 3],
        angles: [f32; 3],
        pos: [f32; 3],
        flags: u16,
        random_float: &mut dyn FnMut(f32, f32) -> f32,
    ) {
        let Some(world) = &self.world else { return };
        let Some(entry) = self.decal_wad.entries.get(texture) else { return };
        let Some(size) = self.decal_wad.size(texture) else {
            log::debug!("decal {}: no texture", entry.name);
            return;
        };
        let shot = DecalShot {
            bsp: &world.bsp,
            lightmaps: &world.lightmaps,
            model,
            entity,
            entity_origin: origin,
            entity_angles: angles,
            texture,
            size,
            name: &entry.name,
            pos,
            flags,
        };
        log::debug!("decal {} (#{texture}) on model {model} entity {entity} at {pos:?}", entry.name);
        self.decals.shoot(&shot, random_float);
    }

    /// `EV_RemoveAllDecals`' `R_DecalRemoveAll(i)` for every `i < r_decals`.
    pub fn decal_remove_all(&mut self) {
        self.decals.remove_all();
    }

    /// A player's spray logo (`Draw_CustomCacheGet`) as a decal list entry of its own, replaced
    /// (with the player's old sprays removed) when the logo changes.
    pub fn custom_decal(&mut self, player: usize, md5: [u8; 16], texture: &gs_formats::miptex::MipTex) -> usize {
        match self.custom_decals.get(&player).copied() {
            Some((entry, m)) if m == md5 => entry,
            Some((entry, _)) => {
                self.decals.remove_texture(entry);
                if let Some(e) = self.decal_wad.entries.get_mut(entry) {
                    e.texture = Some(texture.clone());
                }
                self.decal_materials.remove(&entry);
                self.custom_decals.insert(player, (entry, md5));
                entry
            }
            None => {
                let entry = self.decal_wad.entries.len();
                self.decal_wad.entries.push(crate::decal::DecalEntry { name: texture.name.clone(), texture: Some(texture.clone()) });
                self.custom_decals.insert(player, (entry, md5));
                entry
            }
        }
    }

    /// `R_DecalRemoveAll(~player)`: a replaced customization's sprays go.
    pub fn remove_custom_decals(&mut self, player: usize) {
        if let Some(&(entry, _)) = self.custom_decals.get(&player) {
            self.decals.remove_texture(entry);
        }
    }

    /// Rebuilds and uploads decal geometry after changes; uploads new decal textures.
    fn update_decals(&mut self) {
        if !self.decals.dirty {
            return;
        }
        self.decals.dirty = false;
        let mut vertices: Vec<WorldVertex> = Vec::new();
        let mut draws: Vec<(usize, usize, std::ops::Range<u32>)> = Vec::new();
        for (model, texture, polygon) in self.decals.polygons() {
            let start = vertices.len() as u32;
            for k in 1..polygon.len().saturating_sub(1) {
                vertices.extend_from_slice(&[polygon[0], polygon[k], polygon[k + 1]]);
            }
            let end = vertices.len() as u32;
            match draws.last_mut() {
                Some((m, t, r)) if *m == model && *t == texture && r.end == start => r.end = end,
                _ => draws.push((model, texture, start..end)),
            }
        }
        for &(_, texture, _) in &draws {
            if self.decal_materials.contains_key(&texture) {
                continue;
            }
            let Some(tex) = self.decal_wad.entries.get(texture).and_then(|e| e.texture.as_ref()) else { continue };
            let Some(rgba) = crate::decal::decal_rgba(tex, &self.gamma) else { continue };
            let material = self.material(&tex.name, tex.width, tex.height, &rgba, true);
            self.decal_materials.insert(texture, material);
        }
        self.decal_draws = draws;
        if vertices.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(&vertices);
        if self.decal_vertices.as_ref().is_none_or(|b| b.size() < bytes.len() as u64) {
            self.decal_vertices = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("decal vertices"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(buffer) = &self.decal_vertices {
            self.queue.write_buffer(buffer, 0, bytes);
        }
    }

    /// `R_DrawDecals` for one brush model (its model uniform slot is bound).
    fn draw_decals(&self, pass: &mut wgpu::RenderPass, model: usize) {
        let Some(buffer) = &self.decal_vertices else { return };
        let mut bound = false;
        for (m, texture, range) in &self.decal_draws {
            if *m != model {
                continue;
            }
            let Some(material) = self.decal_materials.get(texture) else { continue };
            if !bound {
                pass.set_pipeline(&self.pipelines.decal);
                pass.set_vertex_buffer(0, buffer.slice(..));
                bound = true;
            }
            self.bind(pass, material);
            pass.draw(range.clone(), 0..1);
        }
    }

    /// Sky TGAs get `texgammatable[c | c>>6]` on every channel byte below 252 (`gl_dither`).
    fn upload_sky(&self, map: &MapAssets) -> Result<wgpu::Texture, RenderError> {
        let (w, h, layers) = match &map.sky {
            Some(sky) => {
                let (w, h) = (sky.faces[0].width, sky.faces[0].height);
                let max = self.device.limits().max_texture_dimension_2d;
                if sky.faces.iter().any(|f| f.width != w || f.height != h) || w == 0 || h == 0 || w > max || h > max {
                    return Err(RenderError::SkySize(sky.name.clone()));
                }
                let mut px: Vec<u8> = sky.faces.iter().flat_map(|f| f.pixels.iter().copied()).collect();
                for (i, c) in px.iter_mut().enumerate() {
                    if i % 4 != 3 && *c < 0xFC {
                        *c = self.gamma.tex[usize::from(*c | (*c >> 6))];
                    }
                }
                // R_LoadSkys: six GL_LoadTexture uploads without mips.
                crate::texture::count_texels(6 * u64::from(w) * u64::from(h));
                (w, h, px)
            }
            None => (1, 1, [0u8, 0, 0, 255].repeat(6)),
        };
        // A seventh, unused layer: wgpu's GL backend makes a square texture of 6·n layers a cube
        // map, which the D2Array view of the sky shader then cannot sample (no sky on OpenGL).
        let mut layers = layers;
        layers.resize(layers.len() / 6 * 7, 0);
        Ok(self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("sky"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 7 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &layers,
        ))
    }

    fn depth_view(&mut self, size: (u32, u32)) -> wgpu::TextureView {
        if let Some(depth) = self.depth.as_ref().filter(|d| d.size == size) {
            return depth.view.clone();
        }
        let view = self.target_texture("depth", size, DEPTH_FORMAT);
        self.depth = Some(DepthTarget { size, view: view.clone() });
        view
    }

    /// The multisampled colour target, `None` without MSAA.
    fn msaa_view(&mut self, size: (u32, u32)) -> Option<wgpu::TextureView> {
        if self.samples <= 1 {
            return None;
        }
        if let Some(msaa) = self.msaa.as_ref().filter(|d| d.size == size) {
            return Some(msaa.view.clone());
        }
        let view = self.target_texture("msaa colour", size, self.color_format);
        self.msaa = Some(DepthTarget { size, view: view.clone() });
        Some(view)
    }

    fn target_texture(&self, label: &str, size: (u32, u32), format: wgpu::TextureFormat) -> wgpu::TextureView {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: self.samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        tex.create_view(&Default::default())
    }

    /// `R_PushDlights` (world), `R_DrawBrushModel`'s marking of brush submodels, then
    /// `R_RenderDynamicLightmaps` and the re-baked faces' upload.
    fn update_lightmaps(&mut self, styles: &[i32; NUM_LIGHTSTYLES], scene: &Scene) {
        let Some(world) = self.world.as_mut() else { return };
        let lights = &scene.dlights;
        world.lightmaps.begin_frame();
        world.lightmaps.mark_lights(&world.bsp, lights, 0, Vec3::ZERO);
        let mut origins = vec![Vec3::ZERO; world.bsp.models.len()];
        for b in &scene.brushes {
            if world.bsp.models.get(b.model).is_some_and(|m| m.first_face != 0) {
                world.lightmaps.mark_lights(&world.bsp, lights, b.model, b.origin);
                origins[b.model] = b.origin;
            }
        }
        world.lightmaps.update(&world.bsp, &self.gamma, styles, lights, &origins, false);
        let (queue, texture, width) = (&self.queue, &world.lightmap_texture, world.lightmaps.width);
        world.lightmaps.upload_dirty(|[x, y, w, h], data| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        });
    }

    /// One frame: the main view, then the PIP inset view (`rp.nextView`'s second pass) into
    /// its viewport, then the 2D overlay over both.
    pub fn render(&mut self, target: &wgpu::TextureView, size: (u32, u32), view: &View, scene: &Scene, inset: Option<(&View, &Scene, [i32; 4])>) {
        self.update_lightmaps(&view.light_styles, scene);
        self.update_decals();
        match inset {
            None => self.render_view(target, size, view, scene, None, Some(&scene.overlay)),
            Some((inset_view, inset_scene, rect)) => {
                self.render_view(target, size, view, scene, None, None);
                self.render_view(target, size, inset_view, inset_scene, Some(rect), Some(&scene.overlay));
            }
        }
    }

    /// `R_RenderView` of one pass: the world, opaque brush and studio entities, opaque sprites,
    /// the client's normal triangles, then translucent entities far to near
    /// (`R_DrawTEntitiesOnList`; brushes with rendermode 4 first), then the view model in the
    /// compressed depth range of `R_DrawViewModel`. An inset pass (`rect`, the refdef viewport)
    /// keeps the colour, clears the depth and draws into `R_SetupGL`'s viewport, one pixel wider
    /// on each side and taller at the top; its projection keeps the rectangle's aspect.
    #[allow(clippy::too_many_lines)]
    fn render_view(&mut self, target: &wgpu::TextureView, size: (u32, u32), view: &View, scene: &Scene, rect: Option<[i32; 4]>, overlay: Option<&[gs_draw::DrawCmd]>) {
        let (brushes, studios, sprites) = (&scene.brushes, &scene.studios, &scene.sprites);
        let depth = self.depth_view(size);
        let msaa = self.msaa_view(size);
        let proj = rect.map_or(size, |r| (r[2].max(1) as u32, r[3].max(1) as u32));
        let viewport = rect.map(|[x, y, w, h]| {
            let (gw, gh) = (size.0 as i32, size.1 as i32);
            let x0 = x - i32::from(x > 0);
            let x1 = x + w + i32::from(x + w < gw);
            let yt = gh - y + i32::from(gh - y < gh);
            let yb = gh - y - h - i32::from(gh - y - h < 0);
            let (vx, vy) = (x0.max(0), (gh - yt).max(0));
            let (vw, vh) = ((x1 - x0).min(gw - vx), (yt - yb).min(gh - vy));
            [vx as f32, vy as f32, vw.max(1) as f32, vh.max(1) as f32]
        });
        // The three GL fog states of a frame (render.md §A.7.9): underwater GL_LINEAR over the
        // opaque scene (end = 1536 − 4·waterFogDensity, colour of the last water texture), the
        // client's GL_EXP2 user fog, and none.
        let (forward, ..) = camera::angle_vectors(view.angles);
        let off = ([0.0; 4], [0.0; 4]);
        let user = view.fog.user.map_or(off, |(c, density)| ([c[0], c[1], c[2], 1.0], [2.0, density, 100.0, 2000.0]));
        let water = self.world.as_ref().and_then(|w| w.water_fog).unwrap_or(([0; 3], 0));
        let opaque = if view.fog.underwater {
            let c = water.0.map(|v| f32::from(v) / 255.0);
            ([c[0], c[1], c[2], 1.0], [1.0, 0.0, 0.0, (1536 - 4 * i32::from(water.1)) as f32])
        } else {
            user
        };
        let mut frames = Vec::with_capacity(3 * MODEL_SLOT as usize);
        for (fog_color, fog) in [opaque, user, off] {
            frames.extend(slot_bytes(&FrameUniform {
                view_proj: camera::view_projection(view, proj.0, proj.1).to_cols_array_2d(),
                eye: view.origin.extend(1.0).to_array(),
                params: [view.time as f32, view.wave_height, view.zfar, 0.0],
                forward: forward.extend(0.0).to_array(),
                fog_color,
                fog,
            }));
        }
        self.queue.write_buffer(&self.frame_buffer, 0, &frames);

        let center = |b: &BrushInstance| {
            self.world.as_ref().and_then(|w| w.bsp.models.get(b.model)).map_or(b.origin, |m| {
                b.origin + (Vec3::from(m.mins) + Vec3::from(m.maxs)) * 0.5
            })
        };
        let mut ordered: Vec<&BrushInstance> = Vec::new();
        let mut translucent: Vec<(f32, Item)> = Vec::new();
        for b in brushes.iter().take(MAX_BRUSH_INSTANCES - 1) {
            if b.rendermode != RenderMode::Normal && b.rendermode != RenderMode::TransAlpha && b.blend <= 0.0 {
                continue;
            }
            let slot = ordered.len();
            ordered.push(b);
            match b.rendermode {
                RenderMode::Normal => {}
                RenderMode::TransAlpha => translucent.push((1e9, Item::Brush(slot))),
                _ => translucent.push(((center(b) - view.origin).length_squared(), Item::Brush(slot))),
            }
        }
        let opaque_brushes = ordered.iter().enumerate().filter(|(_, b)| b.rendermode == RenderMode::Normal).map(|(i, _)| i);
        let opaque_brushes: Vec<usize> = opaque_brushes.collect();

        let (forward, right, up) = camera::angle_vectors(view.angles);
        let chrome = ChromeView { origin: view.origin.to_array(), right: right.to_array() };
        let mut vertices: Vec<StudioVertex> = Vec::new();
        let mut studio_meshes: Vec<Vec<MeshDraw>> = Vec::with_capacity(studios.len());
        let mut opaque_studios = Vec::new();
        let mut view_models = Vec::new();
        for (i, d) in studios.iter().enumerate() {
            let mut meshes = Vec::new();
            if let Some(m) = self.studio.models.get(d.model.0 as usize) {
                crate::studio::build_draw(&m.source, d, &self.gamma, &chrome, &mut vertices, &mut meshes);
            }
            studio_meshes.push(meshes);
            if d.view_model {
                view_models.push(i);
            } else if d.rendermode == RenderMode::Normal {
                opaque_studios.push(i);
            } else if d.blend > 0.0 {
                translucent.push(((Vec3::from(d.origin) - view.origin).length_squared(), Item::Studio(i)));
            }
        }
        let sprite_view = SpriteView { origin: view.origin.to_array(), forward: forward.to_array(), right: right.to_array(), up: up.to_array() };
        let mut sprite_meshes: Vec<Vec<MeshDraw>> = Vec::with_capacity(sprites.len());
        let mut opaque_sprites = Vec::new();
        for (i, d) in sprites.iter().enumerate() {
            let mut meshes = Vec::new();
            if let Some(source) = self.studio.sprites.get(d.sprite.0 as usize).and_then(|s| s.source.as_ref()) {
                crate::sprite::build_sprite(source, d, &sprite_view, &mut vertices, &mut meshes);
            }
            sprite_meshes.push(meshes);
            if d.rendermode == RenderMode::Normal {
                opaque_sprites.push(i);
            } else if d.blend > 0.0 {
                translucent.push(((Vec3::from(d.origin) - view.origin).length_squared(), Item::Sprite(i)));
            }
        }
        translucent.sort_by(|a, b| b.0.total_cmp(&a.0));
        let particle_range = append_range(&mut vertices, &scene.particles);
        let shadow_ranges: Vec<std::ops::Range<u32>> = scene.shadows.iter().map(|e| append_range(&mut vertices, &e.vertices)).collect();
        let transparent_ranges: Vec<std::ops::Range<u32>> = scene.transparent.iter().map(|e| append_range(&mut vertices, &e.vertices)).collect();
        let effect_ranges: Vec<std::ops::Range<u32>> = scene.effects.iter().map(|e| append_range(&mut vertices, &e.vertices)).collect();
        let normal_ranges: Vec<std::ops::Range<u32>> = scene.normal.iter().map(|e| append_range(&mut vertices, &e.vertices)).collect();
        let mesh_keys = studio_meshes.iter().chain(&sprite_meshes).flatten().map(|m| m.key);
        let effect_keys = scene.shadows.iter().chain(&scene.transparent).chain(&scene.effects).chain(&scene.normal).map(|e| effect_key(e.blend));
        for key in mesh_keys.chain([PARTICLE_KEY]).chain(effect_keys).collect::<Vec<_>>() {
            self.studio_pipeline(key);
        }
        self.upload_studio_vertices(&vertices);

        let world_slot = ModelUniform { transform: Mat4::IDENTITY.to_cols_array_2d(), blend: [1.0; 4], color: [1.0; 4] };
        let mut uniforms = slot_bytes(&world_slot);
        for b in &ordered {
            let c = b.color.map(|v| f32::from(v) / 256.0);
            uniforms.extend(slot_bytes(&ModelUniform {
                transform: b.transform().to_cols_array_2d(),
                blend: [b.blend; 4],
                color: [c[0], c[1], c[2], 1.0],
            }));
        }
        self.queue.write_buffer(&self.model_buffer, 0, &uniforms);
        let overlay_runs = overlay.map(|cmds| self.overlay.prepare(&self.device, &self.queue, size, cmds));

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa.as_ref().unwrap_or(target),
                    depth_slice: None,
                    resolve_target: msaa.as_ref().map(|_| target),
                    ops: wgpu::Operations {
                        load: if rect.is_some() { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let vp = viewport.unwrap_or([0.0, 0.0, size.0 as f32, size.1 as f32]);
            pass.set_viewport(vp[0], vp[1], vp[2], vp[3], 0.0, 1.0);
            if let Some(world) = &self.world {
                let set_fog = |pass: &mut wgpu::RenderPass, fog: FogSlot| pass.set_bind_group(0, &world.frame_group, &[fog as u32 * MODEL_SLOT as u32]);
                set_fog(&mut pass, FogSlot::Opaque);
                pass.set_bind_group(2, &self.model_group, &[0]);
                if scene.only_client_draw {
                    // R_RenderScene with onlyClientDraws: HUD_DrawNormalTriangles, then
                    // R_DrawTEntitiesOnList's HUD_DrawTransparentTriangles only.
                    set_fog(&mut pass, FogSlot::Off);
                    self.draw_batches(&mut pass, &scene.normal, &normal_ranges);
                    set_fog(&mut pass, FogSlot::User);
                    self.draw_batches(&mut pass, &scene.transparent, &transparent_ranges);
                } else {
                    pass.set_vertex_buffer(0, world.vertices.slice(..));
                    pass.set_index_buffer(world.indices.slice(..), wgpu::IndexFormat::Uint32);
                    for batch in &world.models[0] {
                        let pipeline = match batch.kind {
                            SurfaceKind::Lit => &self.pipelines.lit,
                            SurfaceKind::Sky => &self.pipelines.sky,
                            SurfaceKind::Water => &self.pipelines.water,
                        };
                        pass.set_pipeline(pipeline);
                        self.bind(&mut pass, &world.materials[batch.texture as usize]);
                        pass.draw_indexed(batch.indices.clone(), 0, 0..1);
                    }
                    self.draw_decals(&mut pass, 0);
                    let draw = Draw { world, ordered: &ordered, studio_meshes: &studio_meshes, studios, sprite_meshes: &sprite_meshes, sprites };
                    for &slot in &opaque_brushes {
                        self.draw_item(&mut pass, &draw, Item::Brush(slot));
                    }
                    for &i in &opaque_studios {
                        self.draw_item(&mut pass, &draw, Item::Studio(i));
                    }
                    self.draw_batches(&mut pass, &scene.shadows, &shadow_ranges);
                    for &i in &opaque_sprites {
                        self.draw_item(&mut pass, &draw, Item::Sprite(i));
                    }
                    // HUD_DrawNormalTriangles with the GL fog off.
                    set_fog(&mut pass, FogSlot::Off);
                    self.draw_batches(&mut pass, &scene.normal, &normal_ranges);
                    // R_DrawTEntitiesOnList: fog off per entity, back on (user fog only) for
                    // brushes that are neither glow nor additive.
                    for &(_, item) in &translucent {
                        let fog = match item {
                            Item::Brush(slot) if !matches!(ordered[slot].rendermode, RenderMode::Glow | RenderMode::TransAdd) => FogSlot::User,
                            _ => FogSlot::Off,
                        };
                        set_fog(&mut pass, fog);
                        self.draw_item(&mut pass, &draw, item);
                    }
                    // HUD_DrawTransparentTriangles turns the user fog on for the client's
                    // triangles; it stays on for the particles and the view model.
                    set_fog(&mut pass, FogSlot::User);
                    self.draw_batches(&mut pass, &scene.transparent, &transparent_ranges);
                    self.draw_effects(&mut pass, scene, particle_range, &effect_ranges);
                    if !view_models.is_empty() {
                        pass.set_viewport(vp[0], vp[1], vp[2], vp[3], 0.0, 0.3);
                        for &i in &view_models {
                            self.draw_item(&mut pass, &draw, Item::Studio(i));
                        }
                    }
                }
            }
            if let Some(runs) = &overlay_runs {
                pass.set_viewport(0.0, 0.0, size.0 as f32, size.1 as f32, 0.0, 1.0);
                self.overlay.draw(&mut pass, size, runs, |texture, frame| {
                    let material = self.overlay_sprites.get(texture as usize).and_then(|s| s.get(frame as usize)).and_then(Option::as_ref)?;
                    Some((&material.group, self.sampler_group(material.sampler)))
                });
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    fn upload_studio_vertices(&mut self, vertices: &[StudioVertex]) {
        if vertices.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(vertices);
        let needed = bytes.len() as u64;
        if self.studio.vertices.as_ref().is_none_or(|b| b.size() < needed) {
            self.studio.vertices = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("studio vertices"),
                size: needed.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(buffer) = &self.studio.vertices {
            self.queue.write_buffer(buffer, 0, bytes);
        }
    }

    /// `R_DrawParticles`' geometry: particles, then tracers and beams.
    fn draw_effects(&self, pass: &mut wgpu::RenderPass, scene: &Scene, particles: std::ops::Range<u32>, effects: &[std::ops::Range<u32>]) {
        let Some(buffer) = &self.studio.vertices else { return };
        pass.set_vertex_buffer(0, buffer.slice(..));
        if !particles.is_empty()
            && let (Some(pipeline), Some(material)) = (self.studio.pipelines.get(&PARTICLE_KEY), &self.particle_material)
        {
            pass.set_pipeline(pipeline);
            self.bind(pass, material);
            pass.draw(particles, 0..1);
        }
        self.draw_batches(pass, &scene.effects, effects);
    }

    /// Sprite-textured triangle batches (`tri.SpriteTexture` + `tri.Begin`).
    fn draw_batches(&self, pass: &mut wgpu::RenderPass, batches: &[EffectBatch], ranges: &[std::ops::Range<u32>]) {
        let Some(buffer) = &self.studio.vertices else { return };
        pass.set_vertex_buffer(0, buffer.slice(..));
        for (batch, range) in batches.iter().zip(ranges) {
            let material = self.studio.sprites.get(batch.sprite.0 as usize).and_then(|s| s.frames.get(batch.frame as usize)).and_then(Option::as_ref);
            let (Some(pipeline), Some(material)) = (self.studio.pipelines.get(&effect_key(batch.blend)), material) else { continue };
            pass.set_pipeline(pipeline);
            self.bind(pass, material);
            pass.draw(range.clone(), 0..1);
        }
    }

    fn draw_item(&self, pass: &mut wgpu::RenderPass, d: &Draw, item: Item) {
        match item {
            Item::Brush(slot) => {
                let b = d.ordered[slot];
                let Some(batches) = d.world.models.get(b.model) else { return };
                pass.set_vertex_buffer(0, d.world.vertices.slice(..));
                pass.set_index_buffer(d.world.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_bind_group(2, &self.model_group, &[(slot as u64 + 1) as u32 * MODEL_SLOT as u32]);
                for batch in batches.iter().filter(|batch| batch.kind != SurfaceKind::Sky) {
                    let pipeline = match (b.rendermode, batch.kind) {
                        (RenderMode::Normal | RenderMode::TransAlpha, SurfaceKind::Water) => &self.pipelines.water,
                        (_, SurfaceKind::Water) => &self.pipelines.water_blend,
                        (RenderMode::Normal, _) => &self.pipelines.lit,
                        (RenderMode::TransAlpha, _) => &self.pipelines.lit_alpha_test,
                        (RenderMode::TransColor, _) => &self.pipelines.trans_color,
                        (RenderMode::TransTexture | RenderMode::Glow, _) => &self.pipelines.trans_texture,
                        (RenderMode::TransAdd, _) => &self.pipelines.additive,
                    };
                    pass.set_pipeline(pipeline);
                    self.bind(pass, &d.world.materials[batch.texture as usize]);
                    pass.draw_indexed(batch.indices.clone(), 0, 0..1);
                }
                self.draw_decals(pass, b.model);
            }
            Item::Studio(i) => {
                let (Some(buffer), Some(model)) = (&self.studio.vertices, self.studio.models.get(d.studios[i].model.0 as usize))
                else {
                    return;
                };
                pass.set_vertex_buffer(0, buffer.slice(..));
                for mesh in &d.studio_meshes[i] {
                    let material = match mesh.sprite {
                        Some(sprite) => self.studio.sprites.get(sprite.0 as usize).and_then(|s| s.frames.get(mesh.texture as usize)).and_then(Option::as_ref),
                        None => model.materials.get(mesh.texture as usize),
                    };
                    let (Some(pipeline), Some(material)) = (self.studio.pipelines.get(&mesh.key), material) else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    self.bind(pass, material);
                    pass.draw(mesh.vertices.clone(), 0..1);
                }
            }
            Item::Sprite(i) => {
                let (Some(buffer), Some(sprite)) = (&self.studio.vertices, self.studio.sprites.get(d.sprites[i].sprite.0 as usize))
                else {
                    return;
                };
                pass.set_vertex_buffer(0, buffer.slice(..));
                for mesh in &d.sprite_meshes[i] {
                    let (Some(pipeline), Some(Some(material))) =
                        (self.studio.pipelines.get(&mesh.key), sprite.frames.get(mesh.texture as usize))
                    else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    self.bind(pass, material);
                    pass.draw(mesh.vertices.clone(), 0..1);
                }
            }
        }
    }
}

/// One entry of the draw order.
#[derive(Debug, Clone, Copy)]
enum Item {
    /// Index into the ordered brush list (uniform slot − 1).
    Brush(usize),
    /// Index into the frame's studio draws.
    Studio(usize),
    /// Index into the frame's sprite draws.
    Sprite(usize),
}

struct Draw<'a> {
    world: &'a WorldGpu,
    ordered: &'a [&'a BrushInstance],
    studio_meshes: &'a [Vec<MeshDraw>],
    studios: &'a [StudioDraw],
    sprite_meshes: &'a [Vec<MeshDraw>],
    sprites: &'a [SpriteDraw],
}

/// One `MODEL_SLOT`-aligned uniform slot.
fn slot_bytes<T: Pod>(u: &T) -> Vec<u8> {
    let mut slot = vec![0u8; MODEL_SLOT as usize];
    slot[..size_of::<T>()].copy_from_slice(bytemuck::bytes_of(u));
    slot
}

/// Appends `src` to the shared vertex list, returning its vertex range.
fn append_range(vertices: &mut Vec<StudioVertex>, src: &[StudioVertex]) -> std::ops::Range<u32> {
    let start = vertices.len() as u32;
    vertices.extend_from_slice(src);
    start..vertices.len() as u32
}

/// `r_notexture_mip`: 16x16 black/magenta checkerboard of 8x8 quadrants.
fn missing_texture() -> (u32, u32, Vec<u8>) {
    let mut px = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            let magenta = (x < 8) == (y < 8);
            px.extend_from_slice(if magenta { &[255, 0, 255, 255] } else { &[0, 0, 0, 255] });
        }
    }
    (16, 16, px)
}
