//! `GL_EndRendering`'s FBO presentation (docs/re/engine-video.md §5.3.4): the window is cleared
//! to black (alpha 0) and the mode-sized frame is drawn into the letterbox rectangle with
//! linear filtering.

/// The letterbox rectangle `[x0, y0, x1, y1]` (window pixels, y down) and the scale factors
/// `(sx, sy)` the cursor mapping reads, for a `mode`-sized frame in a `window`-sized window;
/// `keep_aspect` is `!-stretchaspect`.
pub fn letterbox(mode: (i32, i32), window: (i32, i32), keep_aspect: bool) -> ([i32; 4], (f32, f32)) {
    let (ww, wh) = window;
    let mut rect = [0, 0, ww, wh];
    let mut scale = (1.0f32, 1.0f32);
    let (f_w, f_h) = (ww as f32, wh as f32);
    let a_m = mode.0 as f32 / mode.1 as f32;
    let a_w = f_w / f_h;
    if keep_aspect {
        if a_w > a_m {
            scale.0 = a_w / a_m;
            let pad = f64::from(f_w - f_h * a_m) * 0.5;
            rect[0] = (pad + 0.0) as i32;
            rect[2] = (f64::from(ww) - pad) as i32;
        } else if a_m > a_w {
            scale.1 = a_m / a_w;
            let t = ((1.0 / f64::from(a_m)) * f64::from(ww)) as f32;
            let pad = f64::from(f_h - t) * 0.5;
            rect[1] = (pad + 0.0) as i32;
            rect[3] = (f64::from(wh) - pad) as i32;
        }
    }
    (rect, scale)
}

pub struct Present {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Present {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("present.wgsl"), source: wgpu::ShaderSource::Wgsl(include_str!("shaders/present.wgsl").into()) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("present"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // GL_LINEAR, GL_CLAMP_TO_EDGE.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("present"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, layout, sampler }
    }

    /// Clears `target` to (0, 0, 0, 0) and draws `frame` into `rect` (`[x0, y0, x1, y1]`).
    pub fn draw(&self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &wgpu::TextureView, target: &wgpu::TextureView, rect: [i32; 4]) {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(frame) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let [x0, y0, x1, y1] = rect;
            if x1 > x0 && y1 > y0 {
                pass.set_viewport(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32, 0.0, 1.0);
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// engine-video.md §5.3.4's worked examples.
    #[test]
    fn letterbox_matches_the_worked_examples() {
        let (r, s) = letterbox((1024, 768), (1920, 1080), true);
        assert_eq!(r, [240, 0, 1680, 1080]);
        assert!((s.0 - 1.333_333_3).abs() < 1e-6 && s.1 == 1.0);
        assert_eq!(letterbox((1280, 1024), (1920, 1080), true).0, [285, 0, 1635, 1080]);
        let (r, s) = letterbox((1280, 720), (1920, 1200), true);
        assert_eq!(r, [0, 60, 1920, 1140]);
        assert!(s.0 == 1.0 && (s.1 - 1.111_111).abs() < 1e-6);
        assert_eq!(letterbox((1920, 1080), (1920, 1080), true), ([0, 0, 1920, 1080], (1.0, 1.0)));
        assert_eq!(letterbox((1024, 768), (1920, 1080), false), ([0, 0, 1920, 1080], (1.0, 1.0)));
    }
}
