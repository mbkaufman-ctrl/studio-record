//! "Audio for calls": plays your microphone (respecting Mute) into a
//! virtual audio device, so a call app can use Studio Record as its
//! microphone too — picture and voice from one place, one Mute button.
//!
//! Virtual devices: BlackHole (macOS), VB-CABLE (Windows), a PulseAudio /
//! PipeWire null sink (Linux). Any output device can be chosen.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

const VIRTUAL_HINTS: [&str; 8] = ["blackhole", "vb-audio", "cable input", "loopback", "soundflower", "null", "virtual", "studio record"];

pub fn is_virtual_name(n: &str) -> bool {
    let l = n.to_lowercase();
    VIRTUAL_HINTS.iter().any(|h| l.contains(h))
}

/// Output devices, virtual ones first.
pub fn list_outputs() -> Vec<String> {
    let host = cpal::default_host();
    let mut out: Vec<String> = Vec::new();
    if let Ok(devs) = host.output_devices() {
        for d in devs {
            if let Ok(desc) = d.description() {
                let n = desc.name().to_string();
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    out.sort_by_key(|n| !is_virtual_name(n));
    out
}

#[derive(Clone, Debug, PartialEq)]
pub enum BridgeStatus {
    Starting,
    On(String),
    Error(String),
}

pub struct AudioBridge {
    stop: Arc<AtomicBool>,
    pub status: Arc<Mutex<BridgeStatus>>,
    pub level: Arc<AtomicU32>,
}

impl AudioBridge {
    pub fn start(mic: Option<String>, output: String, muted: Arc<AtomicBool>) -> AudioBridge {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(BridgeStatus::Starting));
        let level = Arc::new(AtomicU32::new(0));
        let (s2, st2, l2) = (stop.clone(), status.clone(), level.clone());
        let _ = std::thread::Builder::new().name("audio bridge".into()).spawn(move || {
            if let Err(e) = run(mic, &output, muted, &s2, &st2, l2) {
                *st2.lock() = BridgeStatus::Error(e);
            }
        });
        AudioBridge { stop, status, level }
    }

    pub fn status(&self) -> BridgeStatus {
        self.status.lock().clone()
    }
}

impl Drop for AudioBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn find(host: &cpal::Host, name: &str, input: bool) -> Option<cpal::Device> {
    let list = if input { host.input_devices().ok()?.collect::<Vec<_>>() } else { host.output_devices().ok()?.collect::<Vec<_>>() };
    list.into_iter().find(|d| d.description().map(|x| x.name() == name).unwrap_or(false))
}

fn run(mic: Option<String>, output: &str, muted: Arc<AtomicBool>, stop: &AtomicBool, status: &Mutex<BridgeStatus>, level: Arc<AtomicU32>) -> Result<(), String> {
    let host = cpal::default_host();
    let input = match &mic {
        Some(n) => find(&host, n, true).or_else(|| host.default_input_device()),
        None => host.default_input_device(),
    }
    .ok_or("No microphone found.")?;
    let out_dev = find(&host, output, false).ok_or_else(|| format!("\u{201c}{output}\u{201d} isn't connected."))?;
    let in_cfg = input.default_input_config().map_err(|e| e.to_string())?;
    let out_cfg = out_dev.default_output_config().map_err(|e| e.to_string())?;
    let in_ch = in_cfg.channels() as usize;
    let out_ch = out_cfg.channels() as usize;
    let ratio = out_cfg.sample_rate() as f64 / in_cfg.sample_rate() as f64;

    // Mono ring buffer at the *output* rate; kept short for low latency.
    let buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(48000)));
    let max_len = (out_cfg.sample_rate() as f64 * 0.12) as usize;
    let b_in = buf.clone();
    let m_in = muted.clone();
    let mut phase = 0.0f64;
    let mut last = 0.0f32;
    let mut push = move |frames: &mut dyn Iterator<Item = f32>| {
        let mut q = b_in.lock();
        let mut peak = 0f32;
        let mut acc = 0f32;
        let mut n = 0usize;
        for s in frames {
            acc += s;
            n += 1;
            if n < in_ch {
                continue;
            }
            let mono = if m_in.load(Ordering::Relaxed) { 0.0 } else { acc / in_ch as f32 };
            acc = 0.0;
            n = 0;
            peak = peak.max(mono.abs());
            // Linear resampling to the output rate.
            phase += ratio;
            while phase >= 1.0 {
                let t = (phase - 1.0) as f32;
                q.push_back(last + (mono - last) * (1.0 - t).clamp(0.0, 1.0));
                phase -= 1.0;
            }
            last = mono;
        }
        while q.len() > max_len {
            q.pop_front();
        }
        let prev = f32::from_bits(level.load(Ordering::Relaxed));
        level.store((if peak > prev { peak } else { prev * 0.9 }).to_bits(), Ordering::Relaxed);
    };
    let err = |e: cpal::Error| log::warn!("audio bridge: {e}");
    let in_stream = match in_cfg.sample_format() {
        cpal::SampleFormat::F32 => input.build_input_stream::<f32, _, _>(in_cfg.config(), move |d: &[f32], _| push(&mut d.iter().copied()), err, None),
        cpal::SampleFormat::I16 => {
            input.build_input_stream::<i16, _, _>(in_cfg.config(), move |d: &[i16], _| push(&mut d.iter().map(|s| *s as f32 / 32768.0)), err, None)
        }
        cpal::SampleFormat::I32 => input.build_input_stream::<i32, _, _>(
            in_cfg.config(),
            move |d: &[i32], _| push(&mut d.iter().map(|s| *s as f32 / 2147483648.0)),
            err,
            None,
        ),
        f => return Err(format!("Unsupported microphone format {f:?}")),
    }
    .map_err(|e| e.to_string())?;

    let b_out = buf.clone();
    let pull = move |out: &mut dyn FnMut(usize, f32), frames: usize| {
        let mut q = b_out.lock();
        for i in 0..frames {
            let v = q.pop_front().unwrap_or(0.0);
            for c in 0..out_ch {
                out(i * out_ch + c, v);
            }
        }
    };
    let out_stream = match out_cfg.sample_format() {
        cpal::SampleFormat::F32 => out_dev.build_output_stream::<f32, _, _>(
            out_cfg.config(),
            move |d: &mut [f32], _| {
                let n = d.len() / out_ch.max(1);
                pull(&mut |i, v| d[i] = v, n);
            },
            err,
            None,
        ),
        cpal::SampleFormat::I16 => out_dev.build_output_stream::<i16, _, _>(
            out_cfg.config(),
            move |d: &mut [i16], _| {
                let n = d.len() / out_ch.max(1);
                pull(&mut |i, v| d[i] = (v.clamp(-1.0, 1.0) * 32767.0) as i16, n);
            },
            err,
            None,
        ),
        f => return Err(format!("Unsupported output format {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    in_stream.play().map_err(|e| e.to_string())?;
    out_stream.play().map_err(|e| e.to_string())?;
    *status.lock() = BridgeStatus::On(output.to_string());
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
