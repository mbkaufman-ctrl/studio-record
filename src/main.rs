//! Studio Record — an open-source lecture studio.
//!
//! Camera + screen + iPad, instant scene switching, recording to MP4, and a
//! camera output for Teams / Meet / Zoom. macOS, Windows and Linux.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![allow(dead_code)]

mod app;
mod audio_bridge;
mod capture;
mod frame;
mod i18n;
mod icons;
mod link;
#[cfg(target_os = "macos")]
mod macapp;
mod mixer;
mod permissions;
mod recorder;
mod selftest;
mod theme;
mod vcam;
mod widgets;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let mut shot: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--screenshot" => shot = args.next().map(PathBuf::from),
            "--selftest" => {
                let secs = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);
                std::process::exit(selftest::run(secs));
            }
            "--help" | "-h" => {
                println!("studio-record [--screenshot <file.png>] [--selftest [seconds]]");
                return Ok(());
            }
            _ => {}
        }
    }
    let checking = shot.is_some();

    // macOS: run as a real app so the camera/microphone dialogs can appear
    // (see macapp.rs for why a bare program gets refused silently).
    #[cfg(target_os = "macos")]
    if !checking {
        let passthrough: Vec<String> = std::env::args().skip(1).collect();
        if macapp::relaunch_as_app_if_needed(&passthrough) {
            return Ok(());
        }
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Studio Record")
        .with_app_id("studio-record")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([1040.0, 680.0]);
    if checking {
        viewport = viewport.with_active(false);
        #[cfg(windows)]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, persist_window: !checking, ..Default::default() };
    if checking {
        #[cfg(target_os = "macos")]
        {
            options.event_loop_builder = Some(Box::new(|b| {
                use winit::platform::macos::EventLoopBuilderExtMacOS;
                b.with_activate_ignoring_other_apps(false);
            }));
        }
    }

    eframe::run_native("Studio Record", options, Box::new(move |cc| Ok(Box::new(app::StudioApp::new(cc, shot)))))
}
