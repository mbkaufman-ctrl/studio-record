//! "Camera output": sends the program picture into a virtual camera so
//! Teams, Meet, Zoom and the browser can pick it like any webcam.
//!
//! Each operating system needs a signed camera driver, which a plain app
//! cannot install by itself. We use the free, open-source OBS Virtual Camera
//! driver as the bridge on every platform (install OBS once; you never need
//! to open it), and also accept any other virtual camera that takes input:
//!
//! * macOS — any Camera Extension with a sink stream (CoreMediaIO), e.g.
//!   "OBS Virtual Camera".
//! * Windows — the OBS Virtual Camera DirectShow filter's shared memory.
//! * Linux — a v4l2loopback device (`sudo modprobe v4l2loopback
//!   exclusive_caps=1 card_label="Studio Record"`).

use crate::frame::FrameRef;
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq)]
pub enum VcamStatus {
    Starting,
    On { target: String },
    /// No usable virtual camera driver: the UI shows the one-time setup.
    NeedsDriver(String),
    Error(String),
}

pub struct VirtualCamera {
    pub id: u64,
    pub status: Arc<Mutex<VcamStatus>>,
    tx: Sender<FrameRef>,
    stop: Arc<AtomicBool>,
}

/// Names of virtual cameras that can take our picture right now.
pub fn list_targets() -> Vec<String> {
    #[cfg(target_os = "macos")]
    return mac::list_sinks().into_iter().map(|s| s.name).collect();
    #[cfg(target_os = "linux")]
    return linux::list_loopbacks().into_iter().map(|(_, n)| n).collect();
    #[cfg(target_os = "windows")]
    return vec!["OBS Virtual Camera".to_string()];
    #[allow(unreachable_code)]
    Vec::new()
}

pub struct Worker {
    pub rx: Receiver<FrameRef>,
    pub stop: Arc<AtomicBool>,
    pub status: Arc<Mutex<VcamStatus>>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub preferred: Option<String>,
}

impl Worker {
    pub fn set(&self, s: VcamStatus) {
        *self.status.lock() = s;
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

impl VirtualCamera {
    pub fn start(width: u32, height: u32, fps: u32, preferred: Option<String>) -> VirtualCamera {
        let (tx, rx) = bounded(2);
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(VcamStatus::Starting));
        let w = Worker { rx, stop: stop.clone(), status: status.clone(), width, height, fps, preferred };
        let _ = std::thread::Builder::new().name("virtual camera".into()).spawn(move || {
            #[cfg(target_os = "macos")]
            mac::run(w);
            #[cfg(target_os = "windows")]
            windows::run(w);
            #[cfg(target_os = "linux")]
            linux::run(w);
            #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
            w.set(VcamStatus::Error("Not supported on this system".into()));
        });
        VirtualCamera { id: NEXT_ID.fetch_add(1, Ordering::Relaxed) + 1_000_000, status, tx, stop }
    }

    pub fn sender(&self) -> Sender<FrameRef> {
        self.tx.clone()
    }

    pub fn status(&self) -> VcamStatus {
        self.status.lock().clone()
    }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// RGBA → NV12 (BT.601, limited range), as camera drivers expect.
#[allow(dead_code)]
pub fn rgba_to_nv12(src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
    let (y_plane, uv_plane) = dst.split_at_mut(w * h);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (r, g, b) = (src[i] as i32, src[i + 1] as i32, src[i + 2] as i32);
            y_plane[y * w + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
        }
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut r = 0;
            let mut g = 0;
            let mut b = 0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y + dy).min(h - 1) * w + (x + dx).min(w - 1)) * 4;
                r += src[i] as i32;
                g += src[i + 1] as i32;
                b += src[i + 2] as i32;
            }
            let (r, g, b) = (r / 4, g / 4, b / 4);
            let o = (y / 2) * w + x;
            uv_plane[o] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            uv_plane[o + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
}

/// RGBA → YUYV 4:2:2 (BT.601, limited range).
#[allow(dead_code)]
pub fn rgba_to_yuyv(src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
    for y in 0..h {
        for x in (0..w).step_by(2) {
            let i0 = (y * w + x) * 4;
            let i1 = (y * w + (x + 1).min(w - 1)) * 4;
            let p = |i: usize| (src[i] as i32, src[i + 1] as i32, src[i + 2] as i32);
            let (r0, g0, b0) = p(i0);
            let (r1, g1, b1) = p(i1);
            let y0 = ((66 * r0 + 129 * g0 + 25 * b0 + 128) >> 8) + 16;
            let y1 = ((66 * r1 + 129 * g1 + 25 * b1 + 128) >> 8) + 16;
            let (r, g, b) = ((r0 + r1) / 2, (g0 + g1) / 2, (b0 + b1) / 2);
            let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
            let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
            let o = (y * w + x) * 2;
            dst[o] = y0 as u8;
            dst[o + 1] = u.clamp(0, 255) as u8;
            dst[o + 2] = y1 as u8;
            dst[o + 3] = v.clamp(0, 255) as u8;
        }
    }
}
