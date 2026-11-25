//! Windows: write frames into the shared-memory queue read by the OBS
//! Virtual Camera DirectShow filter (installed with OBS Studio). Teams,
//! Meet in Chrome/Edge and Zoom list it as "OBS Virtual Camera".

use super::{rgba_to_nv12, VcamStatus, Worker};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

#[repr(C)]
struct QueueHeader {
    write_idx: u32,
    read_idx: u32,
    state: u32,
    offsets: [u32; 3],
    kind: u32,
    cx: u32,
    cy: u32,
    interval: u64,
    reserved: [u32; 8],
}

const STATE_STARTING: u32 = 1;
const STATE_READY: u32 = 2;
const STATE_STOPPING: u32 = 3;
const FRAME_HEADER: usize = 32;

fn align32(v: usize) -> usize {
    (v + 31) & !31
}

pub fn run(w: Worker) {
    let (cx, cy) = (w.width as usize, w.height as usize);
    let frame_size = cx * cy * 3 / 2;
    let mut size = align32(std::mem::size_of::<QueueHeader>());
    let mut offsets = [0u32; 3];
    for o in offsets.iter_mut() {
        *o = size as u32;
        size = align32(size + frame_size + FRAME_HEADER);
    }
    let name: Vec<u16> = "OBSVirtualCamVideo".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, size as u32, name.as_ptr());
        if h.is_null() {
            w.set(VcamStatus::Error("Couldn't open the virtual camera channel.".into()));
            return;
        }
        let view: MEMORY_MAPPED_VIEW_ADDRESS = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, 0);
        let base = view.Value as *mut u8;
        if base.is_null() {
            CloseHandle(h);
            w.set(VcamStatus::Error("Couldn't map the virtual camera channel.".into()));
            return;
        }
        let header = base as *mut QueueHeader;
        std::ptr::write(
            header,
            QueueHeader {
                write_idx: 0,
                read_idx: 0,
                state: STATE_STARTING,
                offsets,
                kind: 0,
                cx: cx as u32,
                cy: cy as u32,
                interval: 10_000_000 / w.fps.max(1) as u64,
                reserved: [0; 8],
            },
        );
        w.set(VcamStatus::On { target: "OBS Virtual Camera".into() });
        let epoch = Instant::now();
        let mut nv12 = vec![0u8; frame_size];
        let mut counter: u32 = 0;
        while !w.stopped() {
            let f = match w.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(f) => f,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            if f.width as usize != cx || f.height as usize != cy {
                continue;
            }
            rgba_to_nv12(&f.data, cx, cy, &mut nv12);
            counter = counter.wrapping_add(1);
            let idx = (counter % 3) as usize;
            let off = offsets[idx] as usize;
            let ts = (epoch.elapsed().as_nanos() / 100) as u64;
            std::ptr::write_unaligned(base.add(off) as *mut u64, ts);
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), base.add(off + FRAME_HEADER), frame_size);
            std::ptr::write_volatile(&mut (*header).write_idx, counter);
            std::ptr::write_volatile(&mut (*header).read_idx, counter);
            std::ptr::write_volatile(&mut (*header).state, STATE_READY);
        }
        std::ptr::write_volatile(&mut (*header).state, STATE_STOPPING);
        UnmapViewOfFile(view);
        CloseHandle(h);
    }
}
