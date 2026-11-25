//! Independent microphone and screen audio inputs for the recorder.

use crossbeam_channel::{bounded, Sender};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SCREEN_RATE: u32 = 48_000;
pub const SCREEN_CHANNELS: u16 = 2;
pub type PauseTime = Arc<Mutex<(Duration, Option<Instant>)>>;
type Writer = hound::WavWriter<std::io::BufWriter<std::fs::File>>;

#[derive(Clone)]
pub enum InputState {
    Waiting,
    Receiving,
    Error(String),
}

pub struct InputTrack {
    gain: AtomicU8,
    muted: Arc<AtomicBool>,
    level: AtomicU32,
    state: Mutex<InputState>,
}

impl InputTrack {
    pub fn new(volume: u8, muted: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            gain: AtomicU8::new(volume.min(100)),
            muted,
            level: AtomicU32::new(0),
            state: Mutex::new(InputState::Waiting),
        })
    }

    pub fn set_volume(&self, volume: u8) {
        self.gain.store(volume.min(100), Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Relaxed);
        if muted {
            self.level.store(0, Relaxed);
        }
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Relaxed))
    }

    pub fn state(&self) -> InputState {
        self.state.lock().clone()
    }

    fn fail(&self, message: String) {
        log::warn!("recording audio: {message}");
        self.level.store(0, Relaxed);
        *self.state.lock() = InputState::Error(message);
    }

    fn write(&self, writer: &mut Writer, samples: impl Iterator<Item = f32>) -> Result<(), String> {
        let gain = if self.muted.load(Relaxed) {
            0.0
        } else {
            self.gain.load(Relaxed) as f32 / 100.0
        };
        let mut peak: f32 = 0.0;
        for sample in samples {
            let sample = if sample.is_finite() {
                (sample * gain).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            writer
                .write_sample(sample)
                .map_err(|error| error.to_string())?;
            peak = peak.max(sample.abs());
        }
        self.level
            .store(peak.max(self.level() * 0.9).to_bits(), Relaxed);
        let mut state = self.state.lock();
        if matches!(*state, InputState::Waiting) {
            *state = InputState::Receiving;
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct ScreenAudioFeed(Arc<Mutex<Option<Sender<ScreenSamples>>>>);

impl ScreenAudioFeed {
    pub fn sender(&self) -> Option<Sender<ScreenSamples>> {
        self.0.lock().clone()
    }
}

pub struct ScreenSamples {
    pub received_at: Instant,
    pub interleaved: Vec<f32>,
}

pub struct ScreenAudioRequest {
    pub feed: ScreenAudioFeed,
    pub volume: u8,
    pub muted: bool,
}

pub struct TrackFile {
    pub path: PathBuf,
    pub offset: Duration,
}

pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<Option<TrackFile>, String>>>,
    track: Arc<InputTrack>,
}

impl AudioCapture {
    fn spawn(
        name: &str,
        track: Arc<InputTrack>,
        capture: impl FnOnce(Arc<AtomicBool>) -> Result<Option<TrackFile>, String> + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_capture = stop.clone();
        let report = track.clone();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let result = capture(stop_capture);
                if let Err(error) = &result {
                    report.fail(error.clone());
                }
                result
            });
        if let Err(error) = &thread {
            track.fail(error.to_string());
        }
        Self {
            stop,
            thread: thread.ok(),
            track,
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Relaxed);
    }

    pub fn finish(mut self) -> Option<TrackFile> {
        self.stop();
        match self.thread.take()?.join() {
            Ok(Ok(Some(file))) => Some(file),
            Ok(Ok(None)) => {
                if !matches!(self.track.state(), InputState::Error(_)) {
                    self.track.fail("No audio samples were received.".into());
                }
                None
            }
            Ok(Err(_)) => None,
            Err(_) => {
                self.track
                    .fail("Audio capture stopped unexpectedly.".into());
                None
            }
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn wav_writer(path: &Path, channels: u16, sample_rate: u32) -> Result<Writer, String> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    hound::WavWriter::create(path, spec).map_err(|error| error.to_string())
}

fn record_position(t0: Instant, at: Instant, pauses: &PauseTime) -> Duration {
    at.saturating_duration_since(t0)
        .saturating_sub(pauses.lock().0)
}

pub fn microphone(
    mic: Option<String>,
    path: PathBuf,
    t0: Instant,
    paused: Arc<AtomicBool>,
    pauses: PauseTime,
    track: Arc<InputTrack>,
) -> AudioCapture {
    AudioCapture::spawn("recording microphone", track.clone(), move |stop| {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::Sample;

        let host = cpal::default_host();
        let device = match mic {
            Some(name) => host.input_devices().ok().and_then(|mut devices| {
                devices.find(|device| device.description().is_ok_and(|d| d.name() == name))
            }),
            None => host.default_input_device(),
        }
        .ok_or("No microphone found.")?;
        let config = device
            .default_input_config()
            .map_err(|error| error.to_string())?;
        let channels = config.channels();
        let rate = config.sample_rate();
        let writer = Arc::new(Mutex::new(Some(wav_writer(&path, channels, rate)?)));
        let first = Arc::new(Mutex::new(None));
        let (sink, first_sample, report) = (writer.clone(), first.clone(), track.clone());
        let stop_on_write_error = stop.clone();
        let push = move |samples: &mut dyn ExactSizeIterator<Item = f32>| {
            if paused.load(Relaxed) || stop_on_write_error.load(Relaxed) || samples.len() == 0 {
                return;
            }
            let buffer_length =
                Duration::from_secs_f64(samples.len() as f64 / (channels as f64 * rate as f64));
            let start = Instant::now().checked_sub(buffer_length).unwrap_or(t0);
            first_sample
                .lock()
                .get_or_insert_with(|| record_position(t0, start, &pauses));
            if let Some(writer) = sink.lock().as_mut() {
                if let Err(error) = report.write(writer, samples) {
                    report.fail(error);
                    stop_on_write_error.store(true, Relaxed);
                }
            }
        };
        let stop_on_input_error = stop.clone();
        let on_error = move |error: cpal::Error| {
            track.fail(error.to_string());
            stop_on_input_error.store(true, Relaxed);
        };
        macro_rules! input {
            ($sample:ty) => {
                device.build_input_stream::<$sample, _, _>(
                    config.config(),
                    move |data: &[$sample], _| {
                        push(&mut data.iter().copied().map(f32::from_sample))
                    },
                    on_error,
                    None,
                )
            };
        }
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => input!(f32),
            cpal::SampleFormat::F64 => input!(f64),
            cpal::SampleFormat::I16 => input!(i16),
            cpal::SampleFormat::I24 => input!(cpal::I24),
            cpal::SampleFormat::I32 => input!(i32),
            cpal::SampleFormat::U16 => input!(u16),
            cpal::SampleFormat::U32 => input!(u32),
            format => return Err(format!("Unsupported microphone format {format:?}")),
        }
        .map_err(|error| error.to_string())?;
        stream.play().map_err(|error| error.to_string())?;
        while !stop.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(stream);
        if let Some(writer) = writer.lock().take() {
            writer.finalize().map_err(|error| error.to_string())?;
        }
        let offset = *first.lock();
        if offset.is_none() {
            let _ = std::fs::remove_file(&path);
        }
        Ok(offset.map(|offset| TrackFile { path, offset }))
    })
}

pub fn screen(
    feed: ScreenAudioFeed,
    path: PathBuf,
    t0: Instant,
    paused: Arc<AtomicBool>,
    pauses: PauseTime,
    track: Arc<InputTrack>,
) -> AudioCapture {
    AudioCapture::spawn("recording screen audio", track.clone(), move |stop| {
        let mut writer = wav_writer(&path, SCREEN_CHANNELS, SCREEN_RATE)?;
        let (sender, receiver) = bounded::<ScreenSamples>(32);
        *feed.0.lock() = Some(sender.clone());
        let mut first = None;
        let mut frames_written = 0u64;
        let result = (|| {
            while !stop.load(Relaxed) {
                let Ok(packet) = receiver.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                if paused.load(Relaxed) || packet.interleaved.is_empty() {
                    continue;
                }
                let channels = SCREEN_CHANNELS as usize;
                let frames = packet.interleaved.len() / channels;
                let buffer_length = Duration::from_secs_f64(frames as f64 / SCREEN_RATE as f64);
                let start = packet.received_at.checked_sub(buffer_length).unwrap_or(t0);
                let position = record_position(t0, start, &pauses);
                let offset = *first.get_or_insert(position);
                let expected_frame = (position.saturating_sub(offset).as_secs_f64()
                    * SCREEN_RATE as f64)
                    .round() as u64;
                // A quiet or disconnected source may stop delivering buffers.
                // Replace real gaps with silence without chasing callback jitter.
                if expected_frame > frames_written + SCREEN_RATE as u64 / 10 {
                    for _ in 0..(expected_frame - frames_written) * channels as u64 {
                        writer
                            .write_sample(0.0f32)
                            .map_err(|error| error.to_string())?;
                    }
                    frames_written = expected_frame;
                }
                track.write(&mut writer, packet.interleaved.into_iter())?;
                frames_written += frames as u64;
            }
            Ok::<_, String>(())
        })();
        let mut route = feed.0.lock();
        if route
            .as_ref()
            .is_some_and(|active| active.same_channel(&sender))
        {
            *route = None;
        }
        drop(route);
        writer.finalize().map_err(|error| error.to_string())?;
        result?;
        if first.is_none() {
            let _ = std::fs::remove_file(&path);
        }
        Ok(first.map(|offset| TrackFile { path, offset }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_controls_affect_only_the_selected_input() {
        let mic = InputTrack::new(100, Arc::default());
        let screen = InputTrack::new(50, Arc::default());
        let path =
            std::env::temp_dir().join(format!("studio-record-controls-{}.wav", std::process::id()));
        let mut writer = wav_writer(&path, 1, SCREEN_RATE).unwrap();
        mic.write(&mut writer, [0.8].into_iter()).unwrap();
        screen.write(&mut writer, [0.8].into_iter()).unwrap();
        mic.set_muted(true);
        screen.set_volume(25);
        mic.write(&mut writer, [0.8].into_iter()).unwrap();
        screen.write(&mut writer, [0.8].into_iter()).unwrap();
        writer.finalize().unwrap();
        let mut reader = hound::WavReader::open(&path).unwrap();
        let samples: Vec<_> = reader.samples::<f32>().map(Result::unwrap).collect();
        drop(reader);
        std::fs::remove_file(path).unwrap();
        assert_eq!(samples, [0.8, 0.4, 0.0, 0.2]);
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "requires Screen Recording permission and plays a quiet test tone"]
    fn native_screen_audio_captures_a_test_tone() {
        use crate::capture::{self, CaptureOptions, SourceState};
        use std::process::{Command, Stdio};

        assert!(
            capture::mac::screen_recording_allowed(),
            "Screen Recording permission is required"
        );
        let display = capture::list_displays()
            .into_iter()
            .next()
            .expect("a display is required");
        let feed = ScreenAudioFeed::default();
        let (sender, receiver) = bounded(32);
        *feed.0.lock() = Some(sender);
        let handle = capture::open(
            &display,
            CaptureOptions {
                max_width: 320,
                max_height: 180,
                fps: 10,
                screen_audio: Some(feed),
            },
        );
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !matches!(handle.state(), SourceState::Live)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            matches!(handle.state(), SourceState::Live),
            "{:?}",
            handle.state()
        );

        let tone =
            std::env::temp_dir().join(format!("studio-record-tone-{}.wav", std::process::id()));
        let mut writer = hound::WavWriter::create(
            &tone,
            hound::WavSpec {
                channels: SCREEN_CHANNELS,
                sample_rate: SCREEN_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        let angle = |frame, hz| std::f32::consts::TAU * frame as f32 * hz / SCREEN_RATE as f32;
        for frame in 0..SCREEN_RATE {
            writer
                .write_sample((angle(frame, 997.0f32).sin() * 7500.0) as i16)
                .unwrap();
            writer
                .write_sample((angle(frame, 611.0f32).sin() * 7500.0) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
        let mut player = Command::new("/usr/bin/afplay")
            .args(["-v", "0.15"])
            .arg(&tone)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut detected = false;
        while Instant::now() < deadline {
            let Ok(samples) = receiver.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            let power = |channel, hz| {
                let (sin, cos) = samples
                    .interleaved
                    .iter()
                    .skip(channel)
                    .step_by(2)
                    .enumerate()
                    .fold((0.0, 0.0), |(sin, cos), (frame, sample)| {
                        let (s, c) = angle(frame as u32, hz).sin_cos();
                        (sin + s * sample, cos + c * sample)
                    });
                sin.hypot(cos)
            };
            let left = power(0, 997.0);
            let right = power(1, 611.0);
            if left > 0.05
                && right > 0.05
                && left > power(0, 611.0) * 3.0
                && right > power(1, 997.0) * 3.0
            {
                detected = true;
                break;
            }
        }
        let _ = player.kill();
        let _ = player.wait();
        std::fs::remove_file(tone).unwrap();
        assert!(
            detected,
            "ScreenCaptureKit did not deliver the two test frequencies"
        );
    }
}
