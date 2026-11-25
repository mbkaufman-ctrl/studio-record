//! macOS: feed a CoreMediaIO Camera Extension through its sink stream.
//!
//! This is the public, documented way for an app to hand frames to a
//! virtual camera: find the device, take the sink (output) stream's buffer
//! queue, start the stream, and enqueue IOSurface-backed sample buffers.

use super::{VcamStatus, Worker};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSString};
use std::ffi::c_void;
use std::time::Duration;

type OSStatus = i32;
type CFTypeRef = *const c_void;

#[repr(C)]
struct Addr {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
struct CMSampleTimingInfo {
    duration: CMTime,
    pts: CMTime,
    dts: CMTime,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}

const SYSTEM_OBJECT: u32 = 1;
const GLOBAL: u32 = fourcc(b"glob");
const OUTPUT_SCOPE: u32 = fourcc(b"outp");

#[link(name = "CoreMediaIO", kind = "framework")]
extern "C" {
    fn CMIOObjectGetPropertyDataSize(o: u32, a: *const Addr, qs: u32, q: *const c_void, size: *mut u32) -> OSStatus;
    fn CMIOObjectGetPropertyData(
        o: u32,
        a: *const Addr,
        qs: u32,
        q: *const c_void,
        size: u32,
        used: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
    fn CMIOStreamCopyBufferQueue(
        stream: u32,
        altered: Option<extern "C" fn(u32, *mut c_void, *mut c_void)>,
        refcon: *mut c_void,
        queue: *mut *mut c_void,
    ) -> OSStatus;
    fn CMIODeviceStartStream(device: u32, stream: u32) -> OSStatus;
    fn CMIODeviceStopStream(device: u32, stream: u32) -> OSStatus;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMClockGetHostTimeClock() -> *const c_void;
    fn CMClockGetTime(clock: *const c_void) -> CMTime;
    fn CMVideoFormatDescriptionCreateForImageBuffer(alloc: CFTypeRef, img: *mut c_void, out: *mut *mut c_void) -> OSStatus;
    fn CMSampleBufferCreateForImageBuffer(
        alloc: CFTypeRef,
        img: *mut c_void,
        ready: u8,
        cb: *const c_void,
        refcon: *const c_void,
        fmt: *mut c_void,
        timing: *const CMSampleTimingInfo,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn CMSimpleQueueEnqueue(q: *mut c_void, e: *const c_void) -> OSStatus;
    fn CMSimpleQueueGetCount(q: *mut c_void) -> i32;
    fn CMSimpleQueueGetCapacity(q: *mut c_void) -> i32;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferCreate(alloc: CFTypeRef, w: usize, h: usize, fmt: u32, attrs: CFTypeRef, out: *mut *mut c_void) -> i32;
    fn CVPixelBufferLockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(pb: *mut c_void) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(o: CFTypeRef);
    fn CFStringGetCString(s: CFTypeRef, buf: *mut u8, size: isize, enc: u32) -> u8;
}

fn get_u32s(obj: u32, selector: u32, scope: u32) -> Vec<u32> {
    let a = Addr { selector, scope, element: 0 };
    let mut size = 0u32;
    unsafe {
        if CMIOObjectGetPropertyDataSize(obj, &a, 0, std::ptr::null(), &mut size) != 0 || size == 0 {
            return Vec::new();
        }
        let mut v = vec![0u32; size as usize / 4];
        let mut used = 0u32;
        if CMIOObjectGetPropertyData(obj, &a, 0, std::ptr::null(), size, &mut used, v.as_mut_ptr() as *mut c_void) != 0 {
            return Vec::new();
        }
        v.truncate(used as usize / 4);
        v
    }
}

fn get_name(obj: u32) -> String {
    let a = Addr { selector: fourcc(b"lnam"), scope: GLOBAL, element: 0 };
    let mut s: CFTypeRef = std::ptr::null();
    let mut used = 0u32;
    unsafe {
        if CMIOObjectGetPropertyData(
            obj,
            &a,
            0,
            std::ptr::null(),
            std::mem::size_of::<CFTypeRef>() as u32,
            &mut used,
            &mut s as *mut CFTypeRef as *mut c_void,
        ) != 0
            || s.is_null()
        {
            return String::new();
        }
        let mut buf = [0u8; 512];
        let ok = CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, 0x0800_0100);
        CFRelease(s);
        if ok == 0 {
            return String::new();
        }
        let n = buf.iter().position(|b| *b == 0).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

pub struct Sink {
    pub name: String,
    device: u32,
    stream: u32,
}

/// Every camera device that exposes a sink stream we can write into.
pub fn list_sinks() -> Vec<Sink> {
    let mut out = Vec::new();
    for dev in get_u32s(SYSTEM_OBJECT, fourcc(b"dev#"), GLOBAL) {
        let sinks = get_u32s(dev, fourcc(b"stm#"), OUTPUT_SCOPE);
        if let Some(stream) = sinks.first() {
            out.push(Sink { name: get_name(dev), device: dev, stream: *stream });
        }
    }
    out
}

extern "C" fn queue_altered(_s: u32, _t: *mut c_void, _r: *mut c_void) {}

pub fn run(w: Worker) {
    // Wait for a sink to exist (the user may be installing the driver).
    let mut sink = None;
    while !w.stopped() {
        let sinks = list_sinks();
        let pick = match &w.preferred {
            Some(p) => sinks.into_iter().find(|s| &s.name == p).or_else(|| list_sinks().into_iter().next()),
            None => sinks.into_iter().next(),
        };
        if let Some(s) = pick {
            sink = Some(s);
            break;
        }
        w.set(VcamStatus::NeedsDriver(
            "No virtual camera driver found. Install the free OBS Studio once — it adds \u{201c}OBS Virtual Camera\u{201d}, which Studio Record drives for you.".into(),
        ));
        std::thread::sleep(Duration::from_millis(1500));
    }
    let Some(sink) = sink else { return };

    let mut queue: *mut c_void = std::ptr::null_mut();
    unsafe {
        let r = CMIOStreamCopyBufferQueue(sink.stream, Some(queue_altered), std::ptr::null_mut(), &mut queue);
        if r != 0 || queue.is_null() {
            w.set(VcamStatus::Error(format!("Couldn't connect to \u{201c}{}\u{201d} (error {r}).", sink.name)));
            return;
        }
        let r = CMIODeviceStartStream(sink.device, sink.stream);
        if r != 0 {
            w.set(VcamStatus::Error(format!("\u{201c}{}\u{201d} refused to start (error {r}).", sink.name)));
            return;
        }
    }
    w.set(VcamStatus::On { target: sink.name.clone() });

    // IOSurface backing lets the picture cross into the camera extension.
    let attrs: Retained<NSDictionary<NSString, AnyObject>> = {
        let empty: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
        let key = NSString::from_str("IOSurfaceProperties");
        let val: &AnyObject = &empty;
        NSDictionary::from_slices(&[&*key], &[val])
    };
    let attrs_ptr = Retained::as_ptr(&attrs) as CFTypeRef;
    let bgra = fourcc(b"BGRA");

    while !w.stopped() {
        let f = match w.rx.recv_timeout(Duration::from_millis(200)) {
            Ok(f) => f,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        unsafe {
            if CMSimpleQueueGetCount(queue) >= CMSimpleQueueGetCapacity(queue) {
                continue; // the consumer is behind; drop this frame
            }
            let (fw, fh) = (f.width as usize, f.height as usize);
            let mut pb: *mut c_void = std::ptr::null_mut();
            if CVPixelBufferCreate(std::ptr::null(), fw, fh, bgra, attrs_ptr, &mut pb) != 0 || pb.is_null() {
                continue;
            }
            CVPixelBufferLockBaseAddress(pb, 0);
            let base = CVPixelBufferGetBaseAddress(pb) as *mut u8;
            let stride = CVPixelBufferGetBytesPerRow(pb);
            for y in 0..fh {
                let src = &f.data[y * fw * 4..(y + 1) * fw * 4];
                let dst = std::slice::from_raw_parts_mut(base.add(y * stride), fw * 4);
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
            CVPixelBufferUnlockBaseAddress(pb, 0);

            let mut fmt: *mut c_void = std::ptr::null_mut();
            if CMVideoFormatDescriptionCreateForImageBuffer(std::ptr::null(), pb, &mut fmt) != 0 {
                CFRelease(pb);
                continue;
            }
            let timing = CMSampleTimingInfo {
                duration: CMTime { value: 1, timescale: w.fps as i32, flags: 1, epoch: 0 },
                pts: CMClockGetTime(CMClockGetHostTimeClock()),
                dts: CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 },
            };
            let mut sb: *mut c_void = std::ptr::null_mut();
            let r = CMSampleBufferCreateForImageBuffer(
                std::ptr::null(),
                pb,
                1,
                std::ptr::null(),
                std::ptr::null(),
                fmt,
                &timing,
                &mut sb,
            );
            CFRelease(fmt);
            CFRelease(pb);
            if r == 0 && !sb.is_null() && CMSimpleQueueEnqueue(queue, sb) != 0 {
                CFRelease(sb);
            }
        }
    }
    unsafe {
        CMIODeviceStopStream(sink.device, sink.stream);
    }
}
