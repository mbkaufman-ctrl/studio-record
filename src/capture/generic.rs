//! Windows and Linux capture: cameras through nokhwa (Media Foundation /
//! Video4Linux), displays through xcap (DXGI / PipeWire / X11).
//!
//! On these systems an iPad reaches the computer through an HDMI capture
//! adapter (it shows up as a camera) or an AirPlay receiver app (capture
//! its display), so the iPad slot simply offers every camera and display.

use super::{CaptureCtx, DeviceInfo, DeviceKind, SourceState};
use crate::frame::Frame;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution};
use std::time::{Duration, Instant};

pub fn list_cameras() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    if let Ok(list) = nokhwa::query(ApiBackend::Auto) {
        for info in list {
            let idx = match info.index() {
                CameraIndex::Index(i) => i.to_string(),
                CameraIndex::String(s) => s.clone(),
            };
            let name = info.human_name();
            let lname = name.to_lowercase();
            let is_virtual = lname.contains("virtual") || lname.contains("dummy") || lname.contains("loopback");
            let mobile = lname.contains("ipad") || lname.contains("iphone");
            out.push(DeviceInfo {
                id: format!("cam:{idx}"),
                name,
                kind: if mobile { DeviceKind::MobileScreen } else { DeviceKind::Camera },
                detail: if is_virtual { "Virtual camera".into() } else { "Camera".into() },
                is_virtual,
            });
        }
    }
    out
}

pub fn run(dev: &DeviceInfo, ctx: &CaptureCtx) {
    if let Some(id) = dev.id.strip_prefix("display:") {
        run_display(id.parse().unwrap_or(0), ctx);
    } else if let Some(id) = dev.id.strip_prefix("window:") {
        run_window(id.parse().unwrap_or(0), ctx);
    } else if let Some(idx) = dev.id.strip_prefix("cam:") {
        run_camera(idx, ctx);
    } else {
        ctx.set(SourceState::Error("Unknown device".into()));
    }
}

fn run_camera(idx: &str, ctx: &CaptureCtx) {
    let index = idx.parse::<u32>().map(CameraIndex::Index).unwrap_or_else(|_| CameraIndex::String(idx.to_string()));
    let (w, h) = if ctx.opts.max_height >= 1080 { (1920, 1080) } else { (1280, 720) };
    let attempts = [
        RequestedFormatType::Closest(CameraFormat::new(Resolution::new(w, h), FrameFormat::MJPEG, 30)),
        RequestedFormatType::Closest(CameraFormat::new(Resolution::new(w, h), FrameFormat::YUYV, 30)),
        RequestedFormatType::AbsoluteHighestFrameRate,
        RequestedFormatType::None,
    ];
    let mut cam = None;
    let mut last_err = String::new();
    for a in attempts {
        match nokhwa::Camera::new(index.clone(), RequestedFormat::new::<RgbAFormat>(a)) {
            Ok(mut c) => match c.open_stream() {
                Ok(()) => {
                    cam = Some(c);
                    break;
                }
                Err(e) => last_err = e.to_string(),
            },
            Err(e) => last_err = e.to_string(),
        }
    }
    let Some(mut cam) = cam else {
        ctx.set(SourceState::Error(format!("Couldn't open the camera. It may be in use by another app. ({last_err})")));
        return;
    };
    let mut fails = 0;
    while !ctx.stopped() {
        match cam.frame() {
            Ok(buf) => match decode(&buf) {
                Some(f) => {
                    ctx.slot.put(f);
                    ctx.fps.lock().tick();
                    ctx.set(SourceState::Live);
                    fails = 0;
                }
                None => fails += 1,
            },
            Err(e) => {
                fails += 1;
                if fails > 60 {
                    ctx.set(SourceState::Error(format!("The camera stopped sending pictures ({e}).")));
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }
    let _ = cam.stop_stream();
}

fn run_display(id: u32, ctx: &CaptureCtx) {
    let monitors = match xcap::Monitor::all() {
        Ok(m) => m,
        Err(e) => {
            ctx.set(SourceState::Error(format!("Couldn't list displays: {e}")));
            return;
        }
    };
    let Some(mon) = monitors.into_iter().find(|m| m.id().ok() == Some(id)) else {
        ctx.set(SourceState::Error("This display is no longer connected.".into()));
        return;
    };

    // Preferred: the streaming recorder (DXGI duplication / PipeWire).
    if let Ok((rec, rx)) = mon.video_recorder() {
        if rec.start().is_ok() {
            let mut got_any = false;
            let started = Instant::now();
            while !ctx.stopped() {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(f) => {
                        got_any = true;
                        if f.raw.len() == (f.width * f.height * 4) as usize {
                            ctx.slot.put(Frame::new(f.width, f.height, f.raw));
                            ctx.fps.lock().tick();
                            ctx.set(SourceState::Live);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if !got_any && started.elapsed() > Duration::from_secs(3) {
                            break; // fall back to polling below
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = rec.stop();
            if ctx.stopped() {
                return;
            }
        }
    }

    // Fallback: grab still pictures at the output rate.
    let period = Duration::from_secs_f64(1.0 / ctx.opts.fps.max(1) as f64);
    while !ctx.stopped() {
        let t = Instant::now();
        match mon.capture_image() {
            Ok(img) => {
                let (w, h) = (img.width(), img.height());
                ctx.slot.put(Frame::new(w, h, img.into_raw()));
                ctx.fps.lock().tick();
                ctx.set(SourceState::Live);
            }
            Err(e) => {
                ctx.set(SourceState::Error(format!("Screen capture failed: {e}")));
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        if let Some(rest) = period.checked_sub(t.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

/// Converts whatever the camera sends into RGBA.
fn decode(buf: &nokhwa::Buffer) -> Option<Frame> {
    let res = buf.resolution();
    let (w, h) = (res.width() as usize, res.height() as usize);
    let src = buf.buffer();
    let mut out = vec![0u8; w * h * 4];
    match buf.source_frame_format() {
        FrameFormat::MJPEG => {
            let img = image::load_from_memory_with_format(src, image::ImageFormat::Jpeg).ok()?.to_rgba8();
            return Some(Frame::new(img.width(), img.height(), img.into_raw()));
        }
        FrameFormat::YUYV => {
            if src.len() < w * h * 2 {
                return None;
            }
            for i in 0..(w * h / 2) {
                let (y0, u, y1, v) = (src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]);
                yuv(y0, u, v, &mut out[i * 8..i * 8 + 4]);
                yuv(y1, u, v, &mut out[i * 8 + 4..i * 8 + 8]);
            }
        }
        FrameFormat::NV12 => {
            if src.len() < w * h * 3 / 2 {
                return None;
            }
            let (yp, uv) = src.split_at(w * h);
            for y in 0..h {
                for x in 0..w {
                    let c = (y / 2) * w + (x / 2) * 2;
                    let o = (y * w + x) * 4;
                    yuv(yp[y * w + x], uv[c], uv[c + 1], &mut out[o..o + 4]);
                }
            }
        }
        FrameFormat::GRAY => {
            for (i, g) in src.iter().take(w * h).enumerate() {
                out[i * 4..i * 4 + 4].copy_from_slice(&[*g, *g, *g, 255]);
            }
        }
        FrameFormat::RAWRGB | FrameFormat::RAWBGR => {
            let bgr = buf.source_frame_format() == FrameFormat::RAWBGR;
            for (i, p) in src.chunks_exact(3).take(w * h).enumerate() {
                let (r, b) = if bgr { (p[2], p[0]) } else { (p[0], p[2]) };
                out[i * 4..i * 4 + 4].copy_from_slice(&[r, p[1], b, 255]);
            }
        }
    }
    Some(Frame::new(w as u32, h as u32, out))
}

fn yuv(y: u8, u: u8, v: u8, out: &mut [u8]) {
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    out[0] = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
    out[1] = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
    out[2] = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
    out[3] = 255;
}

pub fn list_windows() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    let me = std::process::id();
    if let Ok(ws) = xcap::Window::all() {
        for w in ws {
            if w.is_minimized().unwrap_or(false) || w.pid().ok() == Some(me) {
                continue;
            }
            let (ww, wh) = (w.width().unwrap_or(0), w.height().unwrap_or(0));
            if ww < 160 || wh < 100 {
                continue;
            }
            let title = w.title().unwrap_or_default();
            let app = w.app_name().unwrap_or_default();
            if title.trim().is_empty() && app.trim().is_empty() {
                continue;
            }
            let Ok(id) = w.id() else { continue };
            out.push(DeviceInfo {
                id: format!("window:{id}"),
                name: if title.is_empty() { app.clone() } else { title.chars().take(60).collect() },
                kind: DeviceKind::Window,
                detail: if app.is_empty() { "Window".into() } else { app },
                is_virtual: false,
            });
        }
    }
    out
}

fn run_window(id: u32, ctx: &CaptureCtx) {
    let period = Duration::from_secs_f64(1.0 / ctx.opts.fps.clamp(1, 30) as f64);
    let mut window = None;
    while !ctx.stopped() {
        let t = Instant::now();
        if window.is_none() {
            window = xcap::Window::all().ok().and_then(|ws| ws.into_iter().find(|w| w.id().ok() == Some(id)));
            if window.is_none() {
                ctx.set(SourceState::Error("This window was closed.".into()));
                std::thread::sleep(Duration::from_millis(800));
                continue;
            }
        }
        match window.as_ref().map(|w| w.capture_image()) {
            Some(Ok(img)) => {
                let (w, h) = (img.width(), img.height());
                ctx.slot.put(Frame::new(w, h, img.into_raw()));
                ctx.fps.lock().tick();
                ctx.set(SourceState::Live);
            }
            _ => {
                window = None;
                ctx.set(SourceState::Waiting("The window is minimised or hidden.".into()));
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        if let Some(rest) = period.checked_sub(t.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}
