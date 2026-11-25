//! Camera, screen and iPad capture.
//!
//! Every source runs on its own thread and drops its newest picture into a
//! [`FrameSlot`]. The mixer reads whatever is newest, so a slow source can
//! never stall the program output.

use crate::frame::{FpsMeter, FrameSlot};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(not(target_os = "macos"))]
pub mod generic;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceKind {
    /// A webcam, Continuity Camera, capture card…
    Camera,
    /// An iPad / iPhone screen over USB (macOS exposes these as capture devices).
    MobileScreen,
    /// A whole display (including an iPad used as a Sidecar display).
    Display,
    /// One app window (shared on its own, even when covered).
    Window,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable id: `av:<uid>`, `cam:<index>` or `display:<id>`.
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// Short secondary line, e.g. "Built-in" or "3024 × 1964".
    pub detail: String,
    pub is_virtual: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Camera,
    ScreenRecording,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SourceState {
    Off,
    Starting,
    Live,
    /// Running, but nothing has arrived for a while (e.g. iPad locked).
    Waiting(String),
    NeedsPermission(Permission),
    Error(String),
}

/// What the mixer would like from a source, so we can scale at the source.
#[derive(Clone)]
pub struct CaptureOptions {
    pub max_width: u32,
    pub max_height: u32,
    pub fps: u32,
    pub screen_audio: Option<crate::recording_audio::ScreenAudioFeed>,
}

pub fn screen_audio_supported() -> bool {
    #[cfg(target_os = "macos")]
    { mac::screen_audio_supported() }
    #[cfg(not(target_os = "macos"))]
    { false }
}

pub struct CaptureHandle {
    pub device: DeviceInfo,
    pub slot: FrameSlot,
    pub state: Arc<Mutex<SourceState>>,
    pub fps: Arc<Mutex<FpsMeter>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureHandle {
    pub fn state(&self) -> SourceState {
        self.state.lock().clone()
    }
    pub fn fps(&self) -> f32 {
        self.fps.lock().fps()
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            // Don't freeze the UI if a driver hangs on shutdown.
            std::thread::spawn(move || {
                let _ = t.join();
            });
        }
        self.slot.clear();
    }
}

/// Shared context passed into each platform capture thread.
#[derive(Clone)]
pub struct CaptureCtx {
    pub slot: FrameSlot,
    pub state: Arc<Mutex<SourceState>>,
    pub fps: Arc<Mutex<FpsMeter>>,
    pub stop: Arc<AtomicBool>,
    pub opts: CaptureOptions,
}

impl CaptureCtx {
    pub fn set(&self, s: SourceState) {
        *self.state.lock() = s;
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

pub fn open(device: &DeviceInfo, opts: CaptureOptions) -> CaptureHandle {
    let ctx = CaptureCtx {
        slot: FrameSlot::default(),
        state: Arc::new(Mutex::new(SourceState::Starting)),
        fps: Arc::new(Mutex::new(FpsMeter::default())),
        stop: Arc::new(AtomicBool::new(false)),
        opts,
    };
    let dev = device.clone();
    let c2 = ctx.clone();
    let thread = std::thread::Builder::new()
        .name(format!("capture {}", device.name))
        .spawn(move || run_source(dev, c2))
        .ok();
    CaptureHandle { device: device.clone(), slot: ctx.slot, state: ctx.state, fps: ctx.fps, stop: ctx.stop, thread }
}

fn run_source(dev: DeviceInfo, ctx: CaptureCtx) {
    #[cfg(target_os = "macos")]
    mac::run(&dev, &ctx);
    #[cfg(not(target_os = "macos"))]
    generic::run(&dev, &ctx);
    if !ctx.stopped() {
        // The platform loop exited on its own: leave the error state visible.
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// All cameras, iPad/iPhone screens and displays right now.
pub fn list_devices() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    out.extend(mac::list_av_devices());
    #[cfg(not(target_os = "macos"))]
    out.extend(generic::list_cameras());
    out.extend(list_displays());
    out.extend(list_windows());
    out
}

pub fn list_windows() -> Vec<DeviceInfo> {
    #[cfg(target_os = "macos")]
    return mac::list_windows();
    #[cfg(not(target_os = "macos"))]
    return generic::list_windows();
}

pub fn list_displays() -> Vec<DeviceInfo> {
    #[cfg(target_os = "macos")]
    return mac::list_displays();
    #[allow(unreachable_code)]
    let mut out = Vec::new();
    if let Ok(monitors) = xcap::Monitor::all() {
        for (i, m) in monitors.iter().enumerate() {
            let id = m.id().unwrap_or(i as u32);
            let primary = m.is_primary().unwrap_or(false);
            let builtin = m.is_builtin().unwrap_or(false);
            let scale = m.scale_factor().unwrap_or(1.0);
            let w = (m.width().unwrap_or(0) as f32 * scale) as u32;
            let h = (m.height().unwrap_or(0) as f32 * scale) as u32;
            let mut name = m.friendly_name().or_else(|_| m.name()).unwrap_or_else(|_| format!("Display {}", i + 1));
            if name.trim().is_empty() {
                name = format!("Display {}", i + 1);
            }
            let role = if builtin {
                "Built-in"
            } else if primary {
                "Main display"
            } else {
                "External"
            };
            out.push(DeviceInfo {
                id: format!("display:{id}"),
                name,
                kind: DeviceKind::Display,
                detail: format!("{role} · {w} × {h}"),
                is_virtual: false,
            });
        }
    }
    out
}

/// Keeps the device list fresh in the background, so plugging in a camera
/// or an iPad shows up without pressing anything.
#[derive(Clone)]
pub struct DeviceWatcher {
    pub devices: Arc<Mutex<Vec<DeviceInfo>>>,
    pub generation: Arc<std::sync::atomic::AtomicU64>,
}

impl DeviceWatcher {
    pub fn start() -> Self {
        #[cfg(target_os = "macos")]
        mac::allow_screen_capture_devices();
        let w = DeviceWatcher {
            devices: Arc::new(Mutex::new(Vec::new())),
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let w2 = w.clone();
        let _ = std::thread::Builder::new().name("device watcher".into()).spawn(move || loop {
            let list = list_devices();
            {
                let mut cur = w2.devices.lock();
                if *cur != list {
                    *cur = list;
                    w2.generation.fetch_add(1, Ordering::SeqCst);
                }
            }
            std::thread::sleep(Duration::from_millis(2000));
        });
        w
    }

    pub fn snapshot(&self) -> Vec<DeviceInfo> {
        self.devices.lock().clone()
    }

    pub fn refresh_now(&self) {
        let list = list_devices();
        *self.devices.lock() = list;
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

/// Opens the operating system's privacy settings for the given permission.
pub fn open_privacy_settings(p: Permission) {
    #[cfg(target_os = "macos")]
    {
        let pane = match p {
            Permission::Camera => "Privacy_Camera",
            Permission::ScreenRecording => "Privacy_ScreenCapture",
        };
        let _ = std::process::Command::new("open")
            .arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let uri = match p {
            Permission::Camera => "ms-settings:privacy-webcam",
            Permission::ScreenRecording => "ms-settings:privacy",
        };
        let _ = open::that(uri);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = p;
    }
}
