//! Native surface cases, reachable only through an explicitly selected GPU profile.
use super::{Request, changes, descriptor, json, metadata, metric, observed, primitives};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
use revenant_graphics::{
    Scene2D,
    gpu::{GraphicsService, Renderer},
};
use serde_json::Value;
use std::{
    num::NonZeroIsize,
    sync::{Arc, OnceLock},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{GetCurrentProcessId, GetCurrentThreadId},
    },
    UI::WindowsAndMessaging::*,
};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
unsafe extern "system" fn window_proc(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_CLOSE {
        return 0;
    }
    unsafe { DefWindowProcW(window, message, w, l) }
}
fn class() -> &'static Vec<u16> {
    static CLASS: OnceLock<Vec<u16>> = OnceLock::new();
    CLASS.get_or_init(|| {
        let name = wide(&format!("RevenantOptimization{}", unsafe {
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
}
// wgpu may borrow handles on worker threads; final HWND destruction stays on its owner.
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
    fn new() -> Arc<Self> {
        let title = wide("Revenant benchmark: owned DX12 surface");
        let handle = unsafe {
            CreateWindowExW(
                0,
                class().as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                100,
                100,
                640,
                360,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        let owned = Arc::new(Self {
            handle: NonZeroIsize::new(handle as isize).expect("CreateWindowExW failed"),
            thread: unsafe { GetCurrentThreadId() },
        });
        pump();
        owned
    }
}
impl Drop for OwnedWindow {
    fn drop(&mut self) {
        assert_eq!(self.thread, unsafe { GetCurrentThreadId() });
        assert_ne!(
            unsafe { DestroyWindow(self.handle.get() as HWND) },
            0,
            "DestroyWindow failed after surface disposal"
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
fn renderer(service: &GraphicsService) -> Renderer {
    let window = OwnedWindow::new();
    let mut renderer = pollster::block_on(Renderer::with_service(service, window, 640, 360))
        .expect("DX12 renderer initialization failed");
    renderer.options(false, false, [0.025, 0.03, 0.04, 1.]);
    renderer
}

fn record_hardware(surface: &Renderer, adapter: &mut Option<String>) {
    assert_eq!(
        surface.backend(),
        wgpu::Backend::Dx12,
        "GPU workload must use the actual DX12 backend"
    );
    assert!(
        !surface.adapter().is_empty(),
        "Actual GPU adapter identity is required"
    );
    if let Some(expected) = adapter {
        assert_eq!(
            surface.adapter(),
            expected,
            "GPU adapter changed during the case"
        );
    } else {
        *adapter = Some(surface.adapter().to_owned());
    }
}

fn render_present(surface: &mut Renderer, scene: &Scene2D) -> bool {
    let expected = surface
        .successful_presents()
        .checked_add(1)
        .expect("Present counter exhausted");
    let moving = surface.render(scene).unwrap();
    assert_eq!(
        surface.successful_presents(),
        expected,
        "Render skipped presentation; this benchmark run is invalid"
    );
    moving
}

pub(super) fn descriptors() -> Vec<Value> {
    let mut result = vec![];
    for surfaces in [1, 4] {
        result.push(descriptor(&format!("graphics.gpu.setup.{surfaces}"),json!({"surfaces":surfaces,"width":640,"height":360,"backend":"DX12","labels":false,"motion":false,"coldBoundary":"fresh service, compiler, first surface/device/pipeline","warmBoundary":"retained service, surface construction only","oracle":"all warm surfaces share one device; idle eviction clears assets"}),vec![metadata("cold_setup","ns"),metadata("warm_setup_batch","ns"),metadata("shared_devices","count"),metadata("shared_pipelines","count"),metadata("shared_atlas","bytes"),metadata("surface_buffers","bytes"),metadata("cpu_staging","bytes")],"gpu"));
    }
    result.push(descriptor("graphics.gpu.patch",json!({"primitives":50_000,"updates":500,"stride":100,"surfaces":1,"labels":false,"motion":false,"width":640,"height":360,"inputPreparation":"excluded","oracle":"expected revision, one-percent instance upload, stable staging/buffers"}),vec![metadata("render_call","ns"),metadata("instance_upload","bytes"),metadata("full_instance_upload","bytes"),metadata("surface_buffers","bytes"),metadata("cpu_staging","bytes")],"gpu"));
    result.push(descriptor("graphics.gpu.render",json!({"primitives":50_000,"frames":60,"surfaces":1,"labels":false,"motion":false,"width":640,"height":360,"camera":"alternating horizontal pan of 0.5 world units","oracle":"settled geometry revision and zero changed instance uploads"}),vec![metadata("render_batch","ns"),metadata("instance_upload","bytes"),metadata("surface_buffers","bytes"),metadata("cpu_staging","bytes")],"gpu"));
    result.push(descriptor("graphics.gpu.lifecycle",json!({"cycles":20,"primitivesPerCycle":50_000,"surfaces":1,"labels":false,"motion":false,"width":640,"height":360,"oracle":"render before destruction, window lifetime retained by renderer, idle assets zero after every cycle"}),vec![metadata("lifecycle_batch","ns"),metadata("remaining_devices","count"),metadata("remaining_atlas","bytes")],"gpu"));
    result
}

pub(super) fn run_case(request: &Request, mut case: Value) -> Value {
    assert_eq!(
        request.profile, "gpu",
        "Native device creation needs the explicit GPU profile"
    );
    let id = case["id"].as_str().unwrap().to_owned();
    let measured = request.measured();
    let count = request.primitives();
    let mut samples: Vec<Vec<u64>> = vec![vec![]; case["metrics"].as_array().unwrap().len()];
    let mut adapter = None;
    for repetition in 0..request.iterations() {
        let record = measured && repetition >= request.warmups;
        let values = if id.starts_with("graphics.gpu.setup.") {
            let ((service, cold), timing) = observed(record, || {
                let service = GraphicsService::new().unwrap();
                let cold = renderer(&service);
                (service, cold)
            });
            record_hardware(&cold, &mut adapter);
            drop(cold);
            let surfaces = case["parameters"]["surfaces"].as_u64().unwrap() as usize;
            let (warm, warm_timing) = observed(record, || {
                (0..surfaces)
                    .map(|_| renderer(&service))
                    .collect::<Vec<_>>()
            });
            assert!(warm.iter().all(|surface| surface.shares_device(&warm[0])));
            for surface in &warm {
                record_hardware(surface, &mut adapter);
            }
            let memory = service.memory();
            assert_eq!(memory.devices, 1);
            assert_eq!(memory.pipelines, 1);
            let buffers = warm.iter().map(Renderer::surface_buffer_bytes).sum::<u64>();
            let staging = warm
                .iter()
                .map(|renderer| renderer.staging_bytes() as u64)
                .sum::<u64>();
            drop(warm);
            service.evict_idle();
            assert_eq!(service.memory().devices, 0);
            assert_eq!(service.memory().atlas_bytes, 0);
            vec![
                timing[0],
                warm_timing[0],
                memory.devices as u64,
                memory.pipelines as u64,
                memory.atlas_bytes,
                buffers,
                staging,
            ]
        } else if id == "graphics.gpu.lifecycle" {
            let service = GraphicsService::new().unwrap();
            let base = primitives(count, 0);
            let cycles = if measured { 20 } else { 2 };
            let (_, timing) = observed(record, || {
                for _ in 0..cycles {
                    let scene = Scene2D::new();
                    scene.replace(base.clone()).unwrap();
                    let mut surface = renderer(&service);
                    record_hardware(&surface, &mut adapter);
                    render_present(&mut surface, &scene);
                    assert_eq!(surface.scene_revision(), scene.snapshot().revision);
                    drop(surface);
                    service.evict_idle();
                    let memory = service.memory();
                    assert_eq!(memory.devices, 0);
                    assert_eq!(memory.atlas_bytes, 0);
                    pump();
                }
            });
            vec![
                timing[0],
                service.memory().devices as u64,
                service.memory().atlas_bytes,
            ]
        } else {
            let service = GraphicsService::new().unwrap();
            let mut surface = renderer(&service);
            record_hardware(&surface, &mut adapter);
            let scene = Scene2D::new();
            let base = primitives(count, 0);
            let revision = scene.replace(base.clone()).unwrap();
            render_present(&mut surface, &scene);
            let full = surface.last_upload_bytes();
            assert!(full > 0);
            let buffers = surface.surface_buffer_bytes();
            let staging = surface.staging_bytes();
            let values = if id == "graphics.gpu.patch" {
                scene
                    .patch(revision, changes(&base, request.changes(), 1))
                    .unwrap();
                let (_, timing) = observed(record, || render_present(&mut surface, &scene));
                let upload = surface.last_upload_bytes();
                assert_eq!(
                    upload * count as u64,
                    full * request.changes() as u64,
                    "Changed range upload must match the selected changed-key proportion"
                );
                assert_eq!(surface.scene_revision(), revision + 1);
                vec![timing[0], upload, full, buffers, staging as u64]
            } else {
                assert_eq!(id, "graphics.gpu.render");
                let frames = if measured { 60 } else { 2 };
                let (_, timing) = observed(record, || {
                    for frame in 0..frames {
                        pump();
                        surface.pan([if frame % 2 == 0 { 0.5 } else { -0.5 }, 0.]);
                        render_present(&mut surface, &scene);
                        assert_eq!(surface.last_upload_bytes(), 0);
                    }
                });
                assert_eq!(surface.scene_revision(), revision);
                vec![
                    timing[0],
                    surface.last_upload_bytes(),
                    buffers,
                    staging as u64,
                ]
            };
            assert_eq!(surface.surface_buffer_bytes(), buffers);
            assert_eq!(surface.staging_bytes(), staging);
            drop(surface);
            service.evict_idle();
            assert_eq!(service.memory().devices, 0);
            assert_eq!(service.memory().atlas_bytes, 0);
            values
        };
        if record {
            for (target, value) in samples.iter_mut().zip(values) {
                target.push(value);
            }
        }
    }
    if measured {
        for (index, definition) in case["metrics"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            *definition = metric(
                definition["name"].as_str().unwrap(),
                definition["unit"].as_str().unwrap(),
                samples[index].clone(),
            );
        }
    } else {
        case["metrics"] = json!([]);
        case["parameters"]["verificationReduced"] = json!(true);
        if case["parameters"].get("primitives").is_some() {
            case["parameters"]["primitives"] = json!(count);
        }
        if case["parameters"].get("updates").is_some() {
            case["parameters"]["updates"] = json!(request.changes());
            case["parameters"]["stride"] = json!(count / request.changes());
        }
        if case["parameters"].get("frames").is_some() {
            case["parameters"]["frames"] = json!(2);
        }
        if case["parameters"].get("cycles").is_some() {
            case["parameters"]["cycles"] = json!(2);
            case["parameters"]["primitivesPerCycle"] = json!(count);
        }
    }
    case["hardwareIdentity"] =
        json!({"adapter":adapter.expect("Selected GPU adapter required"),"backend":"DX12"});
    case["correctness"] = json!({"passed":true,"oracle":case["parameters"]["oracle"],"hardwareIdentity":case["hardwareIdentity"],"requestedRendersPresented":true,"remainingDevices":0,"remainingAtlasBytes":0});
    case
}
