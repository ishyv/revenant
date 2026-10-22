//! Thread-affine child viewports. No native handle is stored in a Send host state.
use crate::{DesktopRuntime, Error, Result, UpdateSink};
use revenant_core::{Handle, NativeLease};
use revenant_graphics::{Scene2D, SceneSubscription, gpu::Renderer};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    num::NonZeroIsize,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
};
use tauri::{WebviewWindow, Wry};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{
            GetCapture, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT,
            TrackMouseEvent,
        },
        WindowsAndMessaging::*,
    },
};

const WAKE: u32 = WM_APP + 17;
const FRAME_TIMER: usize = 17;
const MOUSE_LEAVE: u32 = 0x02a3; // WM_MOUSELEAVE, emitted by TrackMouseEvent.
thread_local! {static VIEWPORTS:RefCell<HashMap<String,Rc<Viewport>>>=RefCell::new(HashMap::new());}

struct WindowTarget(isize);
impl raw_window_handle::HasWindowHandle for WindowTarget {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let hwnd = NonZeroIsize::new(self.0).ok_or(raw_window_handle::HandleError::Unavailable)?;
        // SAFETY: only a live framework-owned HWND reaches Renderer::new. The
        // surface is destroyed at WM_DESTROY before Windows destroys that HWND.
        Ok(unsafe {
            raw_window_handle::WindowHandle::borrow_raw(raw_window_handle::RawWindowHandle::Win32(
                raw_window_handle::Win32WindowHandle::new(hwnd),
            ))
        })
    }
}
impl raw_window_handle::HasDisplayHandle for WindowTarget {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
    {
        Ok(raw_window_handle::DisplayHandle::windows())
    }
}

struct Viewport {
    hwnd: isize,
    scope: String,
    label: String,
    wake: Arc<WakeTarget>,
    state: RefCell<ViewportState>,
}
// A callback may have been cloned by a background publisher before subscription
// removal. Serialize posting with closure so it cannot wake a reused HWND.
struct WakeTarget(Mutex<Option<isize>>, AtomicBool);
impl WakeTarget {
    fn post(&self) {
        let target = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(hwnd) = *target
            && !self.1.swap(true, Ordering::AcqRel)
            && unsafe { PostMessageW(hwnd as HWND, WAKE, 0, 0) } == 0
        {
            self.1.store(false, Ordering::Release);
        }
    }
    fn close(&self) {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
    }
}
struct ViewportState {
    renderer: Option<Renderer>,
    scene: NativeLease<Scene2D>,
    subscription: Option<SceneSubscription>,
    sink: Option<UpdateSink>,
    visible: bool,
    drag: Option<[f32; 2]>,
    last_hover: Option<String>,
    dead: bool,
}

impl Drop for Viewport {
    fn drop(&mut self) {
        self.wake.close();
        let state = self.state.get_mut();
        state.subscription.take();
        state.renderer.take();
        // The surface and all GPU work targets have gone before window disposal.
        if !state.dead {
            unsafe {
                SetWindowLongPtrW(self.hwnd as HWND, GWLP_USERDATA, 0);
                DestroyWindow(self.hwnd as HWND);
            }
        }
    }
}

fn host_error(e: impl std::fmt::Display) -> Error {
    Error::new("native_viewport", e.to_string())
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| host_error(format!("Missing {key}")))
}

pub(crate) async fn dispatch(
    window: WebviewWindow<Wry>,
    runtime: DesktopRuntime,
    request: Value,
    sink: Option<UpdateSink>,
) -> Result<Value> {
    if serde_json::to_vec(&request)?.len() > 16 * 1024 {
        return Err(host_error("Viewport request exceeds 16 KiB"));
    }
    let scope = string(&request, "scope")?.to_owned();
    runtime.validate_window(&scope, window.label())?;
    runtime.accepting()?;
    if !runtime.inner.options.native_surfaces {
        return Err(Error::new(
            "native_surfaces_disabled",
            "Enable [desktop] native_surfaces = true",
        ));
    }
    let action = string(&request, "action")?.to_owned();
    let lease = if action == "surface.attach" {
        let handle: Handle = serde_json::from_value(
            request
                .get("scene")
                .cloned()
                .ok_or_else(|| host_error("Missing scene resource"))?,
        )?;
        Some(
            runtime
                .lease_native_resource(&scope, &handle)?
                .typed::<Scene2D>()?,
        )
    } else {
        None
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let main_window = window.clone();
    window
        .run_on_main_thread(move || {
            let result = (|| {
                // Recheck ownership after queuing: disposal may win the race.
                runtime.validate_window(&scope, main_window.label())?;
                match action.as_str() {
                    "surface.attach" => {
                        attach(&main_window, &scope, lease.expect("attach lease"), sink)
                    }
                    "surface.place" => {
                        with_viewport(&scope, &request, |v| place(v, &main_window, &request))
                    }
                    "surface.options" => with_viewport(&scope, &request, |v| {
                        let motion = request
                            .get("motion")
                            .and_then(Value::as_bool)
                            .unwrap_or(true);
                        let labels = request
                            .get("labels")
                            .and_then(Value::as_bool)
                            .unwrap_or(true);
                        let bg = request
                            .get("background")
                            .map(|v| serde_json::from_value::<[f64; 4]>(v.clone()))
                            .transpose()?
                            .unwrap_or([0.025, 0.03, 0.04, 1.]);
                        if bg
                            .iter()
                            .any(|n| !n.is_finite() || !(0.0..=1.0).contains(n))
                        {
                            return Err(host_error("Invalid viewport background"));
                        }
                        if let Some(r) = v.state.borrow_mut().renderer.as_mut() {
                            r.options(motion, labels, bg);
                        }
                        unsafe {
                            PostMessageW(v.hwnd as HWND, WAKE, 0, 0);
                        }
                        Ok(Value::Null)
                    }),
                    "surface.fit" => with_viewport(&scope, &request, |v| {
                        let mut s = v.state.borrow_mut();
                        let scene = s.scene.clone();
                        if let Some(r) = s.renderer.as_mut() {
                            r.fit(&scene);
                        }
                        unsafe {
                            PostMessageW(v.hwnd as HWND, WAKE, 0, 0);
                        }
                        Ok(Value::Null)
                    }),
                    "surface.dispose" => {
                        let id = string(&request, "viewport")?;
                        VIEWPORTS.with(|all| {
                            let v = all.borrow().get(id).cloned();
                            if v.as_ref().is_some_and(|v| v.scope != scope) {
                                return Err(host_error("Viewport belongs to another scope"));
                            }
                            let removed = all.borrow_mut().remove(id);
                            drop(removed);
                            Ok(Value::Null)
                        })
                    }
                    _ => Err(host_error("Unknown native surface action")),
                }
            })();
            let _ = tx.send(result);
        })
        .map_err(host_error)?;
    rx.await.map_err(host_error)?
}

fn with_viewport(
    scope: &str,
    request: &Value,
    f: impl FnOnce(&Viewport) -> Result<Value>,
) -> Result<Value> {
    let id = string(request, "viewport")?;
    let v = VIEWPORTS
        .with(|all| all.borrow().get(id).cloned())
        .ok_or_else(|| host_error("Viewport has been disposed"))?;
    if v.scope != scope {
        return Err(host_error("Viewport belongs to another scope"));
    }
    f(&v)
}

fn attach(
    window: &WebviewWindow<Wry>,
    scope: &str,
    scene: NativeLease<Scene2D>,
    sink: Option<UpdateSink>,
) -> Result<Value> {
    if VIEWPORTS.with(|all| all.borrow().len()) >= 16 {
        return Err(host_error("At most 16 native viewports may be attached"));
    }
    let class: Vec<u16> = "RevenantScene2D\0".encode_utf16().collect();
    let hwnd = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            window.hwnd().map_err(host_error)?.0 as HWND,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err(host_error(std::io::Error::last_os_error()));
    }
    let renderer =
        match pollster::block_on(Renderer::new(Arc::new(WindowTarget(hwnd as isize)), 1, 1)) {
            Ok(r) => r,
            Err(e) => {
                unsafe {
                    DestroyWindow(hwnd);
                }
                return Err(e);
            }
        };
    let adapter = renderer.adapter().to_owned();
    let bytes = renderer.buffer_bytes();
    let wake = Arc::new(WakeTarget(
        Mutex::new(Some(hwnd as isize)),
        AtomicBool::new(false),
    ));
    let failure_wake = wake.clone();
    renderer.on_failure(Arc::new(move || failure_wake.post()));
    let v = Rc::new(Viewport {
        hwnd: hwnd as isize,
        scope: scope.into(),
        label: window.label().into(),
        wake: wake.clone(),
        state: RefCell::new(ViewportState {
            renderer: Some(renderer),
            scene,
            subscription: None,
            sink,
            visible: false,
            drag: None,
            last_hover: None,
            dead: false,
        }),
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&v) as isize);
    }
    let subscription = v
        .state
        .borrow()
        .scene
        .subscribe(Arc::new(move || wake.post()));
    v.state.borrow_mut().subscription = Some(subscription);
    let id = uuid::Uuid::new_v4().to_string();
    VIEWPORTS.with(|all| all.borrow_mut().insert(id.clone(), v));
    Ok(json!({"viewport":id,"adapter":adapter,"bufferBytes":bytes}))
}

fn place(v: &Viewport, window: &WebviewWindow<Wry>, request: &Value) -> Result<Value> {
    let bounds: [f64; 4] = serde_json::from_value(
        request
            .get("bounds")
            .cloned()
            .ok_or_else(|| host_error("Missing CSS viewport bounds"))?,
    )?;
    if bounds.iter().any(|n| !n.is_finite() || n.abs() > 100_000.)
        || bounds[2] < 0.
        || bounds[3] < 0.
    {
        return Err(host_error("Invalid viewport bounds"));
    }
    // WebView page zoom changes CSS pixel size independently of monitor DPI.
    let scale = request
        .get("pixelRatio")
        .map(|v| v.as_f64().ok_or_else(|| host_error("Invalid pixel ratio")))
        .transpose()?
        .unwrap_or(window.scale_factor().map_err(host_error)?);
    if !scale.is_finite() || !(0.25..=8.).contains(&scale) {
        return Err(host_error("Invalid pixel ratio"));
    }
    let size = window.inner_size().map_err(host_error)?;
    let x = (bounds[0] * scale).round().clamp(0., size.width as f64) as i32;
    let y = (bounds[1] * scale).round().clamp(0., size.height as f64) as i32;
    let w = (bounds[2] * scale)
        .round()
        .clamp(0., size.width.saturating_sub(x as u32) as f64) as i32;
    let h = (bounds[3] * scale)
        .round()
        .clamp(0., size.height.saturating_sub(y as u32) as f64) as i32;
    let visible = request
        .get("visible")
        .and_then(Value::as_bool)
        .unwrap_or(true)
        && w > 0
        && h > 0;
    {
        let mut state = v.state.borrow_mut();
        state.visible = visible;
        if let Some(r) = state.renderer.as_mut() {
            r.resize(w.max(1) as u32, h.max(1) as u32);
        }
        if !visible {
            state.drag = None;
        }
    }
    unsafe {
        if !visible {
            if GetCapture() == v.hwnd as HWND {
                ReleaseCapture();
            }
            KillTimer(v.hwnd as HWND, FRAME_TIMER);
            ShowWindow(v.hwnd as HWND, SW_HIDE);
        } else {
            SetWindowPos(
                v.hwnd as HWND,
                HWND_TOP,
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            PostMessageW(v.hwnd as HWND, WAKE, 0, 0);
        }
    }
    Ok(Value::Null)
}

pub(crate) fn dispose_window(label: &str) {
    VIEWPORTS.with(|all| {
        let ids: Vec<_> = all
            .borrow()
            .iter()
            .filter(|(_, v)| v.label == label)
            .map(|(id, _)| id.clone())
            .collect();
        let removed: Vec<_> = ids
            .into_iter()
            .filter_map(|id| all.borrow_mut().remove(&id))
            .collect();
        drop(removed);
    });
}
pub(crate) fn dispose_scopes(scopes: &[String]) {
    VIEWPORTS.with(|all| {
        let ids: Vec<_> = all
            .borrow()
            .iter()
            .filter(|(_, v)| scopes.contains(&v.scope))
            .map(|(id, _)| id.clone())
            .collect();
        let removed: Vec<_> = ids
            .into_iter()
            .filter_map(|id| all.borrow_mut().remove(&id))
            .collect();
        drop(removed);
    });
}
pub(crate) fn wake_window(label: &str) {
    VIEWPORTS.with(|all| {
        for v in all.borrow().values().filter(|v| v.label == label) {
            unsafe {
                PostMessageW(v.hwnd as HWND, WAKE, 0, 0);
            }
        }
    });
}

fn event(state: &ViewportState, kind: &str, key: Option<String>, point: [f32; 2]) {
    let revision = state
        .renderer
        .as_ref()
        .map(Renderer::scene_revision)
        .unwrap_or(0);
    if let Some(sink) = &state.sink {
        sink(json!({"kind":kind,"key":key,"point":point,"revision":revision.to_string()}));
    }
}
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // This pointer belongs to an Rc kept in the UI-thread map until after
    // DestroyWindow returns. RefCell prevents aliasing under nested Windows messages.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Viewport;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    }
    let v = unsafe { &*ptr };
    if msg == WAKE {
        v.wake.1.store(false, Ordering::Release);
    }
    let Ok(mut state) = v.state.try_borrow_mut() else {
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    };
    let point = [(l as u16 as i16) as f32, ((l >> 16) as u16 as i16) as f32];
    match msg {
        WM_DESTROY => {
            v.wake.close();
            state.subscription.take();
            state.renderer.take();
            state.dead = true;
            unsafe {
                KillTimer(hwnd, FRAME_TIMER);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT | WAKE | WM_TIMER => {
            if msg == WM_PAINT {
                unsafe {
                    let mut p = std::mem::zeroed();
                    BeginPaint(hwnd, &mut p);
                    EndPaint(hwnd, &p);
                }
            }
            if state.visible
                && unsafe { IsWindowVisible(hwnd) } != 0
                && unsafe { IsIconic(GetParent(hwnd)) } == 0
            {
                let scene = state.scene.clone();
                if let Some(r) = state.renderer.as_mut() {
                    match r.render(&scene) {
                        Ok(true) => unsafe {
                            SetTimer(hwnd, FRAME_TIMER, 16, None);
                        },
                        Ok(false) => unsafe {
                            KillTimer(hwnd, FRAME_TIMER);
                        },
                        Err(e) => {
                            if let Some(sink) = &state.sink {
                                sink(json!({"kind":"error","message":e.to_string()}));
                            }
                            state.renderer.take();
                            unsafe {
                                KillTimer(hwnd, FRAME_TIMER);
                            }
                        }
                    }
                }
            } else {
                unsafe {
                    KillTimer(hwnd, FRAME_TIMER);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            unsafe {
                SetFocus(hwnd);
            }
            let key = state.renderer.as_ref().and_then(|r| r.pick(point));
            event(&state, "select", key, point);
            0
        }
        WM_LBUTTONDBLCLK => {
            let key = state.renderer.as_ref().and_then(|r| r.pick(point));
            event(&state, "activate", key, point);
            0
        }
        WM_MBUTTONDOWN | WM_RBUTTONDOWN => {
            state.drag = Some(point);
            unsafe {
                SetCapture(hwnd);
            }
            0
        }
        WM_MBUTTONUP | WM_RBUTTONUP | WM_CAPTURECHANGED => {
            state.drag = None;
            if msg != WM_CAPTURECHANGED {
                unsafe {
                    ReleaseCapture();
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            unsafe {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut track);
            }
            if let Some(last) = state.drag {
                if let Some(r) = state.renderer.as_mut() {
                    r.pan([point[0] - last[0], point[1] - last[1]]);
                }
                state.drag = Some(point);
                unsafe {
                    PostMessageW(hwnd, WAKE, 0, 0);
                }
            } else {
                let key = state.renderer.as_ref().and_then(|r| r.pick(point));
                if key != state.last_hover {
                    state.last_hover = key.clone();
                    event(&state, "hover", key, point);
                }
            }
            0
        }
        MOUSE_LEAVE => {
            if state.last_hover.take().is_some() {
                event(&state, "hover", None, point);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let mut p = POINT {
                x: point[0] as i32,
                y: point[1] as i32,
            };
            unsafe {
                ScreenToClient(hwnd, &mut p);
            }
            if let Some(r) = state.renderer.as_mut() {
                r.zoom(
                    [p.x as f32, p.y as f32],
                    if (w >> 16) as u16 as i16 > 0 {
                        0.85
                    } else {
                        1.18
                    },
                );
            }
            unsafe {
                PostMessageW(hwnd, WAKE, 0, 0);
            }
            0
        }
        WM_KEYDOWN => {
            if let Some(r) = state.renderer.as_mut() {
                match w as u32 {
                    0x25 => r.pan([40., 0.]),
                    0x27 => r.pan([-40., 0.]),
                    0x26 => r.pan([0., 40.]),
                    0x28 => r.pan([0., -40.]),
                    0xBB => r.zoom([0., 0.], 0.85),
                    0xBD => r.zoom([0., 0.], 1.18),
                    _ => {}
                }
            }
            unsafe {
                PostMessageW(hwnd, WAKE, 0, 0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}
