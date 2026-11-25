//! The "Link" source: a web page (slides, a whiteboard, a document) shown
//! as part of the program, with Next / Back for presentations.
//!
//! macOS: the page lives in a real WebKit view (logins and cookies are
//! kept), which Studio Record photographs ~15 times a second. Next / Back
//! are sent as genuine arrow-key presses, which every web slide tool
//! understands (Google Slides, PowerPoint Online, Canva, Keynote export,
//! reveal.js, PDF viewers…). "Show page" brings the window forward so you
//! can click, scroll or sign in.
//!
//! Windows / Linux: the link opens in your browser and you pick that
//! browser window as the Link picture.

use crate::frame::{FpsMeter, FrameSlot};
use parking_lot::Mutex;
use std::sync::Arc;

pub const PAGE_W: f64 = 1280.0;
pub const PAGE_H: f64 = 720.0;

pub fn normalize_url(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("://") || s.starts_with("about:") || s.starts_with("file:") || s.starts_with("data:") {
        return Some(s.to_string());
    }
    if s.contains('.') && !s.contains(' ') {
        return Some(format!("https://{s}"));
    }
    Some(format!("https://www.google.com/search?q={}", s.replace(' ', "+")))
}

#[cfg(target_os = "macos")]
pub use mac::LinkPage;
#[cfg(not(target_os = "macos"))]
pub use other::LinkPage;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSBackingStoreType, NSEvent, NSEventModifierFlags, NSEventType, NSImage, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSError, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURLRequest, NSURL};
    use objc2_web_kit::{WKInactiveSchedulingPolicy, WKSnapshotConfiguration, WKWebView, WKWebViewConfiguration};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGImageGetWidth(img: *const c_void) -> usize;
        fn CGImageGetHeight(img: *const c_void) -> usize;
        fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
        fn CGColorSpaceRelease(cs: *mut c_void);
        fn CGBitmapContextCreate(data: *mut c_void, w: usize, h: usize, bpc: usize, bpr: usize, cs: *mut c_void, info: u32) -> *mut c_void;
        fn CGContextDrawImage(ctx: *mut c_void, rect: CGRectRaw, img: *const c_void);
        fn CGContextRelease(ctx: *mut c_void);
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRectRaw {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    pub struct LinkPage {
        window: Retained<NSWindow>,
        web: Retained<WKWebView>,
        pub slot: FrameSlot,
        pub fps: Arc<Mutex<FpsMeter>>,
        pending: Arc<AtomicBool>,
        last_snap: Instant,
        shown: bool,
        pub url: String,
        pub max_width: u32,
        pub error: Arc<Mutex<Option<String>>>,
    }

    impl LinkPage {
        pub fn new(mtm: MainThreadMarker) -> LinkPage {
            unsafe {
                let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PAGE_W, PAGE_H));
                let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable | NSWindowStyleMask::Miniaturizable;
                let window = NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false);
                window.setReleasedWhenClosed(false);
                window.setTitle(&NSString::from_str("Studio Record — Link"));

                let config = WKWebViewConfiguration::new(mtm);
                let prefs = config.preferences();
                // Keep the page running at full speed while it's behind other
                // windows or hidden (macOS 14+).
                let sel = sel!(setInactiveSchedulingPolicy:);
                let responds: bool = msg_send![&*prefs, respondsToSelector: sel];
                if responds {
                    prefs.setInactiveSchedulingPolicy(WKInactiveSchedulingPolicy::None);
                }
                let web = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config);
                web.setCustomUserAgent(Some(&NSString::from_str(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
                )));
                window.setContentView(Some(&web));
                // Parked off-screen until "Show page" — it keeps rendering.
                window.setFrameOrigin(NSPoint::new(-30000.0, -30000.0));
                window.orderBack(None);
                LinkPage {
                    window,
                    web,
                    slot: FrameSlot::default(),
                    fps: Arc::new(Mutex::new(FpsMeter::default())),
                    pending: Arc::new(AtomicBool::new(false)),
                    last_snap: Instant::now(),
                    shown: false,
                    url: String::new(),
                    max_width: 1920,
                    error: Arc::new(Mutex::new(None)),
                }
            }
        }

        pub fn load(&mut self, url: &str) {
            let Some(u) = normalize_url(url) else { return };
            unsafe {
                let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(&u)) else {
                    *self.error.lock() = Some("That doesn't look like a web address.".into());
                    return;
                };
                *self.error.lock() = None;
                let req = NSURLRequest::requestWithURL(&nsurl);
                self.web.loadRequest(&req);
            }
            self.url = u;
        }

        pub fn current_url(&self) -> Option<String> {
            unsafe { self.web.URL().and_then(|u| u.absoluteString()).map(|s| s.to_string()) }
        }

        pub fn title(&self) -> String {
            unsafe { self.web.title().map(|t| t.to_string()).unwrap_or_default() }
        }

        pub fn is_loading(&self) -> bool {
            unsafe { self.web.isLoading() }
        }

        pub fn back(&self) {
            unsafe { self.web.goBack() };
        }
        pub fn forward(&self) {
            unsafe { self.web.goForward() };
        }
        pub fn reload(&self) {
            unsafe { self.web.reload() };
        }

        /// Next slide: a real Right-arrow key press inside the page.
        pub fn next(&self) {
            self.key(124, '\u{F703}');
        }
        /// Previous slide: Left arrow.
        pub fn prev(&self) {
            self.key(123, '\u{F702}');
        }

        fn key(&self, code: u16, ch: char) {
            let chars = NSString::from_str(&ch.to_string());
            let wn = self.window.windowNumber();
            self.window.makeFirstResponder(Some(&self.web));
            for ty in [NSEventType::KeyDown, NSEventType::KeyUp] {
                let ev = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                    ty,
                    NSPoint::new(0.0, 0.0),
                    NSEventModifierFlags::NumericPad | NSEventModifierFlags::Function,
                    0.0,
                    wn,
                    None,
                    &chars,
                    &chars,
                    false,
                    code,
                );
                if let Some(ev) = ev {
                    if ty == NSEventType::KeyDown {
                        self.web.keyDown(&ev);
                    } else {
                        self.web.keyUp(&ev);
                    }
                }
            }
        }

        pub fn shown(&self) -> bool {
            self.shown
        }

        /// Brings the page forward to click/scroll/sign in, or parks it.
        pub fn set_shown(&mut self, show: bool) {
            self.shown = show;
            unsafe {
                if show {
                    self.window.center();
                    self.window.makeKeyAndOrderFront(None);
                    let app = objc2_app_kit::NSApplication::sharedApplication(MainThreadMarker::from(&*self.window));
                    #[allow(deprecated)]
                    app.activateIgnoringOtherApps(true);
                } else {
                    self.window.setFrameOrigin(NSPoint::new(-30000.0, -30000.0));
                    self.window.orderBack(None);
                }
            }
        }

        /// Call every UI frame (main thread): takes a fresh picture of the
        /// page when the previous one has arrived.
        pub fn tick(&mut self) {
            // The close button only hides the window: park it again.
            if self.shown && !self.window.isVisible() {
                self.set_shown(false);
            }
            if self.url.is_empty() || self.pending.load(Ordering::SeqCst) || self.last_snap.elapsed() < Duration::from_millis(66) {
                return;
            }
            self.pending.store(true, Ordering::SeqCst);
            self.last_snap = Instant::now();
            let slot = self.slot.clone();
            let pending = self.pending.clone();
            let fps = self.fps.clone();
            let maxw = self.max_width as f64;
            unsafe {
                let cfg = WKSnapshotConfiguration::new(MainThreadMarker::from(&*self.window));
                let scale = self.window.backingScaleFactor().max(1.0);
                let frame: NSRect = msg_send![&*self.web, frame];
                let want_pt = (maxw / scale).min(frame.size.width.max(1.0));
                cfg.setSnapshotWidth(Some(&NSNumber::new_f64(want_pt)));
                let block = block2::RcBlock::new(move |img: *mut NSImage, _err: *mut NSError| {
                    if let Some(img) = img.as_ref() {
                        if let Some(f) = image_to_frame(img, maxw as usize) {
                            slot.put(f);
                            fps.lock().tick();
                        }
                    }
                    pending.store(false, Ordering::SeqCst);
                });
                self.web.takeSnapshotWithConfiguration_completionHandler(Some(&cfg), &block);
            }
        }
    }

    impl Drop for LinkPage {
        fn drop(&mut self) {
            self.window.orderOut(None);
        }
    }

    unsafe fn image_to_frame(img: &NSImage, max_w: usize) -> Option<crate::frame::Frame> {
        let cg = img.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)?;
        let p = &*cg as *const _ as *const c_void;
        let (sw, sh) = (CGImageGetWidth(p), CGImageGetHeight(p));
        if sw == 0 || sh == 0 {
            return None;
        }
        let k = (max_w as f64 / sw as f64).min(1.0);
        let (w, h) = (((sw as f64 * k) as usize).max(1), ((sh as f64 * k) as usize).max(1));
        let mut data = vec![0u8; w * h * 4];
        let cs = CGColorSpaceCreateDeviceRGB();
        // RGBA, premultiplied alpha, big-endian byte order.
        let ctx = CGBitmapContextCreate(data.as_mut_ptr() as *mut c_void, w, h, 8, w * 4, cs, 1 | (4 << 12));
        CGColorSpaceRelease(cs);
        if ctx.is_null() {
            return None;
        }
        CGContextDrawImage(ctx, CGRectRaw { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }, p);
        CGContextRelease(ctx);
        for px in data.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Some(crate::frame::Frame::new(w as u32, h as u32, data))
    }

    #[allow(dead_code)]
    fn _unused(_: &AnyObject) {}
}

#[cfg(not(target_os = "macos"))]
mod other {
    use super::*;

    /// On Windows/Linux the page opens in the browser; the Link picture is
    /// the browser window chosen in Sources.
    pub struct LinkPage {
        pub slot: FrameSlot,
        pub fps: Arc<Mutex<FpsMeter>>,
        pub url: String,
        pub max_width: u32,
        pub error: Arc<Mutex<Option<String>>>,
    }

    impl LinkPage {
        pub fn new() -> LinkPage {
            LinkPage { slot: FrameSlot::default(), fps: Default::default(), url: String::new(), max_width: 1920, error: Default::default() }
        }
        pub fn load(&mut self, url: &str) {
            if let Some(u) = normalize_url(url) {
                let _ = open::that(&u);
                self.url = u;
            }
        }
        pub fn current_url(&self) -> Option<String> {
            (!self.url.is_empty()).then(|| self.url.clone())
        }
        pub fn title(&self) -> String {
            String::new()
        }
        pub fn is_loading(&self) -> bool {
            false
        }
        pub fn back(&self) {}
        pub fn forward(&self) {}
        pub fn reload(&self) {
            if !self.url.is_empty() {
                let _ = open::that(&self.url);
            }
        }
        pub fn next(&self) {}
        pub fn prev(&self) {}
        pub fn shown(&self) -> bool {
            false
        }
        pub fn set_shown(&mut self, _s: bool) {}
        pub fn tick(&mut self) {}
    }
}
