//! Device acquisition, shared by windowed and headless rendering.

use crate::RenderError;

/// The backends the game offers, in the order it prefers them: OpenGL as the original, then
/// Direct3D 12 and Vulkan.
pub const BACKENDS: [wgpu::Backend; 3] = [wgpu::Backend::Gl, wgpu::Backend::Dx12, wgpu::Backend::Vulkan];

/// A backend's settings name (`_setrenderer`, the `RenderBackend` registry value).
pub fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Gl => "gl",
        wgpu::Backend::Metal => "metal",
        _ => "",
    }
}

/// A backend by its settings name; `d3d` is the original's name for Direct3D.
pub fn backend_from_name(name: &str) -> Option<wgpu::Backend> {
    [("dx12", wgpu::Backend::Dx12), ("d3d", wgpu::Backend::Dx12), ("vulkan", wgpu::Backend::Vulkan), ("gl", wgpu::Backend::Gl), ("metal", wgpu::Backend::Metal)]
        .into_iter()
        .find(|(n, _)| name.eq_ignore_ascii_case(n))
        .map(|(_, b)| b)
}

/// The name the video options show for a backend.
pub fn backend_label(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Dx12 => "Direct3D 12",
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Gl => "OpenGL",
        wgpu::Backend::Metal => "Metal",
        _ => "",
    }
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    pub fn new_instance() -> wgpu::Instance {
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env())
    }

    /// Picks a high-performance adapter able to present to `surface` (if any): one of
    /// `preferred` when there is one, else on Windows the first of [`BACKENDS`] that has one,
    /// else any. When `WGPU_BACKEND` is set, it alone decides.
    pub async fn new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, preferred: Option<wgpu::Backend>) -> Result<Self, RenderError> {
        let mut chosen = None;
        if std::env::var_os("WGPU_BACKEND").is_none() {
            // OpenGL first, as the original renders; Direct3D 12 windowed goes through the desktop
            // compositor, a refresh later.
            let defaults = if cfg!(windows) { BACKENDS.as_slice() } else { &[] };
            for &backend in preferred.iter().chain(defaults) {
                chosen = Self::backend_adapter(&instance, surface, backend).await;
                if chosen.is_some() {
                    break;
                }
            }
        }
        let adapter = match chosen {
            Some(adapter) => adapter,
            None => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: surface,
                    apply_limit_buckets: false,
                })
                .await
                .map_err(|e| RenderError::Init(format!("no suitable GPU adapter: {e}")))?,
        };
        let info = adapter.get_info();
        log::info!("GPU: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("gs-render"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                // Small allocator blocks (8–64 MiB): the default reserves 128–256 MiB per heap,
                // three times the video memory the game uses.
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::Init(format!("device request failed: {e}")))?;
        Ok(Self { instance, adapter, device, queue })
    }

    /// The [`BACKENDS`] with a hardware adapter able to present to `surface`; only the running
    /// one when `WGPU_BACKEND` decides.
    pub async fn available_backends(&self, surface: &wgpu::Surface<'_>) -> Vec<wgpu::Backend> {
        if std::env::var_os("WGPU_BACKEND").is_some() {
            return vec![self.adapter.get_info().backend];
        }
        let mut out = Vec::new();
        for backend in BACKENDS {
            if Self::backend_adapter(&self.instance, Some(surface), backend).await.is_some() {
                out.push(backend);
            }
        }
        out
    }

    /// The best adapter of `backend` able to present to `surface`: a discrete GPU first; never a
    /// software one (WARP, llvmpipe).
    async fn backend_adapter(instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, backend: wgpu::Backend) -> Option<wgpu::Adapter> {
        instance
            .enumerate_adapters(backend.into())
            .await
            .into_iter()
            .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
            .filter_map(|a| {
                let rank = match a.get_info().device_type {
                    wgpu::DeviceType::DiscreteGpu => 0,
                    wgpu::DeviceType::IntegratedGpu => 1,
                    wgpu::DeviceType::VirtualGpu | wgpu::DeviceType::Other => 2,
                    wgpu::DeviceType::Cpu => return None,
                };
                Some((rank, a))
            })
            .min_by_key(|&(rank, _)| rank)
            .map(|(_, a)| a)
    }
}
