//! Native wgpu renderer. The owned window target is retained by the GPU surface.
use crate::{Scene2D, SceneFrame, Shape};
use revenant_core::{Error, Result};
#[cfg(all(test, windows))]
use std::time::Duration;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use wgpu::util::DeviceExt;

#[path = "gpu_shared.rs"]
mod shared;
pub use shared::{GraphicsMemory, GraphicsService};

const LABEL_GLYPHS: usize = 20_000;
const ATLAS: u32 = 1024;
include!(concat!(env!("OUT_DIR"), "/scene-font.rs"));
type WakeRegistration = Mutex<Option<Arc<dyn Fn() + Send + Sync>>>;
type FailureWake = Arc<WakeRegistration>;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    start: [f32; 4],
    target: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    tex: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Camera {
    view: [f32; 4],
    options: [f32; 4],
}
struct Glyph {
    uv: [f32; 4],
    width: f32,
    height: f32,
    advance: f32,
    ymin: f32,
}

/// Surface renderer whose destruction must precede destruction of its window.
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    count: u32,
    frame: Arc<SceneFrame>,
    geometry: Vec<Instance>,
    glyphs: Arc<HashMap<char, Glyph>>,
    staging: Vec<Instance>,
    pending: Vec<Instance>,
    epoch: Instant,
    uploaded_bytes: u64,
    successful_presents: u64,
    view: [f32; 4],
    auto_fit: bool,
    motion: bool,
    labels: bool,
    background: [f64; 4],
    adapter: String,
    backend: wgpu::Backend,
    failure: Arc<shared::Failure>,
    failure_wake: FailureWake,
    // Assets and service retain the compiler until the last dependent surface drops.
    assets: Arc<shared::Assets>,
    _service: GraphicsService,
}

fn error(e: impl std::fmt::Display) -> Error {
    Error::new("native_gpu", e.to_string())
}

fn fit_frame(frame: &SceneFrame, width: u32, height: u32) -> Option<[f32; 4]> {
    let first = frame.primitives.first()?;
    let mut min = [first.bounds[0], first.bounds[1]];
    let mut max = [
        first.bounds[0] + first.bounds[2],
        first.bounds[1] + first.bounds[3],
    ];
    for primitive in &frame.primitives[1..] {
        min[0] = min[0].min(primitive.bounds[0]);
        min[1] = min[1].min(primitive.bounds[1]);
        max[0] = max[0].max(primitive.bounds[0] + primitive.bounds[2]);
        max[1] = max[1].max(primitive.bounds[1] + primitive.bounds[3]);
    }
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let width = (max[0] - min[0])
        .max(1.)
        .max((max[1] - min[1]).max(1.) * aspect);
    let height = width / aspect;
    Some([
        (min[0] + max[0] - width) * 0.5,
        (min[1] + max[1] - height) * 0.5,
        width,
        height,
    ])
}

impl Renderer {
    /// Initializes DX12 on Windows, using the bundled shader compiler.
    /// The surface owns `window`; its owner must keep the actual native window alive.
    pub async fn new<W>(window: Arc<W>, width: u32, height: u32) -> Result<Self>
    where
        W: raw_window_handle::HasWindowHandle
            + raw_window_handle::HasDisplayHandle
            + Send
            + Sync
            + 'static,
    {
        Self::with_service(&GraphicsService::shared()?, window, width, height).await
    }

    /// Creates an independent surface using explicitly retained, bounded warm assets.
    pub async fn with_service<W>(
        service: &GraphicsService,
        window: Arc<W>,
        width: u32,
        height: u32,
    ) -> Result<Self>
    where
        W: raw_window_handle::HasWindowHandle
            + raw_window_handle::HasDisplayHandle
            + Send
            + Sync
            + 'static,
    {
        let (surface, assets) = service.surface(window).await?;
        let adapter = &assets.adapter;
        let adapter_name = adapter.get_info().name;
        let backend = adapter.get_info().backend;
        let device = assets.device.clone();
        let queue = assets.queue.clone();
        let failure = assets.failure.clone();
        let failure_wake: FailureWake = Arc::new(Mutex::new(None));
        failure.subscribe(&failure_wake);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut config = surface
            .get_default_config(adapter, width.max(1), height.max(1))
            .ok_or_else(|| error("The native GPU cannot present to this viewport"))?;
        config.present_mode = wgpu::PresentMode::Fifo;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);
        let pipeline = assets.pipeline(config.format)?;
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera"),
            contents: bytemuck::bytes_of(&Camera {
                view: [0., 0., 1000., 600.],
                options: [1., 0., 0., 0.],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let glyphs = assets.glyphs.clone();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Scene2D camera and font"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&assets.atlas),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&assets.sampler),
                },
            ],
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bounded Scene2D instances"),
            size: ((crate::MAX_PRIMITIVES + LABEL_GLYPHS) * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if let Some(e) = validation.pop().await {
            return Err(error(e));
        }
        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            camera_buffer,
            bind,
            instances,
            count: 0,
            frame: Arc::new(SceneFrame {
                revision: 0,
                primitives: vec![],
            }),
            geometry: Vec::with_capacity(crate::MAX_PRIMITIVES),
            staging: Vec::with_capacity(crate::MAX_PRIMITIVES + LABEL_GLYPHS),
            pending: Vec::with_capacity(crate::MAX_PRIMITIVES + LABEL_GLYPHS),
            glyphs,
            epoch: Instant::now(),
            uploaded_bytes: 0,
            successful_presents: 0,
            view: [0., 0., 1000., 600.],
            auto_fit: true,
            motion: true,
            labels: true,
            background: [0.025, 0.03, 0.04, 1.],
            adapter: adapter_name,
            backend,
            failure,
            failure_wake,
            assets,
            _service: service.clone(),
        })
    }

    /// Identifies the physical adapter used by this viewport.
    pub fn adapter(&self) -> &str {
        &self.adapter
    }
    /// Identifies the backend actually selected by this viewport's adapter.
    pub fn backend(&self) -> wgpu::Backend {
        self.backend
    }
    /// Counts successful submission/present calls, saturating at `u64::MAX`.
    /// Timeout, lost, outdated and occluded acquisitions do not increment this count.
    /// A successful call does not establish compositor visibility or GPU completion.
    pub fn successful_presents(&self) -> u64 {
        self.successful_presents
    }
    /// Revision of the geometry used for native picking in the current frame.
    pub fn scene_revision(&self) -> u64 {
        self.frame.revision
    }
    /// Current camera rectangle in world coordinates: x, y, width, height.
    pub fn view_bounds(&self) -> [f32; 4] {
        self.view
    }
    /// Wakes a settled viewport when the driver reports loss or a GPU error.
    pub fn on_failure(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self
            .failure_wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(wake.clone());
        if self.failure.message().is_some() {
            wake();
        }
    }
    /// Approximate owned instance and atlas bytes, excluding driver allocations.
    pub fn buffer_bytes(&self) -> u64 {
        self.instances.size() + u64::from(ATLAS * ATLAS) + 32
    }
    /// Independent per-surface buffers; excludes the shared atlas and unknown driver bytes.
    pub fn surface_buffer_bytes(&self) -> u64 {
        self.instances.size() + self.camera_buffer.size()
    }
    /// Known retained CPU instance staging capacity, in bytes.
    pub fn staging_bytes(&self) -> usize {
        (self.geometry.capacity() + self.staging.capacity() + self.pending.capacity())
            * std::mem::size_of::<Instance>()
    }
    /// Instance bytes uploaded in the most recent render; camera writes are separate.
    pub fn last_upload_bytes(&self) -> u64 {
        self.uploaded_bytes
    }
    /// Whether two viewports retain the same device and immutable assets.
    pub fn shares_device(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.assets, &other.assets)
    }
    /// Reconfigures a nonzero physical-pixel viewport.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (self.config.width == width && self.config.height == height)
        {
            return;
        }
        if self.auto_fit {
            if let Some(view) = fit_frame(&self.frame, width, height) {
                self.view = view;
            }
        } else {
            self.view[3] = self.view[2] * height as f32 / width as f32;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }
    /// Controls labels and geometry interpolation, including reduced-motion mode.
    pub fn options(&mut self, motion: bool, labels: bool, background: [f64; 4]) {
        self.motion = motion;
        self.labels = labels;
        self.background = background;
        self.frame = Arc::new(SceneFrame {
            revision: u64::MAX,
            primitives: vec![],
        });
    }
    /// Fits the complete scene and keeps fitting updates/resizes until pan or zoom.
    pub fn fit(&mut self, scene: &Scene2D) {
        self.auto_fit = true;
        if let Some(view) = fit_frame(&scene.snapshot(), self.config.width, self.config.height) {
            self.view = view;
        }
    }
    /// Pans by physical-pixel displacement.
    pub fn pan(&mut self, delta: [f32; 2]) {
        self.auto_fit = false;
        self.view[0] -= delta[0] / self.config.width as f32 * self.view[2];
        self.view[1] -= delta[1] / self.config.height as f32 * self.view[3];
    }
    /// Zooms around a physical-pixel cursor position.
    pub fn zoom(&mut self, point: [f32; 2], factor: f32) {
        self.auto_fit = false;
        let world = self.world(point);
        let scale = factor.clamp(0.5, 2.0);
        self.view[2] = (self.view[2] * scale).clamp(0.01, 1e8);
        self.view[3] = (self.view[3] * scale).clamp(0.01, 1e8);
        self.view[0] = world[0] - point[0] / self.config.width as f32 * self.view[2];
        self.view[1] = world[1] - point[1] / self.config.height as f32 * self.view[3];
    }
    fn world(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.view[0] + p[0] / self.config.width as f32 * self.view[2],
            self.view[1] + p[1] / self.config.height as f32 * self.view[3],
        ]
    }
    fn now(&self) -> f32 {
        self.epoch.elapsed().as_secs_f32()
    }
    /// Picks the visible interpolated geometry, rather than a future frame.
    pub fn pick(&self, point: [f32; 2]) -> Option<String> {
        let point = self.world(point);
        let now = self.now();
        self.frame
            .primitives
            .iter()
            .zip(&self.geometry)
            .rev()
            .find(|(p, g)| crate::contains_bounds(p.shape, interpolated(g, now), point))
            .map(|(p, _)| p.key.clone())
    }
    /// Draws one frame, returning whether interpolation needs another frame.
    /// Occluded surfaces cease scheduling; lost surfaces are reconfigured once.
    pub fn render(&mut self, scene: &Scene2D) -> Result<bool> {
        if let Some(e) = self.failure.message() {
            return Err(error(e));
        }
        self.uploaded_bytes = 0;
        let frame = scene.snapshot();
        if !Arc::ptr_eq(&frame, &self.frame) {
            if self.auto_fit
                && let Some(view) = fit_frame(&frame, self.config.width, self.config.height)
            {
                self.view = view;
            }
            let now = self.now();
            refresh_geometry(&self.frame, &frame, &mut self.geometry, now, self.motion);
            let mut all = std::mem::take(&mut self.pending);
            all.clear();
            all.extend_from_slice(&self.geometry);
            if self.labels {
                self.append_labels(&frame, &mut all);
            }
            for range in dirty_ranges(&self.staging, &all) {
                let offset = (range.start * std::mem::size_of::<Instance>()) as u64;
                let bytes = bytemuck::cast_slice(&all[range]);
                self.queue.write_buffer(&self.instances, offset, bytes);
                self.uploaded_bytes += bytes.len() as u64;
            }
            self.count = all.len() as u32;
            self.frame = frame;
            std::mem::swap(&mut self.staging, &mut all);
            self.pending = all;
        }
        let now = self.now();
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&Camera {
                view: self.view,
                options: [now, 0., 0., 0.],
            }),
        );
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(o)
            | wgpu::CurrentSurfaceTexture::Suboptimal(o) => o,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(true);
            }
            wgpu::CurrentSurfaceTexture::Timeout => return Ok(true),
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(false),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(error("GPU surface validation failed"));
            }
        };
        let view = output.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Scene2D frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Scene2D"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.background[0],
                            g: self.background[1],
                            b: self.background[2],
                            a: self.background[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..6, 0..self.count);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(output);
        self.successful_presents = self.successful_presents.saturating_add(1);
        Ok(self
            .geometry
            .iter()
            .any(|g| g.start != g.target && now < g.params[1] + g.params[2]))
    }
    fn append_labels(&self, frame: &SceneFrame, out: &mut Vec<Instance>) {
        let start_count = out.len();
        let scale = self.view[2] / self.config.width as f32;
        for p in &frame.primitives {
            if p.bounds[2] / scale < 64. || p.bounds[3] / scale < 24. {
                continue;
            }
            let mut x = p.bounds[0] + 7. * scale;
            let y = p.bounds[1] + 7. * scale;
            for ch in p.label.chars().take(80) {
                if out.len() - start_count >= LABEL_GLYPHS {
                    return;
                }
                let Some(g) = self.glyphs.get(&ch).or_else(|| self.glyphs.get(&'?')) else {
                    continue;
                };
                if x + (g.advance + 5.) * scale > p.bounds[0] + p.bounds[2] {
                    break;
                }
                let b = [
                    x,
                    y + (17. - g.height - g.ymin) * scale,
                    g.width * scale,
                    g.height * scale,
                ];
                out.push(Instance {
                    start: b,
                    target: b,
                    color: [0.94, 0.97, 1., 1.],
                    params: [2., 0., 0., 0.],
                    tex: g.uv,
                });
                x += g.advance * scale;
            }
        }
    }
}

fn interpolated(g: &Instance, now: f32) -> [f32; 4] {
    let t = if g.params[2] > 0. {
        ((now - g.params[1]) / g.params[2]).clamp(0., 1.)
    } else {
        1.
    };
    let t = t * t * (3. - 2. * t);
    std::array::from_fn(|i| g.start[i] + (g.target[i] - g.start[i]) * t)
}

fn refresh_geometry(
    old: &SceneFrame,
    new: &SceneFrame,
    geometry: &mut Vec<Instance>,
    now: f32,
    motion: bool,
) {
    let instance = |p: &crate::Primitive, previous: Option<Instance>| {
        let mut g = previous.unwrap_or(Instance {
            start: p.bounds,
            target: p.bounds,
            color: p.color,
            params: [0.; 4],
            tex: [0.; 4],
        });
        if g.target != p.bounds {
            g.start = interpolated(&g, now);
            g.target = p.bounds;
            g.params[1] = now;
            g.params[2] = if motion { 0.22 } else { 0. };
        }
        if !motion {
            g.start = p.bounds;
            g.target = p.bounds;
            g.params[2] = 0.;
        }
        g.color = p.color;
        g.params[0] = if p.shape == Shape::Circle { 1. } else { 0. };
        g
    };
    let stable = old.primitives.len() == new.primitives.len()
        && geometry.len() == new.primitives.len()
        && old
            .primitives
            .iter()
            .zip(&new.primitives)
            .all(|(a, b)| a.key == b.key);
    if stable {
        for (g, p) in geometry.iter_mut().zip(&new.primitives) {
            *g = instance(p, Some(*g));
        }
    } else {
        let previous: HashMap<&str, Instance> = old
            .primitives
            .iter()
            .zip(geometry.iter())
            .map(|(p, g)| (p.key.as_str(), *g))
            .collect();
        geometry.clear();
        geometry.extend(
            new.primitives
                .iter()
                .map(|p| instance(p, previous.get(p.key.as_str()).copied())),
        );
    }
}

fn dirty_ranges<'a>(
    old: &'a [Instance],
    new: &'a [Instance],
) -> impl Iterator<Item = std::ops::Range<usize>> + 'a {
    let mut index = 0;
    std::iter::from_fn(move || {
        while index < new.len() && old.get(index) == Some(&new[index]) {
            index += 1;
        }
        if index == new.len() {
            return None;
        }
        let start = index;
        index += 1;
        while index < new.len() && old.get(index) != Some(&new[index]) {
            index += 1;
        }
        Some(start..index)
    })
}

fn atlas() -> Result<(HashMap<char, Glyph>, Vec<u8>)> {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/scene-glyphs.bin"));
    let mut glyphs = HashMap::with_capacity(352);
    for row in bytes.as_chunks::<36>().0 {
        let read =
            |i| f32::from_le_bytes(row[i..i + 4].try_into().expect("build-time glyph field"));
        let ch = char::from_u32(u32::from_le_bytes(
            row[..4].try_into().expect("build-time glyph identity"),
        ))
        .ok_or_else(|| error("Invalid packaged glyph identity"))?;
        glyphs.insert(
            ch,
            Glyph {
                uv: [read(4), read(8), read(12), read(16)],
                width: read(20),
                height: read(24),
                advance: read(28),
                ymin: read(32),
            },
        );
    }
    Ok((
        glyphs,
        include_bytes!(concat!(env!("OUT_DIR"), "/scene-atlas.bin")).to_vec(),
    ))
}

#[cfg(test)]
mod framing_tests {
    use super::*;
    fn frame(bounds: &[[f32; 4]]) -> SceneFrame {
        SceneFrame {
            revision: 1,
            primitives: bounds
                .iter()
                .enumerate()
                .map(|(i, bounds)| crate::Primitive {
                    key: i.to_string(),
                    shape: Shape::Rectangle,
                    bounds: *bounds,
                    color: [1.; 4],
                    label: String::new(),
                })
                .collect(),
        }
    }
    fn visible(view: [f32; 4], bounds: [f32; 4]) {
        assert!(view[0] <= bounds[0] + 0.001 && view[1] <= bounds[1] + 0.001);
        assert!(view[0] + view[2] >= bounds[0] + bounds[2] - 0.001);
        assert!(view[1] + view[3] >= bounds[1] + bounds[3] - 0.001);
    }
    #[test]
    fn short_and_tall_viewports_contain_all_extents_including_negative_origins() {
        let bounds = [[-250., -120., 350., 80.], [300., 400., 700., 200.]];
        let frame = frame(&bounds);
        for (width, height) in [(784, 279), (279, 784), (1, 1)] {
            let view = fit_frame(&frame, width, height).unwrap();
            for bounds in bounds {
                visible(view, bounds);
            }
            assert!((view[2] / view[3] - width as f32 / height as f32).abs() < 0.001);
        }
    }
    #[test]
    fn empty_and_zero_area_scenes_have_defined_framing() {
        assert_eq!(fit_frame(&frame(&[]), 784, 279), None);
        let view = fit_frame(&frame(&[[-200., -150., 0., 0.]]), 784, 279).unwrap();
        assert!(view.iter().all(|value| value.is_finite()));
        assert!(view[2] > 0. && view[3] > 0.);
        visible(view, [-200., -150., 0., 0.]);
    }

    #[test]
    fn settled_one_percent_patch_uploads_one_percent_without_staging_growth() {
        let old = frame(
            &(0..crate::MAX_PRIMITIVES)
                .map(|i| [i as f32, 0., 1., 1.])
                .collect::<Vec<_>>(),
        );
        let mut geometry = Vec::with_capacity(crate::MAX_PRIMITIVES);
        refresh_geometry(&frame(&[]), &old, &mut geometry, 0., false);
        let uploaded = geometry.clone();
        let capacity = geometry.capacity();
        let mut new = old.clone();
        new.revision += 1;
        for i in (0..crate::MAX_PRIMITIVES).step_by(100) {
            new.primitives[i].color[0] = 0.5;
        }
        refresh_geometry(&old, &new, &mut geometry, 1., false);
        let bytes: usize = dirty_ranges(&uploaded, &geometry)
            .map(|r| r.len() * std::mem::size_of::<Instance>())
            .sum();
        assert_eq!(bytes, 500 * std::mem::size_of::<Instance>());
        assert!(bytes <= uploaded.len() * std::mem::size_of::<Instance>() / 10);
        assert_eq!(capacity, geometry.capacity());
    }

    #[test]
    fn motion_patch_starts_at_visible_position_and_leaves_other_clocks_unchanged() {
        let initial = frame(&[[0., 0., 10., 10.], [20., 0., 10., 10.]]);
        let mut geometry = Vec::new();
        refresh_geometry(&frame(&[]), &initial, &mut geometry, 0., true);
        let target = frame(&[[10., 0., 10., 10.], [30., 0., 10., 10.]]);
        refresh_geometry(&initial, &target, &mut geometry, 1., true);
        let untouched = geometry[1];
        let visible = interpolated(&geometry[0], 1.11);
        let patch = frame(&[[100., 0., 10., 10.], [30., 0., 10., 10.]]);
        refresh_geometry(&target, &patch, &mut geometry, 1.11, true);
        assert!(bytemuck::bytes_of(&untouched) == bytemuck::bytes_of(&geometry[1]));
        assert_eq!(geometry[0].start, visible);
        assert_eq!(interpolated(&geometry[0], 1.11), visible);
        assert!(interpolated(&geometry[1], 1.22)[0] > 29.99);
    }

    #[test]
    fn packaged_atlas_has_declared_font_identity_and_coverage() {
        use sha2::{Digest, Sha256};
        let (glyphs, pixels) = atlas().unwrap();
        assert_eq!(glyphs.len(), 352);
        assert_eq!(pixels.len(), 1024 * 1024);
        assert!((32..384).all(|ch| glyphs.contains_key(&char::from_u32(ch).unwrap())));
        assert_eq!(
            FONT_SHA256,
            format!(
                "{:x}",
                Sha256::digest(include_bytes!("../assets/AtkinsonHyperlegible-Regular.ttf"))
            )
        );
        assert!(glyphs.contains_key(&'?'));
    }
}

#[cfg(all(test, windows))]
mod device_failure_tests {
    use super::*;
    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
        RawWindowHandle, Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
    };
    use std::{
        num::NonZeroIsize,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use windows_sys::Win32::{
        Foundation::HWND,
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP},
    };

    struct OwnedWindow {
        handle: NonZeroIsize,
        thread: u32,
    }
    // The GPU only borrows the handle. Window creation and final disposal stay
    // on this test's thread, with the surface destroyed first.
    unsafe impl Send for OwnedWindow {}
    unsafe impl Sync for OwnedWindow {}
    impl HasWindowHandle for OwnedWindow {
        fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
            Ok(unsafe {
                WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(
                    self.handle,
                )))
            })
        }
    }
    impl HasDisplayHandle for OwnedWindow {
        fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
            Ok(unsafe {
                DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new()))
            })
        }
    }
    impl OwnedWindow {
        fn new() -> Arc<Self> {
            let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
            let title: Vec<u16> = "Revenant owned GPU error fixture"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let handle = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    64,
                    64,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                )
            };
            Arc::new(Self {
                handle: NonZeroIsize::new(handle as isize).expect("owned test window creation"),
                thread: unsafe { GetCurrentThreadId() },
            })
        }
    }
    impl Drop for OwnedWindow {
        fn drop(&mut self) {
            assert_eq!(self.thread, unsafe { GetCurrentThreadId() });
            assert_ne!(unsafe { DestroyWindow(self.handle.get() as HWND) }, 0);
        }
    }
    fn settled() -> (Renderer, Arc<OwnedWindow>, Scene2D, Arc<AtomicUsize>) {
        let window = OwnedWindow::new();
        let mut renderer = pollster::block_on(Renderer::new(window.clone(), 64, 64)).unwrap();
        let wake = Arc::new(AtomicUsize::new(0));
        let observed = wake.clone();
        renderer.on_failure(Arc::new(move || {
            observed.fetch_add(1, Ordering::AcqRel);
        }));
        renderer.options(false, false, [0., 0., 0., 1.]);
        let scene = Scene2D::new();
        assert_eq!(renderer.successful_presents(), 0);
        assert!(
            !renderer.render(&scene).unwrap(),
            "renderer must be settled before injection"
        );
        assert_eq!(renderer.successful_presents(), 1);
        assert_eq!(wake.load(Ordering::Acquire), 0);
        (renderer, window, scene, wake)
    }
    fn wait_for_wake(renderer: &Renderer, wake: &AtomicUsize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while wake.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            let _ = renderer.device.poll(wgpu::PollType::Poll);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            wake.load(Ordering::Acquire) > 0,
            "GPU failure did not wake the settled viewport within two seconds"
        );
    }
    #[test]
    #[ignore = "Requires a Windows DX12 adapter; injects an error only into this test's owned device"]
    fn settled_renderer_wakes_and_reports_uncaptured_validation_error() {
        let (mut renderer, window, scene, wake) = settled();
        let invalid = renderer
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("intentional-validation-error"),
                source: wgpu::ShaderSource::Wgsl("this is invalid WGSL".into()),
            });
        wait_for_wake(&renderer, &wake);
        let failure = renderer
            .render(&scene)
            .expect_err("uncaptured error must reach the renderer caller");
        assert_eq!(failure.code, "native_gpu");
        assert!(
            failure.message.contains("intentional-validation-error"),
            "unexpected failure: {}",
            failure.message
        );
        drop(invalid);
        drop(renderer);
        drop(window);
    }
    #[test]
    #[ignore = "Requires a Windows DX12 adapter; explicit device destruction is not physical driver loss"]
    fn explicitly_destroyed_owned_device_reports_failure_without_a_blocking_wait() {
        let (mut renderer, window, scene, wake) = settled();
        renderer.device.destroy();
        wait_for_wake(&renderer, &wake);
        let failure = renderer
            .render(&scene)
            .expect_err("destroyed device must not render successfully");
        assert_eq!(failure.code, "native_gpu");
        assert!(
            failure.message.contains("Destroyed"),
            "unexpected failure: {}",
            failure.message
        );
        drop(renderer);
        drop(window);
    }

    #[test]
    #[ignore = "Requires a Windows DX12 adapter; destroys only this fixture's shared device"]
    fn shared_device_failure_wakes_every_surface_and_idle_eviction_releases_assets() {
        let service = GraphicsService::new().unwrap();
        let a = OwnedWindow::new();
        let b = OwnedWindow::new();
        let first =
            pollster::block_on(Renderer::with_service(&service, a.clone(), 64, 64)).unwrap();
        let second =
            pollster::block_on(Renderer::with_service(&service, b.clone(), 64, 64)).unwrap();
        assert!(first.shares_device(&second));
        assert_eq!(service.memory().devices, 1);
        assert_eq!(service.memory().pipelines, 1);
        assert_eq!(service.memory().atlas_bytes, 1024 * 1024);
        let wa = Arc::new(AtomicUsize::new(0));
        let wb = Arc::new(AtomicUsize::new(0));
        let ca = wa.clone();
        first.on_failure(Arc::new(move || {
            ca.fetch_add(1, Ordering::AcqRel);
        }));
        let cb = wb.clone();
        second.on_failure(Arc::new(move || {
            cb.fetch_add(1, Ordering::AcqRel);
        }));
        first.device.destroy();
        wait_for_wake(&first, &wa);
        wait_for_wake(&second, &wb);
        assert!(first.failure.message().is_some());
        assert!(second.failure.message().is_some());
        drop(first);
        drop(second);
        drop(a);
        drop(b);
        service.evict_idle();
        assert_eq!(service.memory().devices, 0);
        assert_eq!(service.memory().atlas_bytes, 0);
    }
}
