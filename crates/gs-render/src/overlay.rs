//! The 2D overlay pass: [`gs_draw::DrawCmd`] quads (HUD sprites, fills, VGUI2 glyphs) drawn in
//! submission order over the 3D view with GoldSrc's 2D blend modes.

use std::collections::HashMap;
use std::ops::Range;

use gs_draw::{Blend, DrawCmd, GlyphAtlas, Texture};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

/// Which fragment program a quad uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Program {
    Flat,
    Textured,
    Holes,
}

/// Consecutive quads sharing pipeline, texture and scissor.
pub(crate) struct Run {
    key: (Blend, Program),
    texture: Texture,
    scissor: Option<[u32; 4]>,
    vertices: Range<u32>,
}

pub(crate) struct OverlayPass {
    pipelines: HashMap<(Blend, Program), wgpu::RenderPipeline>,
    screen_buffer: wgpu::Buffer,
    screen_group: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    glyph_texture: Option<wgpu::Texture>,
    glyph_group: Option<wgpu::BindGroup>,
    /// The glyph atlas's sampler, bound at the renderer's sampler group slot.
    glyph_sampler: wgpu::BindGroup,
    vertices: Option<wgpu::Buffer>,
}

fn blend_state(blend: Blend) -> Option<wgpu::BlendState> {
    let add = |src| wgpu::BlendState {
        color: wgpu::BlendComponent { src_factor: src, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        alpha: wgpu::BlendComponent::OVER,
    };
    match blend {
        Blend::Opaque | Blend::OpaqueTested => None,
        Blend::Holes | Blend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
        Blend::Additive => Some(add(wgpu::BlendFactor::One)),
        Blend::AlphaAdditive => Some(add(wgpu::BlendFactor::SrcAlpha)),
        Blend::Modulate => Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        }),
    }
}

impl OverlayPass {
    pub(crate) fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        material_layout: &wgpu::BindGroupLayout,
        sampler_layout: &wgpu::BindGroupLayout,
        samples: u32,
    ) -> Self {
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay screen"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let screen_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay screen"),
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: screen_buffer.as_entire_binding() }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/overlay.wgsl").into()),
        });
        let flat_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay flat"),
            bind_group_layouts: &[Some(&screen_layout)],
            immediate_size: 0,
        });
        let textured_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay textured"),
            bind_group_layouts: &[Some(&screen_layout), Some(material_layout), None, Some(sampler_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        let mut pipelines = HashMap::new();
        for blend in [Blend::Opaque, Blend::Holes, Blend::Additive, Blend::AlphaAdditive, Blend::Alpha, Blend::Modulate, Blend::OpaqueTested] {
            for program in [Program::Flat, Program::Textured, Program::Holes] {
                let (layout, entry) = match program {
                    Program::Flat => (&flat_layout, "fs_flat"),
                    Program::Textured => (&textured_layout, "fs_textured"),
                    Program::Holes => (&textured_layout, "fs_holes"),
                };
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("overlay"),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        })],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: depth_format,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: color_format,
                            blend: blend_state(blend),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                pipelines.insert((blend, program), pipeline);
            }
        }
        let glyph_sampler = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyphs"),
            layout: sampler_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&device.create_sampler(&wgpu::SamplerDescriptor { label: Some("glyphs"), ..Default::default() })),
            }],
        });
        Self {
            pipelines,
            screen_buffer,
            screen_group,
            material_layout: material_layout.clone(),
            glyph_texture: None,
            glyph_group: None,
            glyph_sampler,
            vertices: None,
        }
    }

    /// Uploads the atlas region changed since the last call.
    pub(crate) fn update_glyphs(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: &mut GlyphAtlas) {
        let size = atlas.size;
        if self.glyph_texture.as_ref().is_none_or(|t| t.width() != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("glyph atlas"),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.glyph_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("glyph atlas"),
                layout: &self.material_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())) }],
            }));
            self.glyph_texture = Some(texture);
            atlas.take_dirty();
            self.write_glyphs(queue, atlas, [0, 0, size, size]);
            return;
        }
        if let Some(rect) = atlas.take_dirty() {
            self.write_glyphs(queue, atlas, rect);
        }
    }

    fn write_glyphs(&self, queue: &wgpu::Queue, atlas: &GlyphAtlas, [x, y, w, h]: [u32; 4]) {
        let Some(texture) = &self.glyph_texture else { return };
        if w == 0 || h == 0 {
            return;
        }
        let offset = ((y * atlas.size + x) * 4) as usize;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            &atlas.pixels[offset..],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(atlas.size * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Builds and uploads this frame's quads; returns the draw runs.
    pub(crate) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), cmds: &[DrawCmd]) -> Vec<Run> {
        queue.write_buffer(&self.screen_buffer, 0, bytemuck::cast_slice(&[size.0 as f32, size.1 as f32, 0.0, 0.0]));
        let mut vertices: Vec<Vertex> = Vec::with_capacity(cmds.len() * 6);
        let mut runs: Vec<Run> = Vec::new();
        for c in cmds {
            let program = match (c.texture, c.blend) {
                (Texture::None, _) => Program::Flat,
                (_, Blend::Holes | Blend::OpaqueTested) => Program::Holes,
                _ => Program::Textured,
            };
            let [x0, y0, x1, y1] = c.rect;
            let [u0, v0, u1, v1] = c.uv;
            let v = |x, y, u, w| Vertex { pos: [x, y], uv: [u, w], color: c.color };
            let start = vertices.len() as u32;
            vertices.extend_from_slice(&[v(x0, y0, u0, v0), v(x1, y0, u1, v0), v(x1, y1, u1, v1), v(x0, y0, u0, v0), v(x1, y1, u1, v1), v(x0, y1, u0, v1)]);
            let key = (c.blend, program);
            match runs.last_mut() {
                Some(r) if r.key == key && r.texture == c.texture && r.scissor == c.scissor => r.vertices.end = start + 6,
                _ => runs.push(Run { key, texture: c.texture, scissor: c.scissor, vertices: start..start + 6 }),
            }
        }
        if vertices.is_empty() {
            return runs;
        }
        let bytes: &[u8] = bytemuck::cast_slice(&vertices);
        if self.vertices.as_ref().is_none_or(|b| b.size() < bytes.len() as u64) {
            self.vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay vertices"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(buffer) = &self.vertices {
            queue.write_buffer(buffer, 0, bytes);
        }
        runs
    }

    /// Draws the runs; `sprite_frame` resolves a sprite texture's frame bind group and the
    /// group of its sampler.
    pub(crate) fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass,
        size: (u32, u32),
        runs: &[Run],
        sprite_frame: impl Fn(u32, u32) -> Option<(&'a wgpu::BindGroup, &'a wgpu::BindGroup)>,
    ) {
        let Some(buffer) = &self.vertices else { return };
        pass.set_viewport(0.0, 0.0, size.0 as f32, size.1 as f32, 0.0, 1.0);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.set_bind_group(0, &self.screen_group, &[]);
        for run in runs {
            let Some(pipeline) = self.pipelines.get(&run.key) else { continue };
            let material = match run.texture {
                Texture::None => None,
                Texture::Glyphs => match &self.glyph_group {
                    Some(g) => Some((g, &self.glyph_sampler)),
                    None => continue,
                },
                Texture::Sprite { texture, frame } => match sprite_frame(texture, frame) {
                    Some(g) => Some(g),
                    None => continue,
                },
            };
            match run.scissor {
                Some([x, y, w, h]) if w > 0 && h > 0 => pass.set_scissor_rect(x, y, w.min(size.0 - x), h.min(size.1 - y)),
                Some(_) => continue,
                None => pass.set_scissor_rect(0, 0, size.0, size.1),
            }
            pass.set_pipeline(pipeline);
            if let Some((texture, sampler)) = material {
                pass.set_bind_group(1, texture, &[]);
                pass.set_bind_group(3, sampler, &[]);
            }
            pass.draw(run.vertices.clone(), 0..1);
        }
        pass.set_scissor_rect(0, 0, size.0, size.1);
    }
}
