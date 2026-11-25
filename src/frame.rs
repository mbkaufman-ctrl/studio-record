//! Video frames shared between capture threads, the mixer, the preview,
//! the recorder and the virtual camera.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// A tightly packed RGBA8 picture.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Monotonic counter, so consumers can tell a new frame from an old one.
    pub seq: u64,
    pub captured: Instant,
}

pub type FrameRef = Arc<Frame>;

static SEQ: AtomicU64 = AtomicU64::new(1);

impl Frame {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        debug_assert_eq!(data.len(), (width * height * 4) as usize);
        Frame { width, height, data, seq: SEQ.fetch_add(1, Ordering::Relaxed), captured: Instant::now() }
    }

    pub fn from_bgra_rows(width: usize, height: usize, stride: usize, src: &[u8]) -> Self {
        let mut data = vec![0u8; width * height * 4];
        for y in 0..height {
            let row = &src[y * stride..y * stride + width * 4];
            let out = &mut data[y * width * 4..(y + 1) * width * 4];
            for (o, i) in out.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                o[0] = i[2];
                o[1] = i[1];
                o[2] = i[0];
                o[3] = 255;
            }
        }
        Frame::new(width as u32, height as u32, data)
    }
}

/// "Latest frame wins" mailbox. Writers never wait for readers.
#[derive(Clone, Default)]
pub struct FrameSlot(Arc<Mutex<Option<FrameRef>>>);

impl FrameSlot {
    pub fn put(&self, f: Frame) {
        *self.0.lock() = Some(Arc::new(f));
    }
    pub fn put_ref(&self, f: FrameRef) {
        *self.0.lock() = Some(f);
    }
    pub fn get(&self) -> Option<FrameRef> {
        self.0.lock().clone()
    }
    pub fn clear(&self) {
        *self.0.lock() = None;
    }
}

/// Rolling frames-per-second estimate for a source.
#[derive(Default)]
pub struct FpsMeter {
    stamps: std::collections::VecDeque<Instant>,
}

impl FpsMeter {
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.stamps.push_back(now);
        while let Some(f) = self.stamps.front() {
            if now.duration_since(*f).as_secs_f32() > 1.0 {
                self.stamps.pop_front();
            } else {
                break;
            }
        }
    }
    pub fn fps(&self) -> f32 {
        self.stamps.len() as f32
    }
}
