//! Making sure macOS treats Studio Record as an app of its own.
//!
//! Why "Allow" showed no dialog: when the bare program is started by
//! another app (a launcher, a terminal, an IDE), macOS attributes every
//! camera/microphone request to *that* app. If that app doesn't declare
//! camera use, the request is refused silently — no dialog, no error.
//!
//! The fix: when started bare, Studio Record installs (or refreshes) a
//! proper, signed "Studio Record.app" in ~/Applications and reopens itself
//! through Launch Services. Then Studio Record is the one asking, its
//! Info.plist explains why, and the system dialog appears.

use std::path::{Path, PathBuf};
use std::process::Command;

const INFO_PLIST: &str = include_str!("../packaging/macos/Info.plist");
const ENTITLEMENTS: &str = include_str!("../packaging/macos/entitlements.plist");

/// True when running from inside a `.app` bundle.
pub fn is_bundled() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().and_then(|p| p.parent()).map(|c| c.join("Info.plist").exists() && e.to_string_lossy().contains(".app/Contents/MacOS/")))
        .unwrap_or(false)
}

pub fn installed_app() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("Applications").join("Studio Record.app")
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The most stable signing identity on this Mac, if any.
fn signing_identity() -> Option<String> {
    let out = Command::new("/usr/bin/security").args(["find-identity", "-v", "-p", "codesigning"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for want in ["Developer ID Application", "Apple Development"] {
        if let Some(line) = text.lines().find(|l| l.contains(want)) {
            // 1) HASH "Name (TEAM)"
            if let Some(hash) = line.split_whitespace().nth(1) {
                if hash.len() == 40 {
                    return Some(hash.to_string());
                }
            }
        }
    }
    None
}

/// Installs or refreshes ~/Applications/Studio Record.app from the running
/// program. Re-signs only when the program changed, so the signature (and
/// with it every approval) stays the same between launches.
pub fn install_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let app = installed_app();
    let macos = app.join("Contents/MacOS");
    let bin = macos.join("studio-record");
    let plist = app.join("Contents/Info.plist");
    let fresh = bin.exists() && same_file(&exe, &bin) && std::fs::read_to_string(&plist).map(|p| p == INFO_PLIST).unwrap_or(false);
    if fresh {
        return Ok(app);
    }
    std::fs::create_dir_all(&macos).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(app.join("Contents/Resources")).map_err(|e| e.to_string())?;
    let tmp = macos.join(".studio-record.new");
    std::fs::copy(&exe, &tmp).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &bin).map_err(|e| e.to_string())?;
    std::fs::write(&plist, INFO_PLIST).map_err(|e| e.to_string())?;
    let ent = std::env::temp_dir().join("studio-record-entitlements.plist");
    std::fs::write(&ent, ENTITLEMENTS).map_err(|e| e.to_string())?;
    let signed = match signing_identity() {
        Some(id) => Command::new("/usr/bin/codesign")
            .args(["--force", "--options", "runtime", "--timestamp=none", "--entitlements"])
            .arg(&ent)
            .args(["--sign", &id])
            .arg(&app)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        None => false,
    };
    if !signed {
        // Last resort: ad-hoc. Works, but approvals reset when the program
        // changes.
        let _ = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&app).output();
    }
    // Tell Launch Services about the (new) bundle.
    let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
        .arg("-f")
        .arg(&app)
        .output();
    Ok(app)
}

/// Called at startup. When running bare, hands over to the app bundle and
/// returns true (the caller then exits).
pub fn relaunch_as_app_if_needed(args: &[String]) -> bool {
    if is_bundled() || std::env::var_os("STUDIO_RECORD_NO_RELAUNCH").is_some() {
        return false;
    }
    let app = match install_bundle() {
        Ok(a) => a,
        Err(e) => {
            log::warn!("couldn't install the app bundle: {e}");
            return false;
        }
    };
    let mut cmd = Command::new("/usr/bin/open");
    cmd.arg("-n").arg(&app);
    if !args.is_empty() {
        cmd.arg("--args").args(args);
    }
    match cmd.status() {
        Ok(s) if s.success() => true,
        _ => false,
    }
}
