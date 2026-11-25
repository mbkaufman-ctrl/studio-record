//! macOS capture through AVFoundation.
//!
//! One code path serves three kinds of input:
//! * cameras (built-in, USB, Continuity Camera, capture cards),
//! * an iPad or iPhone connected by cable — macOS only lists these after the
//!   app opts in with `kCMIOHardwarePropertyAllowScreenCaptureDevices`,
//!   which is exactly what QuickTime does for "New Movie Recording",
//! * whole displays and single windows through ScreenCaptureKit (with
//!   `AVCaptureScreenInput` as a fallback for displays).

#![allow(deprecated)]

use super::{CaptureCtx, DeviceInfo, DeviceKind, Permission, SourceState};
use crate::frame::{FpsMeter, Frame, FrameSlot};
use crate::recording_audio::{ScreenAudioFeed, ScreenSamples, SCREEN_CHANNELS, SCREEN_RATE};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput, AVCaptureInput,
    AVCaptureOutput, AVCaptureScreenInput, AVCaptureSession, AVCaptureSessionPreset1280x720,
    AVCaptureSessionPreset1920x1080, AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate,
    AVMediaTypeMuxed, AVMediaTypeVideo,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved, kAudioFormatLinearPCM,
};
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer, CMTime};
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferIsPlanar,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSProcessInfo, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
};
use parking_lot::Mutex;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// CoreMediaIO / CoreGraphics bits that have no nicer binding.

#[repr(C)]
struct CMIOObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}

#[link(name = "CoreMediaIO", kind = "framework")]
extern "C" {
    fn CMIOObjectSetPropertyData(
        object: u32,
        address: *const CMIOObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *mut c_void) -> usize;
    fn CGDisplayModeRelease(mode: *mut c_void);
    fn CGDisplayVendorNumber(display: u32) -> u32;
}

/// Displays via CoreGraphics (safe from any thread, unlike NSScreen).
pub fn list_displays() -> Vec<DeviceInfo> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    unsafe {
        // Active displays first; if the screen is asleep there are none,
        // so fall back to every connected one.
        if CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) != 0 || n == 0 {
            n = 0;
            if CGGetOnlineDisplayList(16, ids.as_mut_ptr(), &mut n) != 0 {
                return Vec::new();
            }
        }
    }
    let main = unsafe { CGMainDisplayID() };
    let mut ext = 0;
    ids[..n as usize]
        .iter()
        .map(|&id| unsafe {
            let builtin = CGDisplayIsBuiltin(id) != 0;
            let mode = CGDisplayCopyDisplayMode(id);
            let (w, h) = if mode.is_null() {
                (CGDisplayPixelsWide(id), 0)
            } else {
                let r = (CGDisplayModeGetPixelWidth(mode), CGDisplayModeGetPixelHeight(mode));
                CGDisplayModeRelease(mode);
                r
            };
            let name = if builtin {
                "Built-in Display".to_string()
            } else {
                ext += 1;
                // Sidecar (an iPad used as a second screen) reports Apple's vendor id.
                if CGDisplayVendorNumber(id) == 0x610 && !builtin {
                    format!("Apple display {ext}")
                } else {
                    format!("External display {ext}")
                }
            };
            let role = if builtin { "Built-in" } else if id == main { "Main display" } else { "External" };
            DeviceInfo {
                id: format!("display:{id}"),
                name,
                kind: DeviceKind::Display,
                detail: format!("{role} · {w} × {h}"),
                is_virtual: false,
            }
        })
        .collect()
}

/// Lets cable-connected iPads and iPhones appear as capture devices.
pub fn allow_screen_capture_devices() {
    let addr = CMIOObjectPropertyAddress {
        selector: fourcc(b"yes "), // kCMIOHardwarePropertyAllowScreenCaptureDevices
        scope: fourcc(b"glob"),
        element: 0,
    };
    let allow: u32 = 1;
    unsafe {
        CMIOObjectSetPropertyData(1, &addr, 0, std::ptr::null(), 4, &allow as *const u32 as *const c_void);
    }
}

/// Opts out of App Nap: the mixer, recorder and call camera must keep full
/// speed while you present from another app.
pub fn keep_awake() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo};
    let info = NSProcessInfo::processInfo();
    let token = info.beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep | NSActivityOptions::LatencyCritical,
        &NSString::from_str("Live video mixing and recording"),
    );
    std::mem::forget(token);
}

pub fn screen_recording_allowed() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub fn screen_audio_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| NSProcessInfo::processInfo().operatingSystemVersion().majorVersion >= 13)
}

// ---------------------------------------------------------------------------
// Device list

pub fn list_av_devices() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    autoreleasepool(|_| unsafe {
        let mut seen = std::collections::HashSet::new();
        if let Some(muxed) = AVMediaTypeMuxed {
            for d in AVCaptureDevice::devicesWithMediaType(muxed).iter() {
                let uid = d.uniqueID().to_string();
                if !seen.insert(uid.clone()) {
                    continue;
                }
                let model = d.modelID().to_string();
                let kind_label = if model.to_lowercase().contains("iphone") { "iPhone screen" } else { "iPad screen" };
                out.push(DeviceInfo {
                    id: format!("av:{uid}"),
                    name: d.localizedName().to_string(),
                    kind: DeviceKind::MobileScreen,
                    detail: format!("{kind_label} · cable"),
                    is_virtual: false,
                });
            }
        }
        if let Some(video) = AVMediaTypeVideo {
            for d in AVCaptureDevice::devicesWithMediaType(video).iter() {
                let uid = d.uniqueID().to_string();
                if !seen.insert(uid.clone()) {
                    continue;
                }
                let name = d.localizedName().to_string();
                let model = d.modelID().to_string();
                let maker = d.manufacturer().to_string();
                let lname = name.to_lowercase();
                let transport = d.transportType() as u32;
                let is_virtual = transport == fourcc(b"virt") || lname.contains("virtual") || model.to_lowercase().contains("virtual");
                let detail = if is_virtual {
                    "Virtual camera".to_string()
                } else if model.starts_with("iPhone") || model.starts_with("iPad") {
                    "Continuity Camera".to_string()
                } else if transport == fourcc(b"bltn") {
                    "Built-in".to_string()
                } else if transport == fourcc(b"usb ") {
                    if !maker.is_empty() && maker != "Apple Inc." { format!("USB · {maker}") } else { "USB camera".to_string() }
                } else if !maker.is_empty() && maker != "Apple Inc." {
                    maker
                } else {
                    "Camera".to_string()
                };
                out.push(DeviceInfo { id: format!("av:{uid}"), name, kind: DeviceKind::Camera, detail, is_virtual });
            }
        }
    });
    out
}

// ---------------------------------------------------------------------------
// Sample buffer delegate

pub struct DelegateIvars {
    slot: FrameSlot,
    fps: Arc<Mutex<FpsMeter>>,
    last_frame_ms: Arc<AtomicU64>,
    epoch: Instant,
    audio: Option<(ScreenAudioFeed, Arc<AtomicBool>)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "StudioRecordSampleDelegate"]
    #[ivars = DelegateIvars]
    struct SampleDelegate;

    unsafe impl NSObjectProtocol for SampleDelegate {}

    unsafe impl SCStreamOutput for SampleDelegate {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn sc_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Screen {
                self.handle(sample);
            } else if kind == SCStreamOutputType::Audio {
                self.handle_audio(sample);
            }
        }
    }

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for SampleDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn did_output(&self, _output: &AVCaptureOutput, sample: &CMSampleBuffer, _conn: &AVCaptureConnection) {
            self.handle(sample);
        }
    }
);

impl SampleDelegate {
    fn create(ivars: DelegateIvars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    fn handle(&self, sample: &CMSampleBuffer) {
        let iv = self.ivars();
        unsafe {
            let Some(pb) = sample.image_buffer() else { return };
            if CVPixelBufferIsPlanar(&pb) {
                return; // we asked for BGRA; ignore anything else
            }
            CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
            let w = CVPixelBufferGetWidth(&pb);
            let h = CVPixelBufferGetHeight(&pb);
            let stride = CVPixelBufferGetBytesPerRow(&pb);
            let base = CVPixelBufferGetBaseAddress(&pb);
            if !base.is_null() && w > 0 && h > 0 {
                let src = std::slice::from_raw_parts(base as *const u8, stride * h);
                let f = Frame::from_bgra_rows(w, h, stride, src);
                iv.slot.put(f);
                iv.fps.lock().tick();
                iv.last_frame_ms.store(iv.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
            }
            CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
        }
    }

    fn handle_audio(&self, sample: &CMSampleBuffer) {
        let Some((feed, stopped)) = &self.ivars().audio else { return };
        if stopped.load(Ordering::Relaxed) {
            return;
        }
        let Some(sender) = feed.sender() else { return };
        let received_at = Instant::now();
        if let Some(interleaved) = copy_screen_audio(sample) {
            let _ = sender.try_send(ScreenSamples { received_at, interleaved });
        }
    }
}

fn copy_screen_audio(sample: &CMSampleBuffer) -> Option<Vec<f32>> {
    #[repr(C)]
    struct StereoBuffers {
        count: u32,
        buffers: [AudioBuffer; 2],
    }

    // ScreenCaptureKit supplies native-endian float PCM. Verify the ASBD
    // before giving Core Media writable storage in its reported layout.
    let description = unsafe { sample.format_description() }?;
    let format = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&description).as_ref() }?;
    if format.mFormatID != kAudioFormatLinearPCM
        || format.mFormatFlags & (kAudioFormatFlagIsFloat | kAudioFormatFlagIsBigEndian) != kAudioFormatFlagIsFloat
        || format.mBitsPerChannel != 32
        || format.mChannelsPerFrame != SCREEN_CHANNELS.into()
        || format.mSampleRate != f64::from(SCREEN_RATE)
    {
        return None;
    }
    let frames = i32::try_from(unsafe { sample.num_samples() }).ok()?;
    if !(1..=SCREEN_RATE as i32).contains(&frames) {
        return None;
    }
    let planar = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
    let bytes_per_frame = if planar { 4 } else { 8 };
    if format.mBytesPerFrame != bytes_per_frame {
        return None;
    }
    let mut samples = vec![0.0f32; frames as usize * 2];
    let first = samples.as_mut_ptr();
    let second = unsafe { first.add(frames as usize) };
    let mut buffers = StereoBuffers {
        count: if planar { 2 } else { 1 },
        buffers: [
            AudioBuffer {
                mNumberChannels: if planar { 1 } else { 2 },
                mDataByteSize: frames as u32 * bytes_per_frame,
                mData: first.cast(),
            },
            AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: frames as u32 * 4,
                mData: second.cast(),
            },
        ],
    };
    let list = std::ptr::NonNull::from(&mut buffers).cast::<AudioBufferList>();
    if unsafe { sample.copy_pcm_data_into_audio_buffer_list(0, frames, list) } != 0 {
        return None;
    }
    if planar {
        let (first, second) = samples.split_at(frames as usize);
        return Some(first.iter().zip(second.iter()).flat_map(|(&left, &right)| [left, right]).collect());
    }
    Some(samples)
}

// ---------------------------------------------------------------------------
// Running a source

fn wait_permission(ctx: &CaptureCtx, k: crate::permissions::Kind) -> bool {
    use crate::permissions::{wait_for, Kind};
    let p = if k == Kind::Screen { Permission::ScreenRecording } else { Permission::Camera };
    wait_for(k, |_| ctx.set(SourceState::NeedsPermission(p)), || ctx.stopped())
}

pub fn run(dev: &DeviceInfo, ctx: &CaptureCtx) {
    use crate::permissions::Kind;
    if let Some(id) = dev.id.strip_prefix("display:") {
        let id: u32 = id.parse().unwrap_or(0);
        if !wait_permission(ctx, Kind::Screen) {
            return;
        }
        ctx.set(SourceState::Starting);
        match run_sck(ctx, Target::Display(id)) {
            Ok(()) => {}
            Err(e) => {
                log::warn!("ScreenCaptureKit display capture failed ({e}); using AVCaptureScreenInput");
                run_session(ctx, Input::Display(id), dev.kind);
            }
        }
    } else if let Some(id) = dev.id.strip_prefix("window:") {
        let id: u32 = id.parse().unwrap_or(0);
        if !wait_permission(ctx, Kind::Screen) {
            return;
        }
        ctx.set(SourceState::Starting);
        if let Err(e) = run_sck(ctx, Target::Window(id)) {
            ctx.set(SourceState::Error(e));
        }
    } else if let Some(uid) = dev.id.strip_prefix("av:") {
        // Cameras and cable-connected iPads both need camera permission.
        if !wait_permission(ctx, Kind::Camera) {
            return;
        }
        ctx.set(SourceState::Starting);
        run_session(ctx, Input::Device(uid.to_string()), dev.kind);
    } else {
        ctx.set(SourceState::Error("Unknown device".into()));
    }
}

// ---------------------------------------------------------------------------
// ScreenCaptureKit

struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<SendBox<Retained<SCShareableContent>>, String>>();
    let block = block2::RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let r = unsafe {
            match Retained::retain(content) {
                Some(c) => Ok(SendBox(c)),
                None => Err(Retained::retain(err)
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "Screen recording isn't allowed".into())),
            }
        };
        let _ = tx.send(r);
    });
    unsafe { SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true, false, &block) };
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(b)) => Ok(b.0),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("The system didn't answer".into()),
    }
}

/// Windows you can share, newest-first as the system reports them.
pub fn list_windows() -> Vec<DeviceInfo> {
    if !screen_recording_allowed() {
        return Vec::new(); // asking now would pop the system prompt
    }
    let Ok(content) = shareable_content() else { return Vec::new() };
    let me = std::process::id() as i32;
    let mut out = Vec::new();
    autoreleasepool(|_| unsafe {
        for w in content.windows().iter() {
            if w.windowLayer() != 0 {
                continue;
            }
            let f = w.frame();
            if f.size.width < 160.0 || f.size.height < 100.0 {
                continue;
            }
            let app = w.owningApplication();
            if app.as_ref().is_some_and(|a| a.processID() == me) {
                continue;
            }
            let app_name = app.map(|a| a.applicationName().to_string()).unwrap_or_default();
            let title = w.title().map(|t| t.to_string()).unwrap_or_default();
            if app_name.is_empty() && title.is_empty() {
                continue;
            }
            if ["Window Server", "Dock", "Control Center", "Notification Center", "Wallpaper"].contains(&app_name.as_str()) {
                continue;
            }
            let name = if title.is_empty() { app_name.clone() } else { title.clone() };
            out.push(DeviceInfo {
                id: format!("window:{}", w.windowID()),
                name: truncate(&name, 60),
                kind: DeviceKind::Window,
                detail: if title.is_empty() { "Window".into() } else { app_name },
                is_virtual: false,
            });
        }
    });
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

enum Target {
    Display(u32),
    Window(u32),
}

fn run_sck(ctx: &CaptureCtx, target: Target) -> Result<(), String> {
    let content = shareable_content()?;
    let me = std::process::id() as i32;
    let last_frame_ms = Arc::new(AtomicU64::new(0));
    let epoch = Instant::now();
    let (stream, _delegate, _queue, window_size) = autoreleasepool(|_| unsafe {
        let (filter, w_pt, h_pt, scale): (Retained<SCContentFilter>, f64, f64, f64) = match target {
            Target::Display(id) => {
                let d = content.displays().iter().find(|d| d.displayID() == id).ok_or("This display is no longer available.")?;
                // Leave Studio Record's own windows out of the picture (no
                // hall of mirrors, the control panel stays private).
                let mine: Vec<Retained<objc2_screen_capture_kit::SCRunningApplication>> =
                    content.applications().iter().filter(|a| a.processID() == me).collect();
                let mine_refs: Vec<&objc2_screen_capture_kit::SCRunningApplication> = mine.iter().map(|a| &**a).collect();
                let apps = NSArray::from_slice(&mine_refs);
                let none: Retained<NSArray<objc2_screen_capture_kit::SCWindow>> = NSArray::new();
                let f = SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(SCContentFilter::alloc(), &d, &apps, &none);
                let px = CGDisplayPixelsWide(id).max(1) as f64;
                let scale = if d.width() > 0 { (px / d.width() as f64).max(1.0) } else { 2.0 };
                (f, d.width() as f64, d.height() as f64, scale.max(2.0))
            }
            Target::Window(id) => {
                let w = content.windows().iter().find(|w| w.windowID() == id).ok_or("This window was closed.")?;
                let fr = w.frame();
                let f = SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &w);
                (f, fr.size.width, fr.size.height, 2.0)
            }
        };
        let config = SCStreamConfiguration::new();
        let (w, h) = fit(w_pt * scale, h_pt * scale, ctx.opts.max_width as f64 * 1.0, ctx.opts.max_height as f64 * 1.0);
        config.setWidth(w);
        config.setHeight(h);
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        config.setMinimumFrameInterval(CMTime::new(1, ctx.opts.fps.max(1) as i32));
        config.setShowsCursor(true);
        config.setQueueDepth(5);
        config.setIgnoreShadowsSingleWindow(true);
        let audio = ctx.opts.screen_audio.clone().filter(|_| screen_audio_supported());
        if audio.is_some() {
            config.setCapturesAudio(true);
            config.setExcludesCurrentProcessAudio(true);
            config.setSampleRate(SCREEN_RATE as isize);
            config.setChannelCount(SCREEN_CHANNELS as isize);
        }
        let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None);
        let delegate = SampleDelegate::create(DelegateIvars {
            slot: ctx.slot.clone(),
            fps: ctx.fps.clone(),
            last_frame_ms: last_frame_ms.clone(),
            epoch,
            audio: audio.clone().map(|feed| (feed, ctx.stop.clone())),
        });
        let queue = dispatch2::DispatchQueue::new("studio-record.sck", dispatch2::DispatchQueueAttr::SERIAL);
        let proto = ProtocolObject::<dyn SCStreamOutput>::from_ref(&*delegate);
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Screen, Some(&queue))
            .map_err(|e| e.localizedDescription().to_string())?;
        if audio.is_some() {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Audio, Some(&queue))
                .map_err(|e| e.localizedDescription().to_string())?;
        }
        Ok::<_, String>((stream, delegate, queue, (w_pt, h_pt)))
    })?;

    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    let block = block2::RcBlock::new(move |err: *mut NSError| {
        let msg = unsafe { Retained::retain(err).map(|e| e.localizedDescription().to_string()) };
        let _ = tx.send(msg);
    });
    unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
    match rx.recv_timeout(Duration::from_secs(8)) {
        Ok(None) => {}
        Ok(Some(e)) => return Err(e),
        Err(_) => return Err("Screen capture didn't start".into()),
    }
    let _ = window_size;

    while !ctx.stopped() {
        std::thread::sleep(Duration::from_millis(200));
        let last = last_frame_ms.load(Ordering::Relaxed);
        let now = epoch.elapsed().as_millis() as u64;
        let st = if last == 0 && now > 3000 {
            SourceState::Waiting("Waiting for the first picture — is the window minimised?".into())
        } else if last == 0 {
            SourceState::Starting
        } else {
            SourceState::Live // idle screens send no frames: still live
        };
        if *ctx.state.lock() != st {
            ctx.set(st);
        }
    }
    unsafe { stream.stopCaptureWithCompletionHandler(None) };
    Ok(())
}

fn fit(w: f64, h: f64, max_w: f64, max_h: f64) -> (usize, usize) {
    let k = (max_w / w).min(max_h / h).min(1.0);
    let w = ((w * k) as usize).max(2) & !1;
    let h = ((h * k) as usize).max(2) & !1;
    (w, h)
}

enum Input {
    Device(String),
    Display(u32),
}

fn run_session(ctx: &CaptureCtx, input: Input, kind: DeviceKind) {
    let last_frame_ms = Arc::new(AtomicU64::new(0));
    let epoch = Instant::now();

    let built = autoreleasepool(|_| unsafe { build_session(ctx, &input, kind, last_frame_ms.clone(), epoch) });
    let (session, _delegate, _queue) = match built {
        Ok(v) => v,
        Err(msg) => {
            ctx.set(SourceState::Error(msg));
            return;
        }
    };
    unsafe { session.startRunning() };

    let mut warned = false;
    while !ctx.stopped() {
        std::thread::sleep(Duration::from_millis(150));
        let last = last_frame_ms.load(Ordering::Relaxed);
        let now = epoch.elapsed().as_millis() as u64;
        let running = unsafe { session.isRunning() };
        let quiet_for = if last == 0 { now } else { now - last };
        let state = if !running && now > 2500 {
            SourceState::Error("The device stopped. It may be in use by another app or was unplugged.".into())
        } else if last == 0 && quiet_for > 3000 {
            SourceState::Waiting(match kind {
                DeviceKind::MobileScreen => "Waiting for the iPad — unlock it and tap Trust if asked.".into(),
                DeviceKind::Display => "Waiting for the screen picture…".into(),
                DeviceKind::Camera => "Waiting for the camera…".into(),
                DeviceKind::Window => "Waiting for the window…".into(),
            })
        } else if last == 0 {
            SourceState::Starting
        } else if kind == DeviceKind::Camera && quiet_for > 4000 {
            SourceState::Waiting("No picture from the camera — is another app holding it?".into())
        } else {
            // Screens and iPads send frames only when something changes; a
            // still picture is still live.
            SourceState::Live
        };
        if !warned || *ctx.state.lock() != state {
            ctx.set(state);
            warned = true;
        }
    }
    unsafe { session.stopRunning() };
}

#[allow(clippy::type_complexity)]
unsafe fn build_session(
    ctx: &CaptureCtx,
    input: &Input,
    kind: DeviceKind,
    last_frame_ms: Arc<AtomicU64>,
    epoch: Instant,
) -> Result<(Retained<AVCaptureSession>, Retained<SampleDelegate>, dispatch2::DispatchRetained<dispatch2::DispatchQueue>), String>
{
    let session = AVCaptureSession::new();
    session.beginConfiguration();

    let av_input: Retained<AVCaptureInput> = match input {
        Input::Device(uid) => {
            let device = AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(uid))
                .ok_or_else(|| "This device is no longer connected.".to_string())?;
            if kind == DeviceKind::Camera {
                let preset = if ctx.opts.max_height >= 1080 {
                    AVCaptureSessionPreset1920x1080
                } else {
                    AVCaptureSessionPreset1280x720
                };
                if session.canSetSessionPreset(preset) {
                    session.setSessionPreset(preset);
                } else if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
                    session.setSessionPreset(AVCaptureSessionPreset1280x720);
                }
            }
            let di = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                .map_err(|e| format!("Couldn't open: {}", e.localizedDescription()))?;
            Retained::into_super(di)
        }
        Input::Display(id) => {
            let si = AVCaptureScreenInput::initWithDisplayID(AVCaptureScreenInput::alloc(), *id)
                .ok_or_else(|| "This display is no longer available.".to_string())?;
            si.setCapturesCursor(true);
            si.setCapturesMouseClicks(true);
            si.setMinFrameDuration(CMTime::new(1, ctx.opts.fps.max(1) as i32));
            // Scale at the source: no need to move 5K pixels to make 1080p.
            let px = CGDisplayPixelsWide(*id).max(1) as f64;
            let backing = px * 2.0; // retina displays report points; assume 2x worst case
            let want = ctx.opts.max_width as f64;
            let factor = (want / backing).clamp(0.25, 1.0);
            si.setScaleFactor(factor.max(want / (px * 2.0)).min(1.0));
            Retained::into_super(si)
        }
    };
    if !session.canAddInput(&av_input) {
        session.commitConfiguration();
        return Err("This device is busy in another app.".into());
    }
    session.addInput(&av_input);

    let output = AVCaptureVideoDataOutput::new();
    output.setAlwaysDiscardsLateVideoFrames(true);
    let key = NSString::from_str(&kCVPixelBufferPixelFormatTypeKey.to_string());
    let val = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let settings: Retained<NSDictionary<NSString>> = NSDictionary::from_slices::<NSString>(&[key.as_ref()], &[val.as_ref()]);
    output.setVideoSettings(Some(&settings));

    let delegate = SampleDelegate::create(DelegateIvars {
        slot: ctx.slot.clone(),
        fps: ctx.fps.clone(),
        last_frame_ms,
        epoch,
        audio: None,
    });
    let queue = dispatch2::DispatchQueue::new("studio-record.capture", dispatch2::DispatchQueueAttr::SERIAL);
    let proto = ProtocolObject::<dyn AVCaptureVideoDataOutputSampleBufferDelegate>::from_ref(&*delegate);
    let q: &dispatch2::DispatchQueue = queue.as_ref();
    let _: () = msg_send![&output, setSampleBufferDelegate: proto, queue: q];

    if !session.canAddOutput(&output) {
        session.commitConfiguration();
        return Err("Couldn't attach to this device.".into());
    }
    session.addOutput(&output);
    session.commitConfiguration();
    Ok((session, delegate, queue))
}
