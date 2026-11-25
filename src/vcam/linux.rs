//! Linux: write YUYV frames into a v4l2loopback device. Load it once with
//! `sudo modprobe v4l2loopback exclusive_caps=1 card_label="Studio Record"`.

use super::{rgba_to_yuyv, VcamStatus, Worker};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::time::Duration;

#[repr(C)]
#[derive(Clone, Copy)]
struct PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

#[repr(C)]
union FmtUnion {
    pix: PixFormat,
    raw: [u8; 200],
    _align: *const u8,
}

#[repr(C)]
struct Format {
    kind: u32,
    fmt: FmtUnion,
}

const fn iowr(ty: u8, nr: u8, size: usize) -> libc::c_ulong {
    ((3u64 << 30) | ((size as u64) << 16) | ((ty as u64) << 8) | nr as u64) as libc::c_ulong
}

/// (path, name) of every loopback device.
pub fn list_loopbacks() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/class/video4linux") else { return out };
    for e in rd.flatten() {
        let real = std::fs::canonicalize(e.path()).unwrap_or_default();
        if !real.to_string_lossy().contains("/virtual/") {
            continue;
        }
        let name = std::fs::read_to_string(e.path().join("name")).unwrap_or_default().trim().to_string();
        out.push((format!("/dev/{}", e.file_name().to_string_lossy()), name));
    }
    out.sort();
    out
}

pub fn run(w: Worker) {
    let mut dev = None;
    while !w.stopped() {
        let list = list_loopbacks();
        let pick = match &w.preferred {
            Some(p) => list.iter().find(|(_, n)| n == p).or(list.first()).cloned(),
            None => list.first().cloned(),
        };
        if let Some(d) = pick {
            dev = Some(d);
            break;
        }
        w.set(VcamStatus::NeedsDriver(
            "No loopback camera found. Run once: sudo apt install v4l2loopback-dkms && sudo modprobe v4l2loopback exclusive_caps=1 card_label=\"Studio Record\"".into(),
        ));
        std::thread::sleep(Duration::from_millis(1500));
    }
    let Some((path, name)) = dev else { return };
    let mut file = match std::fs::OpenOptions::new().write(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            w.set(VcamStatus::Error(format!("Couldn't open {path}: {e}")));
            return;
        }
    };
    let (cx, cy) = (w.width, w.height);
    let mut fmt = Format { kind: 2 /* VIDEO_OUTPUT */, fmt: FmtUnion { raw: [0; 200] } };
    fmt.fmt.pix = PixFormat {
        width: cx,
        height: cy,
        pixelformat: u32::from_le_bytes(*b"YUYV"),
        field: 1,
        bytesperline: cx * 2,
        sizeimage: cx * cy * 2,
        colorspace: 8, // SRGB
        priv_: 0,
        flags: 0,
        ycbcr_enc: 0,
        quantization: 0,
        xfer_func: 0,
    };
    let req = iowr(b'V', 5, std::mem::size_of::<Format>());
    let r = unsafe { libc::ioctl(file.as_raw_fd(), req as _, &mut fmt) };
    if r < 0 {
        w.set(VcamStatus::Error(format!("{name} didn't accept the picture format.")));
        return;
    }
    w.set(VcamStatus::On { target: name });
    let mut buf = vec![0u8; (cx * cy * 2) as usize];
    while !w.stopped() {
        let f = match w.rx.recv_timeout(Duration::from_millis(200)) {
            Ok(f) => f,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        if f.width != cx || f.height != cy {
            continue;
        }
        rgba_to_yuyv(&f.data, cx as usize, cy as usize, &mut buf);
        if file.write_all(&buf).is_err() {
            w.set(VcamStatus::Error("The loopback camera stopped accepting pictures.".into()));
            break;
        }
    }
}
