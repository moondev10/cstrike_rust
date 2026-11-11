//! Offscreen rendering with CPU readback, for screenshots and automated visual checks.

use gs_assets::RgbaImage;

use crate::{RenderError, Renderer, Scene, View};

/// Renders one frame (with its PIP inset view, if any) into an offscreen target and returns it
/// as RGBA8. Alpha is forced opaque: the framebuffer's alpha channel is not part of the picture
/// (blending leaves partial coverage values there).
pub fn capture(renderer: &mut Renderer, (width, height): (u32, u32), view: &View, scene: &Scene, inset: Option<(&View, &Scene, [i32; 4])>) -> Result<RgbaImage, RenderError> {
    let format = renderer.color_format();
    let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("capture"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    renderer.render(&target.create_view(&Default::default()), (width, height), view, scene, inset);
    let mut image = read_texture(renderer.device(), renderer.queue(), &target)?;
    for px in image.pixels.chunks_exact_mut(4) {
        px[3] = 255;
    }
    Ok(image)
}

/// The window's picture of a fullscreen FBO frame (`VID_TakeSnapshot`/`SCR_ScreenShot_f` read
/// the presented back buffer): the frame rendered at `vid`, presented into a `window`-sized
/// target at the letterbox `rect` (black bars), read back opaque.
#[allow(clippy::too_many_arguments)]
pub fn capture_presented(
    renderer: &mut Renderer,
    present: &crate::present::Present,
    vid: (u32, u32),
    window: (u32, u32),
    rect: [i32; 4],
    view: &View,
    scene: &Scene,
    inset: Option<(&View, &Scene, [i32; 4])>,
) -> Result<RgbaImage, RenderError> {
    let format = renderer.color_format();
    let texture = |label, (width, height): (u32, u32), usage| {
        renderer.device().create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let fbo = texture("capture fbo", vid, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let target = texture("capture window", window, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let fbo_view = fbo.create_view(&Default::default());
    renderer.render(&fbo_view, vid, view, scene, inset);
    present.draw(renderer.device(), renderer.queue(), &fbo_view, &target.create_view(&Default::default()), rect);
    let mut image = read_texture(renderer.device(), renderer.queue(), &target)?;
    for px in image.pixels.chunks_exact_mut(4) {
        px[3] = 255;
    }
    Ok(image)
}

/// Renders every view into one offscreen target and waits for the GPU (`timerefresh`'s loop
/// between two `glFinish`es).
pub fn render_views(renderer: &mut Renderer, (width, height): (u32, u32), views: &[View], scene: &Scene) -> Result<(), RenderError> {
    let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("timerefresh"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: renderer.color_format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target = target.create_view(&Default::default());
    for view in views {
        renderer.render(&target, (width, height), view, scene, None);
    }
    renderer
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .map(|_| ())
        .map_err(|e| RenderError::Capture(format!("device poll: {e}")))
}

/// Copies a 4-byte-per-texel color texture (`COPY_SRC`) back to the CPU as RGBA8.
pub fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Result<RgbaImage, RenderError> {
    let (width, height) = (texture.width(), texture.height());
    let bgra = match texture.format() {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
        other => return Err(RenderError::Capture(format!("unsupported format {other:?}"))),
    };
    let row = width * 4;
    let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("capture") });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(height) },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| RenderError::Capture(format!("device poll: {e}")))?;
    rx.recv()
        .map_err(|e| RenderError::Capture(format!("map callback dropped: {e}")))?
        .map_err(|e| RenderError::Capture(format!("buffer map: {e}")))?;

    let mapped = slice.get_mapped_range().map_err(|e| RenderError::Capture(format!("mapped range: {e}")))?;
    let mut pixels = Vec::with_capacity((row * height) as usize);
    for y in 0..height {
        let start = (y * padded) as usize;
        pixels.extend_from_slice(&mapped[start..start + row as usize]);
    }
    drop(mapped);
    buffer.unmap();
    if bgra {
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    Ok(RgbaImage { width, height, pixels })
}
