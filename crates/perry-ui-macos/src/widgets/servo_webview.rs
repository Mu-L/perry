//! Experimental **Servo** web-engine backend for the WebView widget
//! (feature `servo-webview`, selected at runtime via `PERRY_WEBVIEW=servo`).
//!
//! Design: Servo renders **offscreen** into a [`SoftwareRenderingContext`] — the
//! exact path proven by the headless probe — and each frame is blitted into a
//! layer-backed `NSImageView` (the widget's view) via `NSBitmapImageRep`. An
//! `NSTimer` on the main run loop drives `spin_event_loop` + `paint` + `present`
//! + readback at ~60 Hz.
//!
//! Why software + blit instead of `WindowRenderingContext` (GPU into the view):
//! perry adds widget `NSView`s to the window **lazily** (deferred layout), so a
//! GPU rendering context — which needs a realized window handle at creation —
//! would race the layout. The software context has no window dependency, so the
//! engine is constructable the moment `create()` is called. Performance is
//! secondary for an experimental engine.
//!
//! This file is only compiled with `--features servo-webview`; the default build
//! (system WKWebView) never links Servo.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use dpi::PhysicalSize;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, MainThreadOnly};
use objc2_app_kit::{NSImageView, NSView};
use objc2_foundation::{MainThreadMarker, NSString};
use servo::{
    DeviceIntRect, DeviceIntSize, LoadStatus, RenderingContext, Servo, ServoBuilder,
    SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate,
};
use url::Url;

extern "C" {
    fn js_closure_call0(closure: *const u8) -> f64;
    fn js_closure_call1(closure: *const u8, arg: f64) -> f64;
    fn js_nanbox_get_pointer(value: f64) -> i64;
    fn js_string_from_bytes(ptr: *const u8, len: i64) -> *const u8;
    fn js_nanbox_string(ptr: i64) -> f64;
}

thread_local! {
    /// Live Servo engines keyed by Perry widget handle. Lives on the main
    /// thread only (Servo is `!Send`); the driver timer reads it on the main
    /// run loop.
    static ENGINES: RefCell<HashMap<i64, Rc<ServoEngine>>> = RefCell::new(HashMap::new());
    /// True once the shared driver `NSTimer` has been installed.
    static TIMER_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// `true` when the embedder asked for the Servo backend (`PERRY_WEBVIEW=servo`).
pub fn is_servo_requested() -> bool {
    matches!(std::env::var("PERRY_WEBVIEW").as_deref(), Ok("servo"))
}

/// `true` when `handle` is backed by a Servo engine (vs the system WKWebView).
pub fn has(handle: i64) -> bool {
    ENGINES.with(|e| e.borrow().contains_key(&handle))
}

/// Servo's wake signal. We drive painting from a periodic timer, so this only
/// needs to ensure the main run loop wakes; the timer does the spinning.
#[derive(Clone)]
struct TimerWaker;
impl servo::EventLoopWaker for TimerWaker {
    fn clone_box(&self) -> Box<dyn servo::EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {}
}

/// Per-webview delegate state: load status + the user's `onLoaded` closure.
struct ServoState {
    loaded: Cell<bool>,
    /// NaN-boxed `onLoaded` TS closure, or `0.0` if unset.
    on_loaded: Cell<f64>,
    /// Set by `notify_new_frame_ready`; cleared after a blit.
    dirty: Cell<bool>,
}

impl WebViewDelegate for ServoState {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) && !self.loaded.replace(true) {
            let cb = self.on_loaded.get();
            if cb != 0.0 {
                crate::catch_callback_panic("servo webview onLoaded", || unsafe {
                    js_closure_call0(js_nanbox_get_pointer(cb) as *const u8);
                });
            }
        }
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.dirty.set(true);
    }
}

/// A live Servo engine bound to one Perry widget view.
struct ServoEngine {
    servo: Servo,
    webview: WebView,
    rc: Rc<SoftwareRenderingContext>,
    state: Rc<ServoState>,
    /// Layer-backed `NSImageView` that displays the rendered frames.
    view: Retained<NSImageView>,
    size: Cell<(u32, u32)>,
}

impl ServoEngine {
    /// One driver step: advance Servo, paint, present, and blit the frame.
    fn tick(&self) {
        self.servo.spin_event_loop();
        self.webview.paint();
        self.rc.present();
        let (w, h) = self.size.get();
        let rect = DeviceIntRect::from_size(DeviceIntSize::new(w as i32, h as i32));
        if let Some(img) = self.rc.read_to_image(rect) {
            blit_rgba_to_image_view(&self.view, img.as_raw(), img.width(), img.height());
        }
        self.state.dirty.set(false);
    }
}

/// Create a Servo-backed WebView widget. Returns the Perry widget handle (the
/// same contract as the WKWebView `create`). Falls back to handle `0` on
/// engine-construction failure so the caller can degrade gracefully.
pub fn create(url: &str, width: f64, height: f64) -> i64 {
    let mtm = MainThreadMarker::new().expect("perry/ui must run on the main thread");
    let w = if width > 0.0 { width as u32 } else { 600 };
    let h = if height > 0.0 { height as u32 } else { 400 };

    let rc = match SoftwareRenderingContext::new(PhysicalSize::new(w, h)) {
        Ok(rc) => Rc::new(rc),
        Err(_) => return 0,
    };
    let _ = rc.make_current();

    let servo = ServoBuilder::default()
        .event_loop_waker(Box::new(TimerWaker))
        .build();

    let state = Rc::new(ServoState {
        loaded: Cell::new(false),
        on_loaded: Cell::new(0.0),
        dirty: Cell::new(true),
    });

    let mut builder = WebViewBuilder::new(&servo, rc.clone()).delegate(state.clone());
    if !url.is_empty() {
        if let Ok(parsed) = Url::parse(url) {
            builder = builder.url(parsed);
        }
    }
    let webview = builder.build();
    webview.focus();
    webview.resize(PhysicalSize::new(w, h));

    // The displayed surface: a layer-backed NSImageView sized to the widget.
    let frame = objc2_core_foundation::CGRect::new(
        objc2_core_foundation::CGPoint::new(0.0, 0.0),
        objc2_core_foundation::CGSize::new(w as f64, h as f64),
    );
    let view: Retained<NSImageView> =
        unsafe { msg_send![NSImageView::alloc(mtm), initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&*view, setWantsLayer: true];
        // NSImageScaleAxesIndependently = 3 — stretch to fill the widget box.
        let _: () = msg_send![&*view, setImageScaling: 3usize];
    }

    let view_ns: Retained<NSView> = unsafe { Retained::cast_unchecked(view.clone()) };
    let handle = super::register_widget(view_ns);

    let engine = Rc::new(ServoEngine {
        servo,
        webview,
        rc,
        state,
        view,
        size: Cell::new((w, h)),
    });
    // Paint the first frame immediately so the view isn't blank pre-load.
    engine.tick();

    ENGINES.with(|e| e.borrow_mut().insert(handle, engine));
    install_driver_timer(mtm);
    handle
}

/// Imperative `loadUrl` for a Servo-backed handle.
pub fn load_url(handle: i64, url: &str) {
    if url.is_empty() {
        return;
    }
    if let Ok(parsed) = Url::parse(url) {
        ENGINES.with(|e| {
            if let Some(engine) = e.borrow().get(&handle) {
                engine.state.loaded.set(false);
                engine.webview.load(parsed);
            }
        });
    }
}

pub fn reload(handle: i64) {
    ENGINES.with(|e| {
        if let Some(engine) = e.borrow().get(&handle) {
            engine.webview.reload();
        }
    });
}

/// Async JS evaluation. Fires `callback(result_string)` — errors and
/// null/undefined collapse to the empty string, matching the WKWebView path.
pub fn evaluate_js(handle: i64, js: &str, callback: f64) {
    ENGINES.with(|e| {
        let borrow = e.borrow();
        let Some(engine) = borrow.get(&handle) else {
            return;
        };
        engine
            .webview
            .evaluate_javascript(js.to_string(), move |result| {
                let s = match result {
                    Ok(v) => servo_jsvalue_to_string(&v),
                    Err(_) => String::new(),
                };
                if callback != 0.0 {
                    crate::catch_callback_panic("servo webview evaluateJs callback", || unsafe {
                        let js_str = js_string_from_bytes(s.as_ptr(), s.len() as i64);
                        let boxed = js_nanbox_string(js_str as i64);
                        js_closure_call1(js_nanbox_get_pointer(callback) as *const u8, boxed);
                    });
                }
            });
    });
}

/// Register the user's `onLoaded` closure for a Servo-backed handle.
pub fn set_on_loaded(handle: i64, closure: f64) {
    ENGINES.with(|e| {
        if let Some(engine) = e.borrow().get(&handle) {
            engine.state.on_loaded.set(closure);
        }
    });
}

/// Best-effort stringify of a Servo `JSValue` for `evaluateJavaScript`.
fn servo_jsvalue_to_string(value: &servo::JSValue) -> String {
    use servo::JSValue;
    match value {
        JSValue::String(s) => s.clone(),
        JSValue::Number(n) => n.to_string(),
        JSValue::Boolean(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Install the shared ~60 Hz driver timer once. It ticks every live engine on
/// the main run loop (where Servo lives).
fn install_driver_timer(_mtm: MainThreadMarker) {
    if TIMER_INSTALLED.with(|t| t.replace(true)) {
        return;
    }
    let block = block2::RcBlock::new(move |_timer: *mut AnyObject| {
        ENGINES.with(|e| {
            let engines: Vec<Rc<ServoEngine>> = e.borrow().values().cloned().collect();
            for engine in engines {
                engine.tick();
            }
        });
    });
    unsafe {
        let timer_cls = AnyClass::get(c"NSTimer").expect("NSTimer");
        let _timer: *mut AnyObject = msg_send![
            timer_cls,
            scheduledTimerWithTimeInterval: 0.016f64,
            repeats: true,
            block: &*block,
        ];
    }
    // The scheduled timer is retained by the run loop; the block by the timer.
    std::mem::forget(block);
}

/// Blit a tightly-packed RGBA8 buffer into `view` by wrapping it in an
/// `NSBitmapImageRep` (which allocates its own backing store and we memcpy into)
/// and setting it as the image-view's image.
fn blit_rgba_to_image_view(view: &NSImageView, rgba: &[u8], width: u32, height: u32) {
    if width == 0 || height == 0 || rgba.len() < (width * height * 4) as usize {
        return;
    }
    unsafe {
        let rep_cls = AnyClass::get(c"NSBitmapImageRep").expect("NSBitmapImageRep");
        let alloc: *mut AnyObject = msg_send![rep_cls, alloc];
        // planes = NULL → the rep allocates its own buffer we then fill.
        let null_planes: *mut *mut u8 = std::ptr::null_mut();
        let color_space = NSString::from_str("NSDeviceRGBColorSpace");
        let rep: *mut AnyObject = msg_send![
            alloc,
            initWithBitmapDataPlanes: null_planes,
            pixelsWide: width as isize,
            pixelsHigh: height as isize,
            bitsPerSample: 8isize,
            samplesPerPixel: 4isize,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: &*color_space,
            bytesPerRow: (width * 4) as isize,
            bitsPerPixel: 32isize,
        ];
        if rep.is_null() {
            return;
        }
        let rep = Retained::from_raw(rep).expect("rep");
        let dst: *mut u8 = msg_send![&*rep, bitmapData];
        if !dst.is_null() {
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), dst, (width * height * 4) as usize);
        }

        let img_cls = AnyClass::get(c"NSImage").expect("NSImage");
        let size = objc2_core_foundation::CGSize::new(width as f64, height as f64);
        let image: *mut AnyObject = msg_send![img_cls, alloc];
        let image: *mut AnyObject = msg_send![image, initWithSize: size];
        let _: () = msg_send![image, addRepresentation: &*rep];
        let image = Retained::from_raw(image).expect("image");
        let _: () = msg_send![view, setImage: &*image];
    }
}
