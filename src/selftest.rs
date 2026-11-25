//! `studio-record --selftest [seconds]`: exercises the real pipeline without
//! a window — device discovery, camera (and screen, if allowed) capture, the
//! mixer and a short recording — and prints what happened.

use crate::capture::{self, CaptureOptions, DeviceKind};
use crate::mixer::{self, Layout, MixSettings, Mixer, Role, Scene};
use crate::recorder::{self, RecStatus, RecordRequest, Recording};
use std::time::{Duration, Instant};

pub fn run(secs: u64) -> i32 {
    #[cfg(target_os = "macos")]
    {
        capture::mac::allow_screen_capture_devices();
        capture::mac::keep_awake();
    }
    std::thread::sleep(Duration::from_millis(800));
    {
        use crate::permissions::{status, Kind};
        println!(
            "permissions: camera={:?} microphone={:?} screen={:?} bundle={:?}",
            status(Kind::Camera),
            status(Kind::Microphone),
            status(Kind::Screen),
            crate::permissions::bundle_id()
        );
        if std::env::var("SELFTEST_REQUEST").is_ok() {
            // Ask for the camera and see whether the system shows its
            // dialog (the answer stays pending) or refuses at once.
            let (tx, rx) = std::sync::mpsc::channel();
            crate::permissions::request(Kind::Camera, move |ok| {
                let _ = tx.send(ok);
            });
            match rx.recv_timeout(Duration::from_secs(6)) {
                Ok(ok) => println!("camera request answered immediately: granted={ok}"),
                Err(_) => println!("camera request pending: the system dialog is on screen"),
            }
            return 0;
        }
        if std::env::var("SELFTEST_PERMS_ONLY").is_ok() {
            return 0;
        }
    }
    let devices = capture::list_devices();
    println!("devices:");
    for d in &devices {
        println!("  {:?}  {}  ({})  virtual={}  id={}", d.kind, d.name, d.detail, d.is_virtual, d.id);
    }
    let cam = devices.iter().find(|d| d.kind == DeviceKind::Camera && !d.is_virtual && d.detail == "Built-in")
        .or_else(|| devices.iter().find(|d| d.kind == DeviceKind::Camera && !d.is_virtual));
    let screen = devices.iter().find(|d| d.kind == DeviceKind::Display);
    let opts = CaptureOptions { max_width: 1280, max_height: 720, fps: 30, screen_audio: None };

    let settings = MixSettings { size: mixer::OutputSize::P720, ..Default::default() };
    let scenes = vec![
        Scene { name: "Screen + Face".into(), layout: Layout::Pip { main: Role::Screen, pip: Role::Camera } },
        Scene { name: "Face".into(), layout: Layout::Full(Role::Camera) },
    ];
    let mixer = Mixer::start(settings, scenes, 0);
    let mut handles = Vec::new();
    if let Some(c) = cam {
        let h = capture::open(c, opts.clone());
        mixer.set_source(Role::Camera, Some(h.slot.clone()));
        handles.push(("camera", h));
    }
    #[cfg(target_os = "macos")]
    let screen_ok = capture::mac::screen_recording_allowed();
    #[cfg(not(target_os = "macos"))]
    let screen_ok = true;
    if let (Some(s), true) = (screen, screen_ok) {
        let h = capture::open(s, opts);
        mixer.set_source(Role::Screen, Some(h.slot.clone()));
        handles.push(("screen", h));
    } else {
        println!("screen: skipped (no screen-recording permission for this process)");
    }

    let Some(ff) = recorder::find_ffmpeg() else {
        println!("ffmpeg: not found");
        return 1;
    };
    let enc = recorder::pick_encoder(&ff);
    println!("ffmpeg: {}  encoder: {enc}", ff.display());
    let folder = std::env::temp_dir().join("studio-record-selftest");
    let rec = match Recording::start(RecordRequest {
        ffmpeg: ff.clone(),
        encoder: enc,
        folder: folder.clone(),
        mic: None,
        record_audio: false,
        mic_volume: 100,
        screen_audio: None,
        width: 1280,
        height: 720,
        fps: 30,
        muted: Default::default(),
    }) {
        Ok(r) => r,
        Err(e) => {
            println!("record: failed to start: {e}");
            return 1;
        }
    };
    mixer.add_output(rec.id, rec.sender());
    let t0 = Instant::now();
    let mut switched = false;
    while t0.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_millis(500));
        if !switched && t0.elapsed() > Duration::from_secs(secs / 2) {
            if let Some(f) = mixer.program.get() {
                let _ = image::RgbaImage::from_raw(f.width, f.height, f.data.clone()).map(|im| im.save(folder.join("program-scene1.png")));
            }
            mixer.switch_to(1);
            switched = true;
        }
        for (n, h) in &handles {
            let dims = h.slot.get().map(|f| format!("{}x{}", f.width, f.height)).unwrap_or("-".into());
            print!("  {n}: {:?} {dims} {:.0}fps   ", h.state(), h.fps());
        }
        let st = *mixer.stats.lock();
        println!("mixer {:.0}fps {:.1}ms", st.fps, st.render_ms);
    }
    if let Some(f) = mixer.program.get() {
        let _ = image::RgbaImage::from_raw(f.width, f.height, f.data.clone()).map(|im| im.save(folder.join("program.png")));
        println!("program frame saved: {}", folder.join("program.png").display());
    }
    rec.stop();
    mixer.remove_output(rec.id);
    let t = Instant::now();
    loop {
        match rec.status() {
            RecStatus::Saved(p) => {
                println!("record: saved {}", p.display());
                let out = std::process::Command::new(&ff).arg("-i").arg(&p).output();
                if let Ok(o) = out {
                    for l in String::from_utf8_lossy(&o.stderr).lines().filter(|l| l.contains("Duration") || l.contains("Video:")) {
                        println!("  {}", l.trim());
                    }
                }
                break;
            }
            RecStatus::Failed(e) => {
                println!("record: FAILED {e}");
                return 1;
            }
            _ if t.elapsed() > Duration::from_secs(20) => {
                println!("record: timed out finishing");
                return 1;
            }
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    mixer.stop();
    0
}
