//! Opt-in real DX12 surface measurement; never launches windows in ordinary tests.
#![cfg(all(windows, feature = "gpu"))]

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
use revenant_graphics::{
    Primitive, Scene2D, SceneChange, Shape,
    gpu::{GraphicsService, Renderer},
};
use serde_json::{Value, json};
use std::{
    num::NonZeroIsize,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct AllocationObserver;
static OBSERVE_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATION_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOCATION_BYTES: AtomicU64 = AtomicU64::new(0);
#[global_allocator]
static ALLOCATOR: AllocationObserver = AllocationObserver;
unsafe impl std::alloc::GlobalAlloc for AllocationObserver {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if OBSERVE_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATION_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::GlobalAlloc::dealloc(&std::alloc::System, pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        if OBSERVE_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATION_BYTES.fetch_add(size as u64, Ordering::Relaxed);
        }
        unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, pointer, layout, size) }
    }
}
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Dwm::DwmFlush,
    System::{
        LibraryLoader::GetModuleHandleW,
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::{
            GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetCurrentProcessId,
            GetCurrentThreadId, GetGuiResources, GetProcessHandleCount,
        },
    },
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
        WindowsAndMessaging::*,
    },
};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // A click on Close must not destroy an HWND still borrowed by a GPU surface.
    if message == WM_CLOSE {
        return 0;
    }
    unsafe { DefWindowProcW(window, message, w, l) }
}

fn class() -> &'static Vec<u16> {
    static CLASS: OnceLock<Vec<u16>> = OnceLock::new();
    CLASS.get_or_init(|| {
        let name = wide(&format!("RevenantRendererAcceptance{}", unsafe {
            GetCurrentProcessId()
        }));
        let descriptor = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        assert_ne!(
            unsafe { RegisterClassW(&descriptor) },
            0,
            "RegisterClassW failed"
        );
        name
    })
}

struct OwnedWindow {
    handle: NonZeroIsize,
    thread: u32,
    _parent: Option<Arc<OwnedWindow>>,
}
// The handle may be borrowed by wgpu on its worker threads. Creation, resize,
// message dispatch and final destruction remain on this test's owning thread.
unsafe impl Send for OwnedWindow {}
unsafe impl Sync for OwnedWindow {}
impl HasWindowHandle for OwnedWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(self.handle)))
        })
    }
}
impl HasDisplayHandle for OwnedWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe {
            DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new()))
        })
    }
}
impl OwnedWindow {
    fn new(parent: Option<Arc<Self>>, width: i32, height: i32) -> Arc<Self> {
        let parent_handle = parent.as_ref().map_or(std::ptr::null_mut(), |p| p.hwnd());
        let style = if parent.is_some() {
            WS_CHILD | WS_VISIBLE
        } else {
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN
        };
        let title = wide("Revenant: owned DX12 acceptance fixture");
        let handle = unsafe {
            CreateWindowExW(
                0,
                class().as_ptr(),
                title.as_ptr(),
                style,
                if parent.is_some() { 0 } else { 100 },
                if parent.is_some() { 0 } else { 100 },
                width,
                height,
                parent_handle,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        Arc::new(Self {
            handle: NonZeroIsize::new(handle as isize).expect("CreateWindowExW failed"),
            thread: unsafe { GetCurrentThreadId() },
            _parent: parent,
        })
    }
    fn hwnd(&self) -> HWND {
        self.handle.get() as HWND
    }
    fn resize(&self, width: u32, height: u32) {
        assert_ne!(
            unsafe {
                SetWindowPos(
                    self.hwnd(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    width as i32,
                    height as i32,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            },
            0
        );
    }
}
impl Drop for OwnedWindow {
    fn drop(&mut self) {
        assert_eq!(
            self.thread,
            unsafe { GetCurrentThreadId() },
            "HWND disposal moved off its owner thread"
        );
        assert_ne!(
            unsafe { DestroyWindow(self.hwnd()) },
            0,
            "DestroyWindow failed after renderer disposal"
        );
    }
}

fn pump() {
    let mut message = MSG::default();
    unsafe {
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn memory() -> Value {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    counters.cb = std::mem::size_of_val(&counters) as u32;
    assert_ne!(
        unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters as *mut _ as *mut _,
                counters.cb,
            )
        },
        0
    );
    let mut handles = 0;
    assert_ne!(
        unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) },
        0
    );
    json!({ "working_set_bytes": counters.WorkingSetSize, "private_commit_bytes": counters.PrivateUsage, "peak_working_set_bytes": counters.PeakWorkingSetSize,
        "process_handles": handles, "gdi_objects": unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) }, "user_objects": unsafe { GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS) } })
}

fn stats(samples: &[f64]) -> Value {
    let mut ordered = samples.to_vec();
    ordered.sort_by(f64::total_cmp);
    let percentile = |p: f64| ordered[((ordered.len() - 1) as f64 * p).ceil() as usize];
    json!({ "count": samples.len(), "mean_ms": samples.iter().sum::<f64>() / samples.len() as f64, "p50_ms": percentile(0.5), "p95_ms": percentile(0.95), "p99_ms": percentile(0.99), "max_ms": percentile(1.0) })
}

fn primitives(revision: usize) -> Vec<Primitive> {
    (0..50_000)
        .map(|i| Primitive {
            key: i.to_string(),
            shape: if i % 2 == 0 {
                Shape::Rectangle
            } else {
                Shape::Circle
            },
            bounds: [
                (i % 250) as f32 * 4.,
                (i / 250) as f32 * 3.,
                3.8 - (revision % 2) as f32 * 0.1,
                2.85,
            ],
            color: [
                0.1 + (i % 13) as f32 * 0.035,
                0.4 + (i % 7) as f32 * 0.04,
                0.7,
                1.,
            ],
            label: format!("Fixture {i}"),
        })
        .collect()
}

struct Publisher {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<f64>>>,
}
impl Publisher {
    fn start(scene: Scene2D) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut times = Vec::new();
            let mut revision = 1;
            while !flag.load(Ordering::Acquire) {
                let begin = Instant::now();
                scene.replace(primitives(revision)).unwrap();
                times.push(begin.elapsed().as_secs_f64() * 1000.);
                revision += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            times
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
    fn finish(mut self) -> Vec<f64> {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap()
    }
}
impl Drop for Publisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
#[ignore = "Requires an interactive Windows DX12 desktop; opens only this test's owned windows"]
fn dx12_initial_scene_and_resize_fit_until_camera_interaction() {
    let parent = OwnedWindow::new(None, 850, 850);
    let viewport = OwnedWindow::new(Some(parent.clone()), 784, 279);
    let mut renderer = pollster::block_on(Renderer::new(viewport.clone(), 1, 1)).unwrap();
    renderer.options(false, false, [0.025, 0.03, 0.04, 1.]);
    renderer.resize(784, 279);
    let scene = Scene2D::new();
    renderer.render(&scene).unwrap();
    let bounds = [-200., -120., 1000., 600.];
    let primitive = |bounds| Primitive {
        key: "whole-scene".into(),
        shape: Shape::Rectangle,
        bounds,
        color: [0.2, 0.4, 0.8, 1.],
        label: String::new(),
    };
    let visible = |renderer: &Renderer, bounds: [f32; 4]| {
        let view = renderer.view_bounds();
        assert!(view[0] <= bounds[0] + 0.001 && view[1] <= bounds[1] + 0.001);
        assert!(view[0] + view[2] >= bounds[0] + bounds[2] - 0.001);
        assert!(view[1] + view[3] >= bounds[1] + bounds[3] - 0.001);
    };
    scene.replace(vec![primitive(bounds)]).unwrap();
    renderer.render(&scene).unwrap();
    visible(&renderer, bounds); // No manual fit: the 1x1 -> short viewport admission case.
    let view = renderer.view_bounds();
    for world in [[-199., -119.], [799., 479.]] {
        let pixel = [
            (world[0] - view[0]) / view[2] * 784.,
            (world[1] - view[1]) / view[3] * 279.,
        ];
        assert_eq!(renderer.pick(pixel).as_deref(), Some("whole-scene"));
    }
    renderer.pan([50., 10.]);
    let manual = renderer.view_bounds();
    let replacement = [-2000., -1000., 3000., 1800.];
    scene.replace(vec![primitive(replacement)]).unwrap();
    renderer.render(&scene).unwrap();
    assert_eq!(
        renderer.view_bounds(),
        manual,
        "scene updates must retain the user's camera"
    );
    renderer.zoom([100., 100.], 0.75);
    let zoomed = renderer.view_bounds();
    viewport.resize(279, 784);
    renderer.resize(279, 784);
    renderer.render(&scene).unwrap();
    let resized = renderer.view_bounds();
    assert_eq!(
        [resized[0], resized[1], resized[2]],
        [zoomed[0], zoomed[1], zoomed[2]]
    );
    renderer.fit(&scene);
    visible(&renderer, replacement);
    viewport.resize(784, 279);
    renderer.resize(784, 279);
    renderer.render(&scene).unwrap();
    visible(&renderer, replacement);
    scene.replace(vec![primitive(bounds)]).unwrap();
    renderer.render(&scene).unwrap();
    visible(&renderer, bounds);
    drop(renderer);
    drop(viewport);
    drop(parent);
}

#[test]
#[ignore = "Requires an interactive Windows DX12 desktop; opens only this test's owned windows"]
fn dx12_renderer_50k_and_twenty_disposal_cycles() {
    assert!(
        !unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
            .is_null()
    );
    let start = Instant::now();
    let initial_memory = memory();
    let parent = OwnedWindow::new(None, 1320, 850);
    let viewport = OwnedWindow::new(Some(parent.clone()), 1280, 768);
    let mut client = RECT::default();
    assert_ne!(unsafe { GetClientRect(viewport.hwnd(), &mut client) }, 0);
    assert_eq!(
        [client.right - client.left, client.bottom - client.top],
        [1280, 768]
    );
    pump();
    let initialization = Instant::now();
    let mut renderer = pollster::block_on(Renderer::new(viewport.clone(), 1280, 768))
        .expect("DX12 surface initialization failed");
    let initialization_ms = initialization.elapsed().as_secs_f64() * 1000.;
    let adapter = renderer.adapter().to_owned();
    let scene = Scene2D::new();
    scene.replace(primitives(0)).unwrap();
    renderer.options(false, true, [0.025, 0.03, 0.04, 1.]);
    let upload = Instant::now();
    assert!(
        !renderer.render(&scene).unwrap(),
        "Reduced motion should settle immediately"
    );
    let view = renderer.view_bounds();
    for primitive in &scene.snapshot().primitives {
        assert!(view[0] <= primitive.bounds[0] + 0.001 && view[1] <= primitive.bounds[1] + 0.001);
        assert!(view[0] + view[2] >= primitive.bounds[0] + primitive.bounds[2] - 0.001);
        assert!(view[1] + view[3] >= primitive.bounds[1] + primitive.bounds[3] - 0.001);
    }
    let first_upload_render_ms = upload.elapsed().as_secs_f64() * 1000.;
    assert_eq!(renderer.pick([2.56, 1.92]).as_deref(), Some("0"));
    for _ in 0..30 {
        pump();
        renderer.render(&scene).unwrap();
    }
    let warm_memory = memory();
    let mut renders = Vec::new();
    let mut compositor = Vec::new();
    let mut updates = Vec::new();
    let mut loop_frames = Vec::new();
    let mut revision = 0;
    let mut next_update = Instant::now();
    let background_updates =
        std::env::var("REVENANT_RENDER_BACKGROUND_UPDATES").as_deref() != Ok("0");
    let publisher = background_updates.then(|| Publisher::start(scene.clone()));
    let measured_start = Instant::now();
    // Includes the first render following each native update; do not hide upload costs.
    for frame in 0..300 {
        let frame_start = Instant::now();
        pump();
        if !background_updates && Instant::now() >= next_update {
            revision += 1;
            let update = Instant::now();
            scene.replace(primitives(revision)).unwrap();
            updates.push(update.elapsed().as_secs_f64() * 1000.);
            next_update = Instant::now() + Duration::from_millis(100);
        }
        renderer.pan([if frame % 40 < 20 { 0.5 } else { -0.5 }, 0.]);
        if frame % 60 == 0 {
            renderer.zoom(
                [640., 384.],
                if frame % 120 == 0 { 0.99 } else { 1. / 0.99 },
            );
        }
        let render = Instant::now();
        renderer.render(&scene).unwrap();
        renders.push(render.elapsed().as_secs_f64() * 1000.);
        let wait = Instant::now();
        let result = unsafe { DwmFlush() };
        assert!(result >= 0, "DwmFlush failed: {result:#x}");
        compositor.push(wait.elapsed().as_secs_f64() * 1000.);
        loop_frames.push(frame_start.elapsed().as_secs_f64() * 1000.);
    }
    let measured_seconds = measured_start.elapsed().as_secs_f64();
    if let Some(publisher) = publisher {
        updates = publisher.finish();
    }
    let buffer_bytes = renderer.buffer_bytes();
    // A separate label-visible workload exercises atlas sampling instead of
    // implying that labels were drawn inside the tiny 50k grid cells.
    let label_scene = Scene2D::new();
    label_scene
        .replace(
            (0..50)
                .map(|i| Primitive {
                    key: format!("label-{i}"),
                    shape: Shape::Rectangle,
                    bounds: [(i % 10) as f32 * 100., (i / 10) as f32 * 120., 98., 118.],
                    color: [0.15, 0.4, 0.6, 1.],
                    label: format!("Folder {i}: 123 MiB"),
                })
                .collect(),
        )
        .unwrap();
    renderer.fit(&label_scene);
    let label_first = Instant::now();
    renderer.render(&label_scene).unwrap();
    let label_first_render_ms = label_first.elapsed().as_secs_f64() * 1000.;
    let mut label_renders = Vec::new();
    for _ in 0..60 {
        pump();
        let render = Instant::now();
        renderer.render(&label_scene).unwrap();
        label_renders.push(render.elapsed().as_secs_f64() * 1000.);
        assert!(unsafe { DwmFlush() } >= 0);
    }
    drop(label_scene);
    renderer.fit(&scene);
    viewport.resize(960, 540);
    renderer.resize(960, 540);
    pump();
    renderer.render(&scene).unwrap();
    viewport.resize(1280, 768);
    renderer.resize(1280, 768);
    pump();
    renderer.fit(&scene);
    renderer.render(&scene).unwrap();
    // Verify keyed interpolation requests more work and then stops after settling.
    renderer.options(true, false, [0.025, 0.03, 0.04, 1.]);
    assert!(
        !renderer.render(&scene).unwrap(),
        "Unchanged geometry must not schedule empty animation"
    );
    let current = scene.snapshot();
    let mut moving = current.primitives[0].clone();
    moving.bounds[0] += 1.;
    scene
        .patch(current.revision, vec![SceneChange::Update(moving)])
        .unwrap();
    assert!(renderer.render(&scene).unwrap());
    std::thread::sleep(Duration::from_millis(250));
    assert!(!renderer.render(&scene).unwrap());
    renderer.options(false, false, [0.025, 0.03, 0.04, 1.]);
    unsafe {
        ShowWindow(parent.hwnd(), SW_MINIMIZE);
    }
    pump();
    unsafe {
        ShowWindow(parent.hwnd(), SW_RESTORE);
    }
    pump();
    renderer.render(&scene).unwrap();
    let child_handle = viewport.hwnd();
    drop(renderer);
    assert_eq!(
        Arc::strong_count(&viewport),
        1,
        "Surface retained HWND owner after renderer drop"
    );
    assert_ne!(unsafe { IsWindow(child_handle) }, 0);
    drop(viewport);
    assert_eq!(unsafe { IsWindow(child_handle) }, 0);
    drop(parent);
    pump();
    let after_initial_dispose = memory();
    let mut cycles = Vec::new();
    for cycle in 1..=20 {
        let before = memory();
        let cycle_start = Instant::now();
        let parent = OwnedWindow::new(None, 1320, 850);
        let viewport = OwnedWindow::new(Some(parent.clone()), 1280, 768);
        pump();
        let mut renderer = pollster::block_on(Renderer::new(viewport.clone(), 1280, 768)).unwrap();
        renderer.options(false, false, [0.025, 0.03, 0.04, 1.]);
        renderer.fit(&scene);
        for _ in 0..5 {
            pump();
            renderer.render(&scene).unwrap();
        }
        let handle = viewport.hwnd();
        drop(renderer);
        assert_eq!(Arc::strong_count(&viewport), 1);
        drop(viewport);
        assert_eq!(unsafe { IsWindow(handle) }, 0);
        drop(parent);
        pump();
        // A compositor boundary gives asynchronous driver housekeeping time, not proof of GPU idle.
        assert!(unsafe { DwmFlush() } >= 0);
        cycles.push(json!({ "cycle": cycle, "elapsed_ms": cycle_start.elapsed().as_secs_f64() * 1000., "before": before, "after": memory() }));
    }
    drop(scene);
    let report = json!({
        "schema": 1, "unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" }, "backend": "DX12", "adapter": adapter,
        "physical_viewport": [1280, 768], "primitive_count": 50_000, "rectangles": 25_000, "circles": 25_000,
        "scene_update_thread": if background_updates { "native publisher thread, at most ten replacements per second" } else { "render thread synthetic construction baseline" },
        "labels": "enabled during frame measurement, but grid cells fall below renderer label threshold",
        "initialization_ms": initialization_ms, "first_upload_render_ms": first_upload_render_ms,
        "render_call_wall": stats(&renders), "scene_build_validate_replace_cpu": stats(&updates),
        "full_loop_frame_wall": stats(&loop_frames), "frames_over_16_667_ms": loop_frames.iter().filter(|&&v|v>1000./60.).count(),
        "label_workload": { "large_rectangles": 50, "first_render_ms": label_first_render_ms, "render_call_wall": stats(&label_renders) },
        "dwm_flush_wait_wall": stats(&compositor), "measured_seconds": measured_seconds, "loop_frames_per_second": 300. / measured_seconds,
        "owned_instance_atlas_uniform_bytes": buffer_bytes,
        "initial_memory": initial_memory, "warmed_memory": warm_memory, "after_initial_dispose": after_initial_dispose,
        "disposal_cycles": cycles, "after_scene_dispose": memory(), "total_seconds": start.elapsed().as_secs_f64(),
        "checks": ["native child HWND DX12 surface", "50k upload, revisions, pan and zoom", "picking", "resize and restore", "minimize and restore", "reduced-motion settling", "animated settling", "surface owner released before HWND disposal twenty times"],
        "limitations": [
            "Renderer::render wall includes scene upload, surface acquire, CPU submit and queue present; public API cannot isolate submission or presentation time",
            "DwmFlush measures compositor synchronization; it is neither GPU execution timestamp nor proof of on-screen presentation",
            "FIFO presentation and a second compositor synchronization pace this loop; FPS is not uncapped renderer throughput",
            "Buffer estimate excludes swapchains, pipeline/driver heaps, textures besides the atlas, and retained driver caches",
            "Working set and private commit describe the complete test process, not GPU allocation; twenty samples cannot prove absence of leaks",
            "No forced device-loss, mixed-DPI, pointer-capture, covering-Svelte-dialog, keyboard-focus or navigation automation in this HWND-only harness",
            "Tiny 50k cells suppress labels; the separate 50-rectangle label-visible workload does not establish dense 20k-glyph performance"
        ]
    });
    let output = serde_json::to_string_pretty(&report).unwrap();
    if let Some(path) = std::env::var_os("REVENANT_RENDER_REPORT") {
        std::fs::write(path, &output).unwrap();
    }
    println!("{output}");
}

#[test]
#[ignore = "Creates owned visible native DX12 surfaces for incremental and shared-asset evidence"]
fn dx12_incremental_shared_assets_and_warm_eviction_cycles() {
    let service = GraphicsService::new().unwrap();
    let parent = OwnedWindow::new(None, 1320, 850);
    let mut windows = Vec::new();
    let mut renderers = Vec::new();
    let mut creation = Vec::new();
    let scene = Scene2D::new();
    scene.replace(primitives(0)).unwrap();
    for _ in 0..4 {
        let window = OwnedWindow::new(Some(parent.clone()), 1280, 768);
        let start = Instant::now();
        let mut renderer =
            pollster::block_on(Renderer::with_service(&service, window.clone(), 1280, 768))
                .unwrap();
        creation.push(start.elapsed().as_secs_f64() * 1000.);
        renderer.options(false, false, [0.025, 0.03, 0.04, 1.]);
        renderer.render(&scene).unwrap();
        if let Some(first) = renderers.first() {
            assert!(renderer.shares_device(first));
        }
        windows.push(window);
        renderers.push(renderer);
    }
    let known = service.memory();
    assert_eq!(known.devices, 1);
    assert_eq!(known.pipelines, 1);
    let full_bytes = renderers[0].last_upload_bytes();
    assert_eq!(full_bytes, 50_000 * 80);
    let staging_before = renderers
        .iter()
        .map(Renderer::staging_bytes)
        .collect::<Vec<_>>();
    let baseline = scene.snapshot();
    let changes = (0..50_000)
        .step_by(100)
        .map(|i| {
            let mut p = baseline.primitives[i].clone();
            p.color[0] = (p.color[0] + 0.1).min(1.);
            SceneChange::Update(p)
        })
        .collect();
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    OBSERVE_ALLOCATIONS.store(true, Ordering::Relaxed);
    let patch_start = Instant::now();
    scene.patch(baseline.revision, changes).unwrap();
    let patch_ms = patch_start.elapsed().as_secs_f64() * 1000.;
    OBSERVE_ALLOCATIONS.store(false, Ordering::Relaxed);
    let patch_allocations = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let patch_allocated_bytes = ALLOCATION_BYTES.load(Ordering::Relaxed);
    let mut uploads = Vec::new();
    let mut render_ms = Vec::new();
    for (i, renderer) in renderers.iter_mut().enumerate() {
        let start = Instant::now();
        renderer.render(&scene).unwrap();
        render_ms.push(start.elapsed().as_secs_f64() * 1000.);
        uploads.push(renderer.last_upload_bytes());
        assert_eq!(renderer.last_upload_bytes(), 500 * 80);
        assert_eq!(renderer.staging_bytes(), staging_before[i]);
    }
    pump();
    assert!(unsafe { DwmFlush() } >= 0);
    let adapter = renderers[0].adapter().to_owned();
    let surface_bytes = renderers
        .iter()
        .map(Renderer::surface_buffer_bytes)
        .sum::<u64>();
    let mut pick_times = Vec::new();
    for _ in 0..1000 {
        let start = Instant::now();
        std::hint::black_box(renderers[0].pick([2.56, 1.92]));
        pick_times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    drop(renderers);
    drop(windows);
    drop(parent);
    pump();
    assert_eq!(
        service.memory().devices,
        1,
        "the explicit owner retains warm assets"
    );
    let mut cycles = Vec::new();
    for cycle in 0..20 {
        let parent = OwnedWindow::new(None, 640, 480);
        let window = OwnedWindow::new(Some(parent.clone()), 600, 420);
        let start = Instant::now();
        let mut renderer =
            pollster::block_on(Renderer::with_service(&service, window.clone(), 600, 420)).unwrap();
        renderer.options(false, false, [0., 0., 0., 1.]);
        renderer.render(&scene).unwrap();
        let hwnd = window.hwnd();
        drop(renderer);
        assert_eq!(Arc::strong_count(&window), 1);
        drop(window);
        assert_eq!(unsafe { IsWindow(hwnd) }, 0);
        drop(parent);
        pump();
        cycles.push(json!({"cycle":cycle,"create_render_dispose_ms":start.elapsed().as_secs_f64()*1000.,"process":memory()}));
        assert_eq!(service.memory().devices, 1);
    }
    service.evict_idle();
    assert_eq!(service.memory().devices, 0);
    assert_eq!(service.memory().atlas_bytes, 0);
    let report = json!({"schema":1,"backend":"DX12","adapter":adapter,"profile":if cfg!(debug_assertions){"debug"}else{"release"},
        "physical_viewport":[1280,768],"primitive_count":50_000,"changed_keys":500,"surfaces":4,
        "creation_ms_cold_then_three_warm":creation,"full_instance_upload_bytes":full_bytes,"patch_instance_upload_bytes_each":uploads,
        "patch_cpu_ms":patch_ms,"patch_process_allocation_calls":patch_allocations,"patch_process_cumulative_allocated_bytes":patch_allocated_bytes,
        "patch_render_call_ms_each":render_ms,"worst_order_pick_50k_wall":stats(&pick_times),"shared_devices":known.devices,"shared_format_pipelines":known.pipelines,
        "shared_atlas_bytes":known.atlas_bytes,"surface_buffer_bytes_total":surface_bytes,"cpu_staging_bytes_each":staging_before,
        "warm_disposal_cycles":cycles,"after_idle_eviction_devices":service.memory().devices,
        "source_sha256":{
            "scene":source_hash(include_bytes!("../src/lib.rs")),"renderer":source_hash(include_bytes!("../src/gpu.rs")),
            "shared_assets":source_hash(include_bytes!("../src/gpu_shared.rs")),"shader":source_hash(include_bytes!("../src/scene.wgsl")),
            "atlas_build":source_hash(include_bytes!("../src/atlas_build.rs"))},
        "limitations":["Patch publications clone immutable Vec snapshots, so publication CPU allocations remain proportional to scene size",
            "Process allocation counts include concurrent Rust driver housekeeping; author change construction is outside the observed interval",
            "Labels disabled to isolate settled geometric edits; changed glyph visibility can require larger uploads",
            "DwmFlush is compositor synchronization, not GPU timestamps or proof of presentation of every overlapping surface",
            "Driver heaps, swapchains and pipeline byte sizes remain unknown; counted GPU buffers and atlas are separate from process memory",
            "Twenty warm cycles plus idle eviction do not prove absence of physical driver leaks or mixed-DPI behavior"]});
    let output = serde_json::to_string_pretty(&report).unwrap();
    if let Some(path) = std::env::var_os("REVENANT_INCREMENTAL_RENDER_REPORT") {
        std::fs::write(path, &output).unwrap();
    }
    println!("{output}");
}

fn source_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
