//! Recording the program output to an MP4 file.
//!
//! Video: raw frames are piped into FFmpeg, which encodes with the
//! computer's hardware encoder when one works (VideoToolbox, NVENC, Quick
//! Sync, AMF, Media Foundation) and falls back to x264.
//!
//! Audio: the microphone is written to a WAV next to the video while
//! recording; when you stop, both are joined into the final MP4. The work in
//! progress is a Matroska file, so even a crash or power cut leaves a
//! playable recording behind.

use crate::frame::FrameRef;
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

static NEXT_ID: AtomicU64 = AtomicU64::new(1000);

pub fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

// ---------------------------------------------------------------------------
// Finding FFmpeg

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Studio Record")
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

fn works(p: &Path) -> bool {
    hidden(Command::new(p).arg("-version")).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("STUDIO_RECORD_FFMPEG") {
        candidates.push(p.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(exe_name()));
            candidates.push(dir.join("../Resources").join(exe_name()));
        }
    }
    candidates.push(data_dir().join("ffmpeg").join(exe_name()));
    candidates.push(PathBuf::from(exe_name())); // PATH
    for p in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg", "/snap/bin/ffmpeg"] {
        candidates.push(p.into());
    }
    candidates.into_iter().find(|p| works(p))
}

fn download_url() -> Option<&'static str> {
    // Static, widely used community builds.
    if cfg!(target_os = "windows") {
        Some("https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linuxarm64-gpl.tar.xz")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("https://www.osxexperts.net/ffmpeg80arm.zip")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("https://www.osxexperts.net/ffmpeg80intel.zip")
    } else {
        None
    }
}

fn find_named(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    subdirs.into_iter().find_map(|d| find_named(&d, name, depth - 1))
}

/// Downloads a static FFmpeg build for this computer into the app's data
/// folder, using the system's own `curl` and `tar` (both ship with macOS,
/// Windows 10+ and every mainstream Linux). Blocking: run it on a thread.
pub fn download_ffmpeg(progress: Arc<Mutex<String>>) -> Result<PathBuf, String> {
    let url = download_url().ok_or("No ready-made FFmpeg for this computer; install it from ffmpeg.org.")?;
    let dest = data_dir().join("ffmpeg");
    let work = data_dir().join("ffmpeg-download");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let archive = work.join(if url.ends_with(".tar.xz") { "ffmpeg.tar.xz" } else { "ffmpeg.zip" });
    *progress.lock() = "Downloading the recording engine (about 80 MB)…".into();
    let ok = hidden(Command::new("curl").args(["-fL", "--retry", "2", "-o"]).arg(&archive).arg(url))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err("Download failed — check the internet connection.".into());
    }
    *progress.lock() = "Unpacking…".into();
    let ok = hidden(Command::new("tar").arg("-xf").arg(&archive).arg("-C").arg(&work)).status().is_ok_and(|s| s.success());
    if !ok {
        return Err("Couldn't unpack the download.".into());
    }
    let found = find_named(&work, exe_name(), 4).ok_or("The download didn't contain FFmpeg.")?;
    let bin = dest.join(exe_name());
    std::fs::copy(&found, &bin).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&work);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr").args(["-d", "com.apple.quarantine"]).arg(&bin).status();
    }
    if works(&bin) {
        Ok(bin)
    } else {
        Err("The downloaded recording engine didn't start.".into())
    }
}

/// Picks the first video encoder that actually works on this machine.
pub fn pick_encoder(ffmpeg: &Path) -> String {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["h264_videotoolbox", "libx264"]
    } else if cfg!(windows) {
        &["h264_nvenc", "h264_qsv", "h264_amf", "h264_mf", "libx264"]
    } else {
        &["h264_nvenc", "libx264", "h264_vaapi"]
    };
    for enc in candidates {
        let ok = hidden(Command::new(ffmpeg).args([
            "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=640x360:r=30:d=0.3", "-pix_fmt",
            "yuv420p", "-c:v", enc, "-f", "null", "-",
        ]))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
        if ok {
            return enc.to_string();
        }
    }
    "mpeg4".into()
}

pub fn encoder_label(enc: &str) -> &'static str {
    match enc {
        "h264_videotoolbox" => "Apple hardware encoder",
        "h264_nvenc" => "NVIDIA hardware encoder",
        "h264_qsv" => "Intel Quick Sync",
        "h264_amf" => "AMD hardware encoder",
        "h264_mf" => "Windows hardware encoder",
        "libx264" => "x264 (software)",
        _ => "Basic encoder",
    }
}

fn encoder_args(enc: &str, height: u32) -> Vec<String> {
    let mbps = if height >= 1080 { 8 } else { 5 };
    let b = format!("{mbps}M");
    let v: Vec<&str> = match enc {
        "h264_videotoolbox" => vec!["-c:v", enc, "-b:v", &b, "-realtime", "1", "-allow_sw", "1", "-profile:v", "high"],
        "h264_nvenc" => vec!["-c:v", enc, "-preset", "p4", "-b:v", &b],
        "h264_qsv" | "h264_amf" | "h264_mf" => vec!["-c:v", enc, "-b:v", &b],
        "libx264" => vec!["-c:v", enc, "-preset", "veryfast", "-crf", "21", "-tune", "stillimage"],
        _ => vec!["-c:v", "mpeg4", "-q:v", "4"],
    };
    let mut v: Vec<String> = v.into_iter().map(String::from).collect();
    v.extend(["-pix_fmt", "yuv420p", "-g", "60"].map(String::from));
    v
}

// ---------------------------------------------------------------------------
// Microphones

pub fn list_microphones() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let mut out = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            if let Ok(desc) = d.description() {
                let n = desc.name().to_string();
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    out
}

struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<Option<(PathBuf, f64)>, String>>>,
}

/// Starts the microphone on its own thread (cpal streams aren't `Send`).
/// Returns the WAV path and the delay (seconds) between `t0` and the first
/// sample when stopped.
fn start_audio(mic: Option<String>, wav: PathBuf, t0: Instant, level: Arc<AtomicU32>, paused: Arc<AtomicBool>, muted: Arc<AtomicBool>) -> AudioCapture {
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name("microphone".into())
        .spawn(move || -> Result<Option<(PathBuf, f64)>, String> {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
            let host = cpal::default_host();
            let device = match &mic {
                Some(name) => host
                    .input_devices()
                    .ok()
                    .and_then(|mut it| it.find(|d| d.description().map(|x| x.name() == name).unwrap_or(false)))
                    .or_else(|| host.default_input_device()),
                None => host.default_input_device(),
            };
            let Some(device) = device else { return Ok(None) };
            let cfg = device.default_input_config().map_err(|e| e.to_string())?;
            let channels = cfg.channels();
            let rate = cfg.sample_rate();
            let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
            let writer = Arc::new(Mutex::new(Some(hound::WavWriter::create(&wav, spec).map_err(|e| e.to_string())?)));
            let first = Arc::new(Mutex::new(None::<Instant>));

            let w2 = writer.clone();
            let f2 = first.clone();
            let l2 = level.clone();
            let push = move |samples: &mut dyn Iterator<Item = f32>| {
                let mut f = f2.lock();
                if f.is_none() {
                    *f = Some(Instant::now());
                }
                drop(f);
                let mut peak = 0f32;
                if paused.load(Ordering::Relaxed) {
                    return; // paused: this stretch isn't in the recording
                }
                let mute = muted.load(Ordering::Relaxed);
                if let Some(w) = w2.lock().as_mut() {
                    for s in samples {
                        let s = if mute { 0.0 } else { s };
                        peak = peak.max(s.abs());
                        let _ = w.write_sample(s);
                    }
                }
                let prev = f32::from_bits(l2.load(Ordering::Relaxed));
                let v = if peak > prev { peak } else { prev * 0.9 + peak * 0.1 };
                l2.store(v.to_bits(), Ordering::Relaxed);
            };
            let err = |e: cpal::Error| log::warn!("microphone: {e}");
            let config = cfg.config();
            let stream = match cfg.sample_format() {
                cpal::SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
                    config,
                    move |d: &[f32], _| push(&mut d.iter().copied()),
                    err,
                    None,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
                    config,
                    move |d: &[i16], _| push(&mut d.iter().map(|s| *s as f32 / 32768.0)),
                    err,
                    None,
                ),
                cpal::SampleFormat::I32 => device.build_input_stream::<i32, _, _>(
                    config,
                    move |d: &[i32], _| push(&mut d.iter().map(|s| *s as f32 / 2147483648.0)),
                    err,
                    None,
                ),
                cpal::SampleFormat::U16 => device.build_input_stream::<u16, _, _>(
                    config,
                    move |d: &[u16], _| push(&mut d.iter().map(|s| (*s as f32 - 32768.0) / 32768.0)),
                    err,
                    None,
                ),
                other => return Err(format!("Unsupported microphone format {other:?}")),
            }
            .map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            while !s2.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(40));
            }
            drop(stream);
            if let Some(w) = writer.lock().take() {
                w.finalize().map_err(|e| e.to_string())?;
            }
            let offset = first.lock().map(|f| f.saturating_duration_since(t0).as_secs_f64()).unwrap_or(0.0);
            if first.lock().is_none() {
                return Ok(None); // permission denied or silent device: no audio track
            }
            Ok(Some((wav, offset)))
        })
        .ok();
    AudioCapture { stop, thread }
}

// ---------------------------------------------------------------------------
// A recording

#[derive(Clone, Debug)]
pub enum RecStatus {
    Recording,
    Finishing,
    Saved(PathBuf),
    Failed(String),
}

pub struct Recording {
    pub id: u64,
    pub started: Instant,
    pub path: PathBuf,
    pub status: Arc<Mutex<RecStatus>>,
    pub mic_level: Arc<AtomicU32>,
    pub dropped: Arc<AtomicU64>,
    pub paused: Arc<AtomicBool>,
    paused_total: Arc<Mutex<(Duration, Option<Instant>)>>,
    tx: Sender<FrameRef>,
    stop: Arc<AtomicBool>,
}

pub struct RecordRequest {
    pub ffmpeg: PathBuf,
    pub encoder: String,
    pub folder: PathBuf,
    pub mic: Option<String>,
    pub record_audio: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub muted: Arc<AtomicBool>,
}

impl Recording {
    pub fn sender(&self) -> Sender<FrameRef> {
        self.tx.clone()
    }

    pub fn mic_level(&self) -> f32 {
        f32::from_bits(self.mic_level.load(Ordering::Relaxed))
    }

    pub fn start(req: RecordRequest) -> Result<Recording, String> {
        std::fs::create_dir_all(&req.folder).map_err(|e| format!("Can't use the recordings folder: {e}"))?;
        let stamp = chrono::Local::now().format("%Y-%m-%d at %H.%M.%S").to_string();
        let base = format!("Lecture {stamp}");
        let final_path = req.folder.join(format!("{base}.mp4"));
        let video_tmp = req.folder.join(format!(".{base}.video.mkv"));
        let audio_tmp = req.folder.join(format!(".{base}.audio.wav"));
        let log_path = data_dir().join("last-recording.log");
        let _ = std::fs::create_dir_all(data_dir());

        let mut args: Vec<String> = vec!["-hide_banner", "-loglevel", "warning", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s"]
            .into_iter()
            .map(String::from)
            .collect();
        args.push(format!("{}x{}", req.width, req.height));
        args.extend(["-r".into(), req.fps.to_string(), "-i".into(), "-".into()]);
        args.extend(encoder_args(&req.encoder, req.height));
        args.push(video_tmp.to_string_lossy().into_owned());

        let log = std::fs::File::create(&log_path).ok();
        let mut child = hidden(Command::new(&req.ffmpeg).args(&args))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(log.map(Stdio::from).unwrap_or_else(Stdio::null))
            .spawn()
            .map_err(|e| format!("Couldn't start the recording engine: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("No pipe to the recording engine")?;

        let (tx, rx): (Sender<FrameRef>, Receiver<FrameRef>) = bounded(3);
        let status = Arc::new(Mutex::new(RecStatus::Recording));
        let stop = Arc::new(AtomicBool::new(false));
        let mic_level = Arc::new(AtomicU32::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let t0 = Instant::now();
        let paused = Arc::new(AtomicBool::new(false));
        let paused_total: Arc<Mutex<(Duration, Option<Instant>)>> = Arc::new(Mutex::new((Duration::ZERO, None)));

        let audio = if req.record_audio {
            Some(start_audio(req.mic.clone(), audio_tmp.clone(), t0, mic_level.clone(), paused.clone(), req.muted.clone()))
        } else {
            None
        };
        let (pz, pt) = (paused.clone(), paused_total.clone());

        let (st, sp, dr) = (status.clone(), stop.clone(), dropped.clone());
        let (w, h, fps) = (req.width, req.height, req.fps.max(1));
        let ffmpeg = req.ffmpeg.clone();
        let final2 = final_path.clone();
        std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                let frame_len = (w * h * 4) as usize;
                let black = vec![0u8; frame_len];
                let mut last: Option<FrameRef> = None;
                let mut written: u64 = 0;
                let mut failed: Option<String> = None;
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(f) if f.data.len() == frame_len => last = Some(f),
                        Ok(_) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                    if sp.load(Ordering::SeqCst) {
                        break;
                    }
                    // Keep exactly `fps` frames per second of (unpaused) wall
                    // time, so the picture never drifts away from the voice.
                    {
                        let mut p = pt.lock();
                        let now_paused = pz.load(Ordering::Relaxed);
                        match (now_paused, p.1) {
                            (true, None) => p.1 = Some(Instant::now()),
                            (false, Some(since)) => {
                                p.0 += since.elapsed();
                                p.1 = None;
                            }
                            _ => {}
                        }
                        if now_paused {
                            continue;
                        }
                    }
                    let active = t0.elapsed().saturating_sub(pt.lock().0);
                    let due = (active.as_secs_f64() * fps as f64).floor() as u64 + 1;
                    let behind = due.saturating_sub(written);
                    if behind > 1 {
                        dr.fetch_add(behind - 1, Ordering::Relaxed);
                    }
                    while written < due {
                        let data: &[u8] = last.as_ref().map(|f| f.data.as_slice()).unwrap_or(&black);
                        if let Err(e) = stdin.write_all(data) {
                            failed = Some(format!("The recording engine stopped: {e}"));
                            break;
                        }
                        written += 1;
                    }
                    if failed.is_some() {
                        break;
                    }
                }
                *st.lock() = RecStatus::Finishing;
                drop(stdin);
                let enc_ok = child.wait().is_ok_and(|s| s.success());
                let audio_res = audio.map(|mut a| {
                    a.stop.store(true, Ordering::SeqCst);
                    a.thread.take().and_then(|t| t.join().ok()).unwrap_or(Ok(None))
                });
                if let Some(msg) = failed {
                    if !video_tmp.exists() {
                        *st.lock() = RecStatus::Failed(msg);
                        return;
                    }
                }
                if !enc_ok && !video_tmp.exists() {
                    *st.lock() = RecStatus::Failed("The recording engine couldn't write the video. See last-recording.log.".into());
                    return;
                }
                let audio_part = match audio_res {
                    Some(Ok(Some(a))) => Some(a),
                    _ => None,
                };
                match finalize(&ffmpeg, &video_tmp, audio_part.as_ref(), &final2) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&video_tmp);
                        if let Some((a, _)) = &audio_part {
                            let _ = std::fs::remove_file(a);
                        }
                        *st.lock() = RecStatus::Saved(final2);
                    }
                    Err(e) => {
                        // Keep the raw parts so nothing is lost.
                        let keep = final2.with_extension("mkv");
                        let _ = std::fs::rename(&video_tmp, &keep);
                        *st.lock() = RecStatus::Failed(format!("Saved the video without joining audio ({e}). File: {}", keep.display()));
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Recording {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            started: t0,
            path: final_path,
            status,
            mic_level,
            dropped,
            paused,
            paused_total,
            tx,
            stop,
        })
    }

    pub fn stop(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn set_paused(&self, on: bool) {
        self.paused.store(on, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Length of the recording so far, not counting pauses.
    pub fn elapsed(&self) -> Duration {
        let p = self.paused_total.lock();
        let extra = p.1.map(|s| s.elapsed()).unwrap_or_default();
        self.started.elapsed().saturating_sub(p.0 + extra)
    }

    pub fn status(&self) -> RecStatus {
        self.status.lock().clone()
    }
}

fn finalize(ffmpeg: &Path, video: &Path, audio: Option<&(PathBuf, f64)>, out: &Path) -> Result<(), String> {
    let partial = out.with_extension("partial.mp4");
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(video);
    if let Some((wav, offset)) = audio {
        cmd.args(["-itsoffset", &format!("{offset:.3}")]);
        let channels = hound::WavReader::open(wav).map_err(|e| e.to_string())?.spec().channels;
        if channels == 1 {
            // Hound's extensible mono WAV names the channel FL, which
            // FFmpeg's AAC encoder rejects as a single-channel layout.
            cmd.args(["-channel_layout", "mono"]);
        }
        cmd.arg("-i").arg(wav);
        cmd.args(["-map", "0:v:0", "-map", "1:a:0", "-af", "apad", "-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-shortest"]);
    } else {
        cmd.args(["-c", "copy"]);
    }
    cmd.args(["-movflags", "+faststart"]).arg(&partial);
    let o = hidden(&mut cmd).output().map_err(|e| e.to_string())?;
    if o.status.success() {
        std::fs::rename(&partial, out).map_err(|e| e.to_string())
    } else {
        let _ = std::fs::remove_file(&partial);
        Err(String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("unknown error").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires FFmpeg"]
    fn exports_hound_mono_without_cutting_the_picture_short() {
        let ffmpeg = find_ffmpeg().expect("FFmpeg is required for this test");
        let dir = std::env::temp_dir().join(format!("studio-record-export-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let video = dir.join("video.mkv");
        let wav = dir.join("voice.wav");
        let out = dir.join("recording.mp4");
        let input = Command::new(&ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "color=s=64x64:r=20:d=2", "-c:v", "mpeg4"])
            .arg(&video)
            .output()
            .unwrap();
        assert!(input.status.success(), "{}", String::from_utf8_lossy(&input.stderr));
        let mut writer = hound::WavWriter::create(&wav, hound::WavSpec {
            channels: 1,
            sample_rate: 24000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        }).unwrap();
        for i in 0..12_000 {
            writer.write_sample((i as f32 * 0.1).sin() * 0.25).unwrap();
        }
        writer.finalize().unwrap();
        finalize(&ffmpeg, &video, Some(&(wav, 0.1)), &out).unwrap();
        let decoded = Command::new(&ffmpeg)
            .args(["-v", "error", "-i"]).arg(&out)
            .args(["-map", "0:a:0", "-f", "s16le", "-"])
            .output().unwrap();
        assert!(decoded.status.success());
        assert!(decoded.stdout.len() >= 24_000 * 2 * 18 / 10, "audio padding should cover the full video");
        assert!(decoded.stdout.chunks_exact(2).any(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > 1000));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
