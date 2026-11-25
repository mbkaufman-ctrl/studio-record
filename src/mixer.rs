//! The program mixer: turns the current scene plus the newest frame from
//! every source into one picture, at a steady frame rate, and hands that
//! picture to the preview, the recorder and the virtual camera.

use crate::frame::{Frame, FrameRef, FrameSlot};
use crossbeam_channel::Sender;
use fast_image_resize::{images::Image, images::ImageRef, FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Camera,
    Screen,
    Tablet,
    /// A web page (slides) from the Link bar.
    Link,
}

impl Role {
    /// Roles backed by a capture device (Link is fed by the web page).
    pub const DEVICES: [Role; 3] = [Role::Camera, Role::Screen, Role::Tablet];
    pub fn label(self) -> &'static str {
        match self {
            Role::Camera => "Camera",
            Role::Screen => "Screen or window",
            Role::Tablet => "iPad",
            Role::Link => "Link",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Role::Camera => "Camera",
            Role::Screen => "Screen",
            Role::Tablet => "iPad",
            Role::Link => "Link",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Layout {
    Full(Role),
    /// `main` fills the frame, `pip` floats in a corner.
    Pip { main: Role, pip: Role },
    /// `left` takes the larger share, `right` sits beside it.
    Split { left: Role, right: Role },
}

impl Layout {
    pub fn roles(&self) -> Vec<Role> {
        match *self {
            Layout::Full(r) => vec![r],
            Layout::Pip { main, pip } => vec![main, pip],
            Layout::Split { left, right } => vec![left, right],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub layout: Layout,
}

pub const SCENE_SET: u32 = 2;

pub fn default_scenes() -> Vec<Scene> {
    vec![
        Scene { name: "Face".into(), layout: Layout::Full(Role::Camera) },
        Scene { name: "Screen + Face".into(), layout: Layout::Pip { main: Role::Screen, pip: Role::Camera } },
        Scene { name: "Link + Face".into(), layout: Layout::Pip { main: Role::Link, pip: Role::Camera } },
        Scene { name: "iPad + Face".into(), layout: Layout::Pip { main: Role::Tablet, pip: Role::Camera } },
        Scene { name: "Screen".into(), layout: Layout::Full(Role::Screen) },
        Scene { name: "Link".into(), layout: Layout::Full(Role::Link) },
        Scene { name: "iPad".into(), layout: Layout::Full(Role::Tablet) },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipSize {
    Small,
    Medium,
    Large,
}

impl PipSize {
    fn fraction(self) -> f32 {
        match self {
            PipSize::Small => 0.20,
            PipSize::Medium => 0.27,
            PipSize::Large => 0.36,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipShape {
    Rounded,
    Circle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputSize {
    P720,
    P1080,
}

impl OutputSize {
    pub fn dims(self) -> (u32, u32) {
        match self {
            OutputSize::P720 => (1280, 720),
            OutputSize::P1080 => (1920, 1080),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            OutputSize::P720 => "720p",
            OutputSize::P1080 => "1080p",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transition {
    Cut,
    Fade,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MixSettings {
    pub size: OutputSize,
    pub fps: u32,
    pub pip_corner: Corner,
    pub pip_size: PipSize,
    pub pip_shape: PipShape,
    pub mirror_camera: bool,
    pub transition: Transition,
}

impl Default for MixSettings {
    fn default() -> Self {
        MixSettings {
            size: OutputSize::P1080,
            fps: 30,
            pip_corner: Corner::BottomRight,
            pip_size: PipSize::Medium,
            pip_shape: PipShape::Rounded,
            mirror_camera: false,
            transition: Transition::Fade,
        }
    }
}

pub const FADE: Duration = Duration::from_millis(200);

pub struct MixerState {
    pub settings: MixSettings,
    pub scenes: Vec<Scene>,
    pub active: usize,
    pub from: Option<(usize, Instant)>,
    pub sources: HashMap<Role, FrameSlot>,
    /// Pause: outputs show a still of the last picture with a pause mark.
    pub hold: bool,
}

#[derive(Clone, Copy, Default)]
pub struct MixStats {
    pub fps: f32,
    pub render_ms: f32,
}

#[derive(Clone)]
pub struct Mixer {
    pub state: Arc<Mutex<MixerState>>,
    pub program: FrameSlot,
    pub outputs: Arc<Mutex<Vec<(u64, Sender<FrameRef>)>>>,
    pub stats: Arc<Mutex<MixStats>>,
    /// The paused still, while Pause is on (the preview shows it).
    pub hold_frame: FrameSlot,
    stop: Arc<AtomicBool>,
}

impl Mixer {
    pub fn start(settings: MixSettings, scenes: Vec<Scene>, active: usize) -> Mixer {
        let active = active.min(scenes.len().saturating_sub(1));
        let m = Mixer {
            state: Arc::new(Mutex::new(MixerState { settings, scenes, active, from: None, sources: HashMap::new(), hold: false })),
            program: FrameSlot::default(),
            outputs: Arc::new(Mutex::new(Vec::new())),
            stats: Arc::new(Mutex::new(MixStats::default())),
            hold_frame: FrameSlot::default(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let m2 = m.clone();
        let _ = std::thread::Builder::new().name("mixer".into()).spawn(move || m2.run());
        m
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn switch_to(&self, idx: usize) {
        let mut s = self.state.lock();
        if idx >= s.scenes.len() || idx == s.active {
            return;
        }
        s.from = match s.settings.transition {
            Transition::Fade => Some((s.active, Instant::now())),
            Transition::Cut => None,
        };
        s.active = idx;
    }

    pub fn set_hold(&self, on: bool) {
        self.state.lock().hold = on;
    }

    pub fn hold(&self) -> bool {
        self.state.lock().hold
    }

    pub fn active(&self) -> usize {
        self.state.lock().active
    }

    pub fn set_source(&self, role: Role, slot: Option<FrameSlot>) {
        let mut s = self.state.lock();
        match slot {
            Some(sl) => {
                s.sources.insert(role, sl);
            }
            None => {
                s.sources.remove(&role);
            }
        }
    }

    pub fn add_output(&self, id: u64, tx: Sender<FrameRef>) {
        let mut o = self.outputs.lock();
        o.retain(|(i, _)| *i != id);
        o.push((id, tx));
    }

    pub fn remove_output(&self, id: u64) {
        self.outputs.lock().retain(|(i, _)| *i != id);
    }

    fn run(&self) {
        let mut painter = Painter::default();
        let mut pool: Vec<FrameRef> = Vec::new();
        let mut from_pool: Vec<FrameRef> = Vec::new();
        let mut next = Instant::now();
        let mut ticks: std::collections::VecDeque<Instant> = Default::default();
        let mut held: Option<FrameRef> = None;

        while !self.stop.load(Ordering::SeqCst) {
            let (settings, layout, from_layout, t, slots, hold) = {
                let mut s = self.state.lock();
                let mut t = 1.0f32;
                let mut from_layout = None;
                if let Some((fi, started)) = s.from {
                    let e = started.elapsed();
                    if e >= FADE {
                        s.from = None;
                    } else {
                        t = e.as_secs_f32() / FADE.as_secs_f32();
                        from_layout = s.scenes.get(fi).map(|sc| sc.layout);
                    }
                }
                let layout = s.scenes.get(s.active).map(|sc| sc.layout).unwrap_or(Layout::Full(Role::Camera));
                (s.settings.clone(), layout, from_layout, t, s.sources.clone(), s.hold)
            };
            let period = Duration::from_secs_f64(1.0 / settings.fps.clamp(5, 60) as f64);
            let started = Instant::now();
            let (w, h) = settings.size.dims();

            let frames: HashMap<Role, FrameRef> =
                slots.iter().filter_map(|(r, sl)| sl.get().map(|f| (*r, f))).collect();

            let mut out = take_buffer(&mut pool, w, h);
            if hold {
                // Freeze on the last live picture, dimmed, with a pause mark.
                if held.as_ref().is_none_or(|f| f.width != w || f.height != h) {
                    held = self.program.get().filter(|f| f.width == w && f.height == h);
                }
                let buf = Arc::get_mut(&mut out).expect("unique");
                match &held {
                    Some(src) => buf.data.copy_from_slice(&src.data),
                    None => buf.data.fill(0),
                }
                paint_hold(&mut buf.data, w, h);
                buf.restamp();
                for (_, tx) in self.outputs.lock().iter() {
                    let _ = tx.try_send(out.clone());
                }
                self.hold_frame.put_ref(out.clone());
                pool.push(out);
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
                continue;
            }
            held = None;
            self.hold_frame.clear();
            {
                let buf = Arc::get_mut(&mut out).expect("unique");
                painter.paint(layout, &frames, &settings, &mut buf.data, w, h);
                if let Some(fl) = from_layout {
                    let mut prev = take_buffer(&mut from_pool, w, h);
                    let pb = Arc::get_mut(&mut prev).expect("unique");
                    painter.paint(fl, &frames, &settings, &mut pb.data, w, h);
                    let eased = ease_out(t);
                    blend(&mut buf.data, &pb.data, eased);
                    from_pool.push(prev);
                }
                buf.restamp();
            }

            self.program.put_ref(out.clone());
            for (_, tx) in self.outputs.lock().iter() {
                let _ = tx.try_send(out.clone());
            }
            pool.push(out);

            let now = Instant::now();
            ticks.push_back(now);
            while ticks.front().is_some_and(|f| now.duration_since(*f) > Duration::from_secs(1)) {
                ticks.pop_front();
            }
            {
                let mut st = self.stats.lock();
                st.fps = ticks.len() as f32;
                st.render_ms = st.render_ms * 0.9 + started.elapsed().as_secs_f32() * 1000.0 * 0.1;
            }

            next += period;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > period * 4 {
                next = now; // fell far behind (e.g. laptop slept): don't sprint to catch up
            }
        }
    }
}

/// Dims the picture and draws two rounded pause bars in the middle.
fn paint_hold(buf: &mut [u8], w: u32, h: u32) {
    for px in buf.chunks_exact_mut(4) {
        px[0] = (px[0] as u16 * 38 / 100) as u8;
        px[1] = (px[1] as u16 * 38 / 100) as u8;
        px[2] = (px[2] as u16 * 38 / 100) as u8;
    }
    let (w, h) = (w as f32, h as f32);
    let bh = h * 0.16;
    let bw = bh * 0.28;
    let gap = bw * 0.9;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let r = bw / 2.0;
    for (x0, _) in [(cx - gap / 2.0 - bw, 0), (cx + gap / 2.0, 0)] {
        let (ys, ye) = ((cy - bh / 2.0) as i32, (cy + bh / 2.0) as i32);
        for y in ys..ye {
            for x in (x0 as i32)..((x0 + bw) as i32 + 1) {
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    continue;
                }
                let lx = x as f32 + 0.5 - x0;
                let ly = y as f32 + 0.5 - (cy - bh / 2.0);
                let ccx = lx.clamp(r, bw - r);
                let ccy = ly.clamp(r, bh - r);
                let d = ((lx - ccx).powi(2) + (ly - ccy).powi(2)).sqrt();
                let a = (r - d + 0.5).clamp(0.0, 1.0) * 0.92;
                if a <= 0.0 {
                    continue;
                }
                let i = ((y as u32 * w as u32 + x as u32) * 4) as usize;
                for c in 0..3 {
                    buf[i + c] = (buf[i + c] as f32 * (1.0 - a) + 240.0 * a) as u8;
                }
            }
        }
    }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Reuse a frame buffer nobody else is holding, or make a new one.
fn take_buffer(pool: &mut Vec<FrameRef>, w: u32, h: u32) -> FrameRef {
    if let Some(i) = pool.iter().position(|f| Arc::strong_count(f) == 1 && f.width == w && f.height == h) {
        return pool.swap_remove(i);
    }
    pool.retain(|f| Arc::strong_count(f) > 1 || (f.width == w && f.height == h));
    if pool.len() > 8 {
        pool.remove(0);
    }
    Arc::new(Frame::new(w, h, vec![0u8; (w * h * 4) as usize]))
}

impl Frame {
    pub fn restamp(&mut self) {
        let fresh = Frame::new(1, 1, vec![0; 4]);
        self.seq = fresh.seq;
        self.captured = fresh.captured;
    }
}

/// `dst = dst * t + src * (1 - t)`
fn blend(dst: &mut [u8], src: &[u8], t: f32) {
    let a = (t * 256.0) as u32;
    let b = 256 - a;
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d = ((*d as u32 * a + *s as u32 * b) >> 8) as u8;
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Clone, Copy, PartialEq)]
enum Fit {
    Contain,
    Cover,
}

#[derive(Clone, Copy)]
enum Mask {
    None,
    Rounded(f32),
    Circle,
}

pub const BACKGROUND: [u8; 3] = [16, 16, 18];

#[derive(Default)]
struct Painter {
    resizer: Resizer,
    scratch: Vec<u8>,
}

impl Painter {
    fn paint(&mut self, layout: Layout, frames: &HashMap<Role, FrameRef>, s: &MixSettings, out: &mut [u8], w: u32, h: u32) {
        for px in out.chunks_exact_mut(4) {
            px[0] = BACKGROUND[0];
            px[1] = BACKGROUND[1];
            px[2] = BACKGROUND[2];
            px[3] = 255;
        }
        let (wi, hi) = (w as i32, h as i32);
        let fit_for = |r: Role| if r == Role::Camera { Fit::Cover } else { Fit::Contain };
        let _ = Role::Link;
        let mirror_for = |r: Role| r == Role::Camera && s.mirror_camera;
        match layout {
            Layout::Full(r) => {
                if let Some(f) = frames.get(&r) {
                    self.draw(out, w, h, f, Rect { x: 0, y: 0, w: wi, h: hi }, fit_for(r), mirror_for(r), Mask::None);
                }
            }
            Layout::Pip { main, pip } => {
                let main_frame = frames.get(&main);
                if let Some(f) = main_frame {
                    self.draw(out, w, h, f, Rect { x: 0, y: 0, w: wi, h: hi }, fit_for(main), mirror_for(main), Mask::None);
                }
                if let Some(f) = frames.get(&pip) {
                    let margin = (wi as f32 * 0.022) as i32;
                    let pw = (wi as f32 * s.pip_size.fraction()) as i32;
                    let ph = match s.pip_shape {
                        PipShape::Rounded => pw * 9 / 16,
                        PipShape::Circle => pw * 3 / 4,
                    };
                    let pw = if s.pip_shape == PipShape::Circle { ph } else { pw };
                    let (x, y) = match s.pip_corner {
                        Corner::TopLeft => (margin, margin),
                        Corner::TopRight => (wi - pw - margin, margin),
                        Corner::BottomLeft => (margin, hi - ph - margin),
                        Corner::BottomRight => (wi - pw - margin, hi - ph - margin),
                    };
                    let mask = match s.pip_shape {
                        PipShape::Rounded => Mask::Rounded(ph as f32 * 0.09),
                        PipShape::Circle => Mask::Circle,
                    };
                    let r = Rect { x, y, w: pw, h: ph };
                    shadow(out, w, h, r, mask);
                    self.draw(out, w, h, f, r, Fit::Cover, mirror_for(pip), mask);
                }
            }
            Layout::Split { left, right } => {
                let pad = (wi as f32 * 0.025) as i32;
                let lw = ((wi - pad * 3) as f32 * 0.64) as i32;
                let rw = wi - pad * 3 - lw;
                let lr = Rect { x: pad, y: pad, w: lw, h: hi - pad * 2 };
                if let Some(f) = frames.get(&left) {
                    let fit = fit_for(left);
                    let r = if fit == Fit::Contain { contain_rect(f, lr) } else { lr };
                    shadow(out, w, h, r, Mask::Rounded(pad as f32 * 0.6));
                    self.draw(out, w, h, f, r, fit, mirror_for(left), Mask::Rounded(pad as f32 * 0.6));
                }
                if let Some(f) = frames.get(&right) {
                    // A portrait-ish card for the face, vertically centred.
                    let rh = ((rw as f32) * 1.25).min((hi - pad * 2) as f32) as i32;
                    let rr = Rect { x: pad * 2 + lw, y: (hi - rh) / 2, w: rw, h: rh };
                    shadow(out, w, h, rr, Mask::Rounded(pad as f32 * 0.6));
                    self.draw(out, w, h, f, rr, fit_for(right), mirror_for(right), Mask::Rounded(pad as f32 * 0.6));
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, out: &mut [u8], ow: u32, oh: u32, f: &Frame, rect: Rect, fit: Fit, mirror: bool, mask: Mask) {
        if f.width == 0 || f.height == 0 || rect.w <= 0 || rect.h <= 0 {
            return;
        }
        let (sw, sh) = (f.width as f64, f.height as f64);
        let (rw, rh) = (rect.w as f64, rect.h as f64);
        // Work out the destination box and the source crop.
        let (dst, crop) = match fit {
            Fit::Contain => {
                let sc = (rw / sw).min(rh / sh);
                let dw = (sw * sc).round().max(1.0) as i32;
                let dh = (sh * sc).round().max(1.0) as i32;
                (Rect { x: rect.x + (rect.w - dw) / 2, y: rect.y + (rect.h - dh) / 2, w: dw, h: dh }, (0.0, 0.0, sw, sh))
            }
            Fit::Cover => {
                let sc = (rw / sw).max(rh / sh);
                let cw = rw / sc;
                let ch = rh / sc;
                (rect, ((sw - cw) / 2.0, (sh - ch) / 2.0, cw, ch))
            }
        };
        let (dw, dh) = (dst.w as u32, dst.h as u32);
        let need = (dw * dh * 4) as usize;

        let pixels: &[u8] = if dw == f.width && dh == f.height {
            &f.data
        } else {
            self.scratch.resize(need, 0);
            let Ok(src) = ImageRef::new(f.width, f.height, &f.data, PixelType::U8x4) else { return };
            let Ok(mut dimg) = Image::from_slice_u8(dw, dh, &mut self.scratch, PixelType::U8x4) else { return };
            let opts = ResizeOptions::new()
                .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
                .use_alpha(false)
                .crop(crop.0, crop.1, crop.2, crop.3);
            if self.resizer.resize(&src, &mut dimg, &opts).is_err() {
                return;
            }
            &self.scratch
        };

        let (ow, oh) = (ow as i32, oh as i32);
        for y in 0..dst.h {
            let oy = dst.y + y;
            if oy < 0 || oy >= oh {
                continue;
            }
            let x0 = dst.x.max(0);
            let x1 = (dst.x + dst.w).min(ow);
            if x1 <= x0 {
                continue;
            }
            let row = &pixels[(y * dst.w * 4) as usize..((y + 1) * dst.w * 4) as usize];
            let orow = &mut out[((oy * ow + x0) * 4) as usize..((oy * ow + x1) * 4) as usize];
            match mask {
                Mask::None if !mirror => {
                    let sx = (x0 - dst.x) as usize * 4;
                    orow.copy_from_slice(&row[sx..sx + orow.len()]);
                }
                _ => {
                    for x in x0..x1 {
                        let lx = x - dst.x;
                        let cov = coverage(mask, lx as f32 + 0.5, y as f32 + 0.5, dst.w as f32, dst.h as f32);
                        if cov <= 0.0 {
                            continue;
                        }
                        let sx = if mirror { dst.w - 1 - lx } else { lx } as usize * 4;
                        let o = ((x - x0) * 4) as usize;
                        if cov >= 1.0 {
                            orow[o..o + 3].copy_from_slice(&row[sx..sx + 3]);
                        } else {
                            for c in 0..3 {
                                orow[o + c] = (orow[o + c] as f32 * (1.0 - cov) + row[sx + c] as f32 * cov) as u8;
                            }
                        }
                    }
                }
            }
        }
    }
}

fn contain_rect(f: &Frame, r: Rect) -> Rect {
    let sc = (r.w as f32 / f.width as f32).min(r.h as f32 / f.height as f32);
    let dw = (f.width as f32 * sc) as i32;
    let dh = (f.height as f32 * sc) as i32;
    Rect { x: r.x + (r.w - dw) / 2, y: r.y + (r.h - dh) / 2, w: dw, h: dh }
}

/// Anti-aliased coverage (0..1) of a pixel centre inside the mask shape.
fn coverage(mask: Mask, x: f32, y: f32, w: f32, h: f32) -> f32 {
    match mask {
        Mask::None => 1.0,
        Mask::Circle => {
            let r = w.min(h) / 2.0;
            let d = ((x - w / 2.0).powi(2) + (y - h / 2.0).powi(2)).sqrt();
            (r - d + 0.5).clamp(0.0, 1.0)
        }
        Mask::Rounded(r) => {
            let cx = x.clamp(r, w - r);
            let cy = y.clamp(r, h - r);
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            (r - d + 0.5).clamp(0.0, 1.0)
        }
    }
}

/// A soft, low shadow under floating cards so they read against any screen.
fn shadow(out: &mut [u8], ow: u32, oh: u32, r: Rect, mask: Mask) {
    let spread = (r.w.min(r.h) as f32 * 0.08).max(6.0);
    let (ow, oh) = (ow as i32, oh as i32);
    let s = spread as i32;
    let dy = (spread * 0.35) as i32;
    for y in (r.y - s + dy).max(0)..(r.y + r.h + s + dy).min(oh) {
        for x in (r.x - s).max(0)..(r.x + r.w + s).min(ow) {
            let lx = (x - r.x) as f32 + 0.5;
            let ly = (y - r.y - dy) as f32 + 0.5;
            // distance outside the shape
            let d = match mask {
                Mask::Circle => {
                    let rad = r.w.min(r.h) as f32 / 2.0;
                    (((lx - r.w as f32 / 2.0).powi(2) + (ly - r.h as f32 / 2.0).powi(2)).sqrt() - rad).max(0.0)
                }
                Mask::Rounded(rad) => {
                    let cx = lx.clamp(rad, r.w as f32 - rad);
                    let cy = ly.clamp(rad, r.h as f32 - rad);
                    (((lx - cx).powi(2) + (ly - cy).powi(2)).sqrt() - rad).max(0.0)
                }
                Mask::None => 0.0,
            };
            let a = (1.0 - d / spread).clamp(0.0, 1.0);
            let a = a * a * 0.45;
            if a <= 0.0 {
                continue;
            }
            let i = ((y * ow + x) * 4) as usize;
            for c in 0..3 {
                out[i + c] = (out[i + c] as f32 * (1.0 - a)) as u8;
            }
        }
    }
}

/// Where each source lands in a layout, as fractions of the output
/// (x, y, w, h) — used to draw the scene cards and the preview overlays.
pub fn layout_boxes(layout: Layout, s: &MixSettings) -> Vec<(Role, [f32; 4], bool)> {
    let (w, h) = (1600.0f32, 900.0f32);
    let n = |x: f32, y: f32, bw: f32, bh: f32| [x / w, y / h, bw / w, bh / h];
    match layout {
        Layout::Full(r) => vec![(r, [0.0, 0.0, 1.0, 1.0], false)],
        Layout::Pip { main, pip } => {
            let margin = w * 0.022;
            let pw = w * s.pip_size.fraction();
            let (pw, ph) = match s.pip_shape {
                PipShape::Rounded => (pw, pw * 9.0 / 16.0),
                PipShape::Circle => (pw * 0.75, pw * 0.75),
            };
            let (x, y) = match s.pip_corner {
                Corner::TopLeft => (margin, margin),
                Corner::TopRight => (w - pw - margin, margin),
                Corner::BottomLeft => (margin, h - ph - margin),
                Corner::BottomRight => (w - pw - margin, h - ph - margin),
            };
            vec![(main, [0.0, 0.0, 1.0, 1.0], false), (pip, n(x, y, pw, ph), s.pip_shape == PipShape::Circle)]
        }
        Layout::Split { left, right } => {
            let pad = w * 0.025;
            let lw = (w - pad * 3.0) * 0.64;
            let rw = w - pad * 3.0 - lw;
            let rh = (rw * 1.25).min(h - pad * 2.0);
            vec![
                (left, n(pad, pad, lw, h - pad * 2.0), false),
                (right, n(pad * 2.0 + lw, (h - rh) / 2.0, rw, rh), false),
            ]
        }
    }
}
