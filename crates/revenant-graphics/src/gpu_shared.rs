//! Bounded GPU assets shared by independent native surfaces.
use super::{ATLAS, FailureWake, Glyph, Instance, atlas, error};
use revenant_core::Result;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

const DEVICES: usize = 2;
const PIPELINES: usize = 4;

struct Backend {
    instance: wgpu::Instance,
    // Compiler files outlive the instance and every dependent device.
    #[cfg(windows)]
    _compiler: crate::compiler::BundledDxc,
}
struct Pool {
    devices: Vec<Arc<Assets>>,
    creating: usize,
}
struct Service {
    pool: Mutex<Pool>,
    backend: Arc<Backend>,
}

/// Explicit owner of at most two devices and four format pipelines per device.
/// Keeping this service retains warm immutable assets; `evict_idle` releases them.
#[derive(Clone)]
pub struct GraphicsService(Arc<Service>);

/// Known service allocations. Driver allocations and pipeline bytes are unknown.
#[derive(Clone, Copy, Debug, Default)]
pub struct GraphicsMemory {
    /// Live or warm device count.
    pub devices: usize,
    /// Format-specific render pipelines retained by those devices.
    pub pipelines: usize,
    /// Immutable R8 atlas texture bytes, counted once per device.
    pub atlas_bytes: u64,
}

impl GraphicsService {
    /// Prepares the pinned compiler and GPU instance, without creating a device.
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        let compiler = crate::compiler::BundledDxc::prepare()?;
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = if cfg!(windows) {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::VULKAN
        };
        #[cfg(windows)]
        {
            descriptor.backend_options.dx12.shader_compiler = compiler.compiler();
        }
        Ok(Self(Arc::new(Service {
            pool: Mutex::new(Pool {
                devices: vec![],
                creating: 0,
            }),
            backend: Arc::new(Backend {
                instance: wgpu::Instance::new(descriptor),
                #[cfg(windows)]
                _compiler: compiler,
            }),
        })))
    }

    pub(super) fn shared() -> Result<Self> {
        static SHARED: std::sync::OnceLock<Mutex<Weak<Service>>> = std::sync::OnceLock::new();
        let mut shared = SHARED
            .get_or_init(|| Mutex::new(Weak::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(service) = shared.upgrade() {
            return Ok(Self(service));
        }
        let service = Self::new()?;
        *shared = Arc::downgrade(&service.0);
        Ok(service)
    }

    /// Releases devices without dependent surfaces; active assets remain pinned.
    pub fn evict_idle(&self) {
        let retired = {
            let mut pool = self
                .0
                .pool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut retired = vec![];
            let mut i = 0;
            while i < pool.devices.len() {
                if Arc::strong_count(&pool.devices[i]) == 1 {
                    retired.push(pool.devices.remove(i));
                } else {
                    i += 1;
                }
            }
            retired
        };
        drop(retired);
    }

    /// Counts allocations owned by this service, excluding independent surface buffers.
    pub fn memory(&self) -> GraphicsMemory {
        let pool = self
            .0
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        GraphicsMemory {
            devices: pool.devices.len(),
            pipelines: pool
                .devices
                .iter()
                .map(|d| {
                    d.pipelines
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .len()
                })
                .sum(),
            atlas_bytes: pool.devices.len() as u64 * u64::from(ATLAS * ATLAS),
        }
    }

    pub(super) async fn surface<W>(
        &self,
        window: Arc<W>,
    ) -> Result<(wgpu::Surface<'static>, Arc<Assets>)>
    where
        W: raw_window_handle::HasWindowHandle
            + raw_window_handle::HasDisplayHandle
            + Send
            + Sync
            + 'static,
    {
        let surface = self
            .0
            .backend
            .instance
            .create_surface(window)
            .map_err(error)?;
        {
            let mut pool = self
                .0
                .pool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(assets) = pool
                .devices
                .iter()
                .find(|d| !d.failure.failed() && d.adapter.is_surface_supported(&surface))
            {
                return Ok((surface, assets.clone()));
            }
            if pool.devices.len() + pool.creating >= DEVICES {
                return Err(error(
                    "Shared GPU device capacity exhausted; evict idle devices before retrying",
                ));
            }
            pool.creating += 1;
        }
        // Cancellation of adapter/device creation also releases admission.
        struct Admission(Arc<Service>);
        impl Drop for Admission {
            fn drop(&mut self) {
                self.0
                    .pool
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .creating -= 1;
            }
        }
        let _admission = Admission(self.0.clone());
        let adapter = self
            .0
            .backend
            .instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                ..Default::default()
            })
            .await
            .map_err(error)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Shared Revenant GPU"),
                ..Default::default()
            })
            .await
            .map_err(error)?;
        let failure = Arc::new(Failure::default());
        let uncaptured = failure.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            uncaptured.record(e.to_string())
        }));
        let lost = failure.clone();
        device.set_device_lost_callback(move |reason, message| {
            lost.record(format!("GPU device lost ({reason:?}): {message}"))
        });
        let (glyphs, pixels) = atlas()?;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shared Atkinson atlas"),
            size: wgpu::Extent3d {
                width: ATLAS,
                height: ATLAS,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS),
                rows_per_image: Some(ATLAS),
            },
            wgpu::Extent3d {
                width: ATLAS,
                height: ATLAS,
                depth_or_array_layers: 1,
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let assets = Arc::new(Assets {
            adapter,
            device,
            queue,
            pipelines: Mutex::new(HashMap::new()),
            atlas: texture.create_view(&Default::default()),
            sampler,
            glyphs: Arc::new(glyphs),
            failure,
            _backend: self.0.backend.clone(),
        });
        let mut pool = self
            .0
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = pool
            .devices
            .iter()
            .find(|d| !d.failure.failed() && d.adapter.is_surface_supported(&surface))
        {
            return Ok((surface, existing.clone()));
        }
        pool.devices.push(assets.clone());
        Ok((surface, assets))
    }
}

#[derive(Default)]
pub(super) struct Failure {
    message: Mutex<Option<String>>,
    wakes: Mutex<Vec<Weak<super::WakeRegistration>>>,
}
impl Failure {
    fn failed(&self) -> bool {
        self.message
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
    pub(super) fn message(&self) -> Option<String> {
        self.message
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub(super) fn subscribe(&self, wake: &FailureWake) {
        let mut wakes = self
            .wakes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        wakes.retain(|w| w.strong_count() > 0);
        wakes.push(Arc::downgrade(wake));
    }
    fn record(&self, message: String) {
        *self
            .message
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message);
        let wakes = {
            let mut wakes = self
                .wakes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            wakes.retain(|w| w.strong_count() > 0);
            wakes.iter().filter_map(Weak::upgrade).collect::<Vec<_>>()
        };
        for wake in wakes {
            let callback = wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(callback) = callback {
                callback();
            }
        }
    }
}

pub(super) struct Assets {
    pub(super) adapter: wgpu::Adapter,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pipelines: Mutex<HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>>,
    pub(super) atlas: wgpu::TextureView,
    pub(super) sampler: wgpu::Sampler,
    pub(super) glyphs: Arc<HashMap<char, Glyph>>,
    pub(super) failure: Arc<Failure>,
    _backend: Arc<Backend>,
}
impl Assets {
    pub(super) fn pipeline(&self, format: wgpu::TextureFormat) -> Result<wgpu::RenderPipeline> {
        let mut pipelines = self
            .pipelines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pipeline) = pipelines.get(&format) {
            return Ok(pipeline.clone());
        }
        if pipelines.len() >= PIPELINES {
            return Err(error("Shared GPU pipeline format capacity exhausted"));
        }
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Scene2D"),
                source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
            });
        let attrs = wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4,3=>Float32x4,4=>Float32x4];
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Shared Scene2D"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attrs,
                    })],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
        pipelines.insert(format, pipeline.clone());
        Ok(pipeline)
    }
}
