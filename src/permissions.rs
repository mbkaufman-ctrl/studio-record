//! Privacy permissions: what the system allows Studio Record to use, how to
//! ask, and how to un-stick an approval.
//!
//! Why approvals got "stuck" before: macOS remembers a camera approval for
//! one exact code signature. An ad-hoc signed build gets a new signature on
//! every rebuild, so the switch in System Settings could be on for an old
//! build while the running one was still refused — and the app gave up
//! after the first answer instead of noticing a later approval. Now the app
//! is signed with a stable identity (see scripts/bundle-macos.sh), every
//! capture keeps watching the permission and starts the moment it's given,
//! and "Reset" clears a stale entry so macOS asks fresh.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Camera,
    Microphone,
    Screen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Granted,
    /// Never asked: pressing Allow shows the system prompt.
    NotAsked,
    Denied,
    /// Blocked by a device-management profile.
    Restricted,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Camera => "Camera",
            Kind::Microphone => "Microphone",
            Kind::Screen => "Screen & window recording",
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Kind, Status};
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    fn media(k: Kind) -> Option<&'static objc2_av_foundation::AVMediaType> {
        unsafe {
            match k {
                Kind::Camera => AVMediaTypeVideo,
                Kind::Microphone => AVMediaTypeAudio,
                Kind::Screen => None,
            }
        }
    }

    pub fn status(k: Kind) -> Status {
        if k == Kind::Screen {
            return if unsafe { CGPreflightScreenCaptureAccess() } { Status::Granted } else { Status::Denied };
        }
        let Some(m) = media(k) else { return Status::Granted };
        let s = unsafe { AVCaptureDevice::authorizationStatusForMediaType(m) };
        if s == AVAuthorizationStatus::Authorized {
            Status::Granted
        } else if s == AVAuthorizationStatus::NotDetermined {
            Status::NotAsked
        } else if s == AVAuthorizationStatus::Restricted {
            Status::Restricted
        } else {
            Status::Denied
        }
    }

    /// Shows the system prompt (asynchronously). `done` gets the answer.
    pub fn request(k: Kind, done: impl Fn(bool) + Send + 'static) {
        if k == Kind::Screen {
            let ok = unsafe { CGRequestScreenCaptureAccess() };
            done(ok);
            return;
        }
        let Some(m) = media(k) else { return done(true) };
        let block = block2::RcBlock::new(move |g: Bool| done(g.as_bool()));
        unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(m, &block) };
    }

    pub fn bundle_id() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        // …/Studio Record.app/Contents/MacOS/studio-record
        let contents = exe.parent()?.parent()?;
        let plist = std::fs::read_to_string(contents.join("Info.plist")).ok()?;
        let i = plist.find("<key>CFBundleIdentifier</key>")?;
        let rest = &plist[i..];
        let a = rest.find("<string>")? + 8;
        let b = rest.find("</string>")?;
        Some(rest[a..b].trim().to_string())
    }

    /// Forgets this app's answer so the next request asks again. Works
    /// without an administrator password for the app's own entry.
    pub fn reset(k: Kind) -> Result<(), String> {
        let id = bundle_id().ok_or("Run the Studio Record app (not the bare program) to reset permissions.")?;
        let service = match k {
            Kind::Camera => "Camera",
            Kind::Microphone => "Microphone",
            Kind::Screen => "ScreenCapture",
        };
        let out = std::process::Command::new("/usr/bin/tccutil").args(["reset", service, &id]).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    pub fn open_settings(k: Kind) {
        let pane = match k {
            Kind::Camera => "Privacy_Camera",
            Kind::Microphone => "Privacy_Microphone",
            Kind::Screen => "Privacy_ScreenCapture",
        };
        let _ = std::process::Command::new("open").arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}")).spawn();
    }

    /// Screen-recording approval only applies after a relaunch.
    pub fn relaunch() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(app) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
                let _ = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(format!("sleep 1; open -n \"{}\"", app.display()))
                    .spawn();
                std::process::exit(0);
            }
            let _ = std::process::Command::new(exe).spawn();
            std::process::exit(0);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Kind, Status};
    pub fn status(_k: Kind) -> Status {
        Status::Granted
    }
    pub fn request(_k: Kind, done: impl Fn(bool) + Send + 'static) {
        done(true);
    }
    pub fn reset(_k: Kind) -> Result<(), String> {
        Ok(())
    }
    pub fn bundle_id() -> Option<String> {
        None
    }
    pub fn open_settings(k: Kind) {
        #[cfg(windows)]
        {
            let uri = match k {
                Kind::Camera => "ms-settings:privacy-webcam",
                Kind::Microphone => "ms-settings:privacy-microphone",
                Kind::Screen => "ms-settings:privacy",
            };
            let _ = open::that(uri);
        }
        let _ = k;
    }
    pub fn relaunch() {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
            std::process::exit(0);
        }
    }
}

pub use imp::{bundle_id, open_settings, relaunch, request, reset, status};

/// Blocks until the permission is granted or `stop` returns true. Asks
/// once if never asked. Used by capture threads so they start by
/// themselves the moment the person approves — no restart needed.
pub fn wait_for(k: Kind, mut on_wait: impl FnMut(Status), stop: impl Fn() -> bool) -> bool {
    let mut asked = false;
    loop {
        let s = status(k);
        match s {
            Status::Granted => return true,
            Status::NotAsked if !asked => {
                asked = true;
                request(k, |_| {});
            }
            _ => {}
        }
        on_wait(s);
        if stop() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}
