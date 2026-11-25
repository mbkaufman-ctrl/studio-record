//! The studio window.

use crate::audio_bridge::{self, AudioBridge, BridgeStatus};
use crate::link::{self, LinkPage};
use crate::permissions::{self, Kind as PermKind, Status as PermStatus};
use crate::capture::{self, CaptureHandle, CaptureOptions, DeviceInfo, DeviceKind, DeviceWatcher, Permission, SourceState};
use crate::icons::{self, Icon};
use crate::mixer::{self, Corner, Layout, MixSettings, Mixer, OutputSize, PipShape, PipSize, Role, Scene, Transition};
use crate::recorder::{self, RecStatus, RecordRequest, Recording};
use crate::recording_audio::{InputState, InputTrack, ScreenAudioFeed, ScreenAudioRequest};
use crate::theme::{self, ThemeChoice, Tokens, R, R_SM, S12, S16, S20, S24, S4, S8};
use crate::vcam::{self, VcamStatus, VirtualCamera};
use crate::widgets::{self, font, font_medium, font_semibold, mono, HoverTr, Variant};
use crate::i18n::{self, tr, trf, Lang};
use crossbeam_channel::{unbounded, Receiver};
use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Settings

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub camera: Option<String>,
    pub screen: Option<String>,
    pub tablet: Option<String>,
    pub mic: Option<String>,
    pub record_audio: bool,
    pub mix: MixSettings,
    pub scenes: Vec<Scene>,
    pub active_scene: usize,
    pub folder: Option<PathBuf>,
    pub theme: ThemeChoice,
    pub vcam_target: Option<String>,
    pub vcam_on_launch: bool,
    pub auto_picked: bool,
    pub auto_picked_screen: bool,
    /// Last link loaded in the Link bar, and recent ones.
    pub link_url: String,
    pub recent_links: Vec<String>,
    /// Windows/Linux: the browser window that shows the link.
    pub link_window: Option<String>,
    pub scene_set: u32,
    pub camera_on: bool,
    pub mic_muted: bool,
    pub mic_volume: u8,
    pub record_screen_audio: bool,
    pub screen_audio_volume: u8,
    pub screen_audio_muted: bool,
    pub panel_on_launch: bool,
    pub bridge_output: Option<String>,
    pub bridge_on: bool,
    pub setup_seen: bool,
    /// None until chosen: follows the system language.
    pub lang: Option<Lang>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            camera: None,
            screen: None,
            tablet: None,
            mic: None,
            record_audio: true,
            mix: MixSettings::default(),
            scenes: mixer::default_scenes(),
            active_scene: 0,
            folder: None,
            theme: ThemeChoice::System,
            vcam_target: None,
            vcam_on_launch: false,
            auto_picked: false,
            auto_picked_screen: false,
            link_url: String::new(),
            recent_links: Vec::new(),
            link_window: None,
            scene_set: mixer::SCENE_SET,
            camera_on: true,
            mic_muted: false,
            mic_volume: 100,
            record_screen_audio: false,
            screen_audio_volume: 100,
            screen_audio_muted: false,
            panel_on_launch: true,
            bridge_output: None,
            bridge_on: false,
            setup_seen: false,
            lang: None,
        }
    }
}

impl Settings {
    fn device_for(&self, role: Role) -> &Option<String> {
        match role {
            Role::Camera => &self.camera,
            Role::Screen => &self.screen,
            Role::Tablet => &self.tablet,
            Role::Link => &self.link_window,
        }
    }
    fn device_for_mut(&mut self, role: Role) -> &mut Option<String> {
        match role {
            Role::Camera => &mut self.camera,
            Role::Screen => &mut self.screen,
            Role::Tablet => &mut self.tablet,
            Role::Link => &mut self.link_window,
        }
    }
    fn folder(&self) -> PathBuf {
        self.folder.clone().unwrap_or_else(default_folder)
    }
}

fn default_folder() -> PathBuf {
    dirs::video_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("Studio Record")
}

// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Engine {
    Looking,
    Ready { path: PathBuf, encoder: String },
    Missing,
    Downloading(Arc<Mutex<String>>),
    Failed(String),
}

#[derive(Clone, Copy, Debug)]
enum Hot {
    Scene(usize),
    Record,
    Vcam,
    Next,
    Prev,
    Hold,
    Mute,
}

struct Toast {
    text: String,
    action: Option<(String, PathBuf)>,
    born: Instant,
    error: bool,
}

struct Palette {
    query: String,
    selected: usize,
    opened: Instant,
}

#[derive(Clone)]
enum Command {
    Scene(usize),
    ToggleRecord,
    ToggleVcam,
    OutputWindow,
    MiniControls,
    OpenFolder,
    ToggleTheme,
    Mirror,
    Refresh,
    Setup,
    SetLanguage(Lang),
    NextSlide,
    PrevSlide,
    Hold,
    Mute,
    CameraOnOff,
    ShowPage,
}

pub struct ScreenshotJob {
    pub path: PathBuf,
    pub frames: u32,
    pub requested: bool,
    pub done: Arc<AtomicBool>,
}

pub struct StudioApp {
    s: Settings,
    live: bool,
    shot: Option<ScreenshotJob>,
    watcher: DeviceWatcher,
    devices: Vec<DeviceInfo>,
    dev_gen: u64,
    sources: HashMap<Role, CaptureHandle>,
    retry_at: HashMap<Role, Instant>,
    mixer: Mixer,
    program_tex: Option<egui::TextureHandle>,
    program_seq: u64,
    recording: Option<Recording>,
    finishing: Vec<Recording>,
    engine: Arc<Mutex<Engine>>,
    mics: Vec<String>,
    vcam: Option<VirtualCamera>,
    vcam_targets: Vec<String>,
    palette: Option<Palette>,
    output_window: bool,
    mini: bool,
    _hotkeys: Option<global_hotkey::GlobalHotKeyManager>,
    hot_rx: Receiver<Hot>,
    toasts: Vec<Toast>,
    started: Instant,
    link: Option<LinkPage>,
    link_input: String,
    mic_muted: Arc<AtomicBool>,
    screen_audio_feed: ScreenAudioFeed,
    bridge: Option<AudioBridge>,
    outputs: Vec<String>,
    setup_open: bool,
    perms: HashMap<PermKind, PermStatus>,
    perms_at: Instant,
    perm_note: Option<String>,
    /// Arrow keys we send into the page can bounce back to this window;
    /// ignore slide keys until this moment.
    synthetic_until: Instant,
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, shot: Option<PathBuf>) -> Self {
        #[cfg(target_os = "macos")]
        capture::mac::keep_awake();
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        let live = shot.is_none();
        let s: Settings = if live {
            cc.storage.and_then(|st| eframe::get_value(st, eframe::APP_KEY)).unwrap_or_default()
        } else {
            let light = std::env::var("STUDIO_RECORD_THEME").map(|v| v == "light").unwrap_or(false);
            Settings { theme: if light { ThemeChoice::Light } else { ThemeChoice::Dark }, ..Default::default() }
        };
        let mut s = s;
        if !live && std::env::var_os("STUDIO_RECORD_TEST_AUDIO").is_some() {
            s.record_screen_audio = true;
        }
        if s.scenes.is_empty() || s.scene_set < mixer::SCENE_SET {
            s.scenes = mixer::default_scenes();
            s.scene_set = mixer::SCENE_SET;
            s.active_scene = 0;
        }
        theme::set_choice(&cc.egui_ctx, s.theme);
        let lang = if live {
            s.lang.unwrap_or_else(Lang::system)
        } else {
            std::env::var("STUDIO_RECORD_LANG").ok().and_then(|locale| Lang::from_locale(&locale)).unwrap_or(Lang::En)
        };
        i18n::set(lang);

        let mixer = Mixer::start(s.mix.clone(), s.scenes.clone(), s.active_scene);
        let watcher = DeviceWatcher::start();

        // Recording engine discovery happens off the UI thread.
        let engine = Arc::new(Mutex::new(Engine::Looking));
        {
            let e = engine.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let found = recorder::find_ffmpeg();
                *e.lock() = match found {
                    Some(p) => {
                        let enc = recorder::pick_encoder(&p);
                        Engine::Ready { path: p, encoder: enc }
                    }
                    None => Engine::Missing,
                };
                ctx.request_repaint();
            });
        }

        let (hot_tx, hot_rx) = unbounded();
        let hotkeys = if live { register_hotkeys(&cc.egui_ctx, mixer.clone(), hot_tx) } else { None };

        let mics = if live { recorder::list_microphones() } else { Vec::new() };
        let vcam_targets = if live { vcam::list_targets() } else { Vec::new() };
        let outputs = if live { audio_bridge::list_outputs() } else { Vec::new() };
        let mic_muted = Arc::new(AtomicBool::new(s.mic_muted));
        let link_input = s.link_url.clone();
        if live {
            // The control panel must keep working while this window is
            // minimised or hidden.
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        }
        let mini = live && s.panel_on_launch;

        let mut app = StudioApp {
            s,
            live,
            shot: shot.map(|path| ScreenshotJob { path, frames: 0, requested: false, done: Arc::new(AtomicBool::new(false)) }),
            watcher,
            devices: Vec::new(),
            dev_gen: u64::MAX,
            sources: HashMap::new(),
            retry_at: HashMap::new(),
            mixer,
            program_tex: None,
            program_seq: 0,
            recording: None,
            finishing: Vec::new(),
            engine,
            mics,
            vcam: None,
            vcam_targets,
            palette: None,
            output_window: false,
            mini,
            _hotkeys: hotkeys,
            hot_rx,
            toasts: Vec::new(),
            started: Instant::now(),
            link: None,
            link_input,
            mic_muted,
            screen_audio_feed: ScreenAudioFeed::default(),
            bridge: None,
            outputs,
            setup_open: false,
            perms: HashMap::new(),
            perms_at: Instant::now() - Duration::from_secs(10),
            perm_note: None,
            synthetic_until: Instant::now(),
        };
        if !live {
            // Pictures for checks: list devices once so the panel is real,
            // but never open a camera or the screen.
            app.watcher.refresh_now();
        }
        if live && app.s.vcam_on_launch {
            app.toggle_vcam();
        }
        if live {
            app.refresh_perms();
            // First launch, or something essential isn't allowed: open the
            // setup check so it's one click to fix.
            let missing = [PermKind::Camera, PermKind::Microphone].iter().any(|k| app.perms.get(k) != Some(&PermStatus::Granted));
            if !app.s.setup_seen || missing {
                app.setup_open = true;
                app.s.setup_seen = true;
            }
            if app.s.bridge_on {
                app.toggle_bridge();
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            app.link = Some(LinkPage::new());
        }
        if !live {
            // Developer check: STUDIO_RECORD_TEST_LINK=<url> shows a page in
            // the screenshot (and _NEXT presses Next once).
            if let Ok(u) = std::env::var("STUDIO_RECORD_TEST_LINK") {
                app.load_link(&u);
            }
            if std::env::var("STUDIO_RECORD_TEST_SETUP").is_ok() {
                app.setup_open = true;
            }
            if std::env::var("STUDIO_RECORD_TEST_PANEL").is_ok() {
                // Draw the floating panel inside the window so it's in the picture.
                cc.egui_ctx.set_embed_viewports(true);
                app.mini = true;
            }
        }
        app
    }

    fn capture_opts(&self) -> CaptureOptions {
        let (w, h) = self.s.mix.size.dims();
        CaptureOptions { max_width: w, max_height: h, fps: self.s.mix.fps, screen_audio: None }
    }

    // -- devices -----------------------------------------------------------

    fn sync_devices(&mut self) {
        let gen = self.watcher.generation.load(Ordering::SeqCst);
        if gen == self.dev_gen {
            return;
        }
        self.dev_gen = gen;
        let before: Vec<String> = self.devices.iter().map(|d| d.id.clone()).collect();
        self.devices = self.watcher.snapshot();
        if std::env::var("STUDIO_RECORD_DEBUG").is_ok() {
            eprintln!("devices gen {gen}: {:?}", self.devices.iter().map(|d| (&d.name, d.kind)).collect::<Vec<_>>());
        }
        if self.devices.is_empty() {
            return;
        }

        // First launch: choose sensible defaults so it works immediately.
        // (Camera and screen separately: each list can arrive a moment apart.)
        if !self.s.auto_picked {
            if let Some(cam) = pick_default_camera(&self.devices) {
                self.s.auto_picked = true;
                if self.s.camera.is_none() {
                    self.s.camera = Some(cam.id.clone());
                }
            }
        }
        if !self.s.auto_picked_screen {
            if let Some(d) = self.devices.iter().find(|d| d.kind == DeviceKind::Display) {
                self.s.auto_picked_screen = true;
                if self.s.screen.is_none() {
                    self.s.screen = Some(d.id.clone());
                }
            }
        }
        // An iPad plugged in while the iPad slot is empty: use it.
        if self.s.tablet.is_none() || !self.devices.iter().any(|d| Some(&d.id) == self.s.tablet.as_ref()) {
            if let Some(ipad) = self.devices.iter().find(|d| d.kind == DeviceKind::MobileScreen && !before.contains(&d.id)) {
                if self.s.tablet.as_ref() != Some(&ipad.id) {
                    let was_empty = self.s.tablet.is_none();
                    self.s.tablet = Some(ipad.id.clone());
                    if !before.is_empty() || was_empty {
                        self.toast(trf("{} connected — ready in the iPad scenes.", &[&ipad.name]), None, false);
                    }
                }
            }
        }
    }

    fn device(&self, id: &str) -> Option<&DeviceInfo> {
        self.devices.iter().find(|d| d.id == id)
    }

    fn ensure_sources(&mut self) {
        if !self.live {
            return;
        }
        let opts = self.capture_opts();
        let roles: &[Role] = if cfg!(target_os = "macos") { &Role::DEVICES } else { &[Role::Camera, Role::Screen, Role::Tablet, Role::Link] };
        for &role in roles {
            let mut want = self.s.device_for(role).clone();
            if role == Role::Camera && !self.s.camera_on {
                want = None; // "Video off": the camera light goes out too
            }
            let current = self.sources.get(&role).map(|h| h.device.id.clone());
            if want != current {
                self.sources.remove(&role);
                self.mixer.set_source(role, None);
            }
            let Some(id) = want else { continue };
            let Some(dev) = self.device(&id).cloned() else {
                // Unplugged: drop the session; it restarts when it's back.
                if self.sources.remove(&role).is_some() {
                    self.mixer.set_source(role, None);
                }
                continue;
            };
            if let Some(h) = self.sources.get(&role) {
                if matches!(h.state(), SourceState::Error(_)) {
                    let due = self.retry_at.entry(role).or_insert_with(|| Instant::now() + Duration::from_secs(4));
                    if Instant::now() >= *due {
                        self.retry_at.remove(&role);
                        self.sources.remove(&role);
                    }
                }
            }
            if !self.sources.contains_key(&role) {
                let mut source_opts = opts.clone();
                if role == Role::Screen && self.s.record_screen_audio && capture::screen_audio_supported() {
                    source_opts.screen_audio = Some(self.screen_audio_feed.clone());
                }
                let h = capture::open(&dev, source_opts);
                self.mixer.set_source(role, Some(h.slot.clone()));
                self.sources.insert(role, h);
            }
        }
    }

    fn restart_sources(&mut self) {
        self.sources.clear();
        for r in Role::DEVICES {
            self.mixer.set_source(r, None);
        }
    }

    fn set_lang(&mut self, ctx: &egui::Context, l: Lang) {
        self.s.lang = Some(l);
        ctx.request_repaint();
    }

    fn lang_picker(&mut self, ui: &mut Ui, width: f32, id: &str) {
        if let Some(language) = widgets::language_picker(ui, width, id, i18n::lang()) {
            self.set_lang(ui.ctx(), language);
        }
    }

    // -- permissions ---------------------------------------------------------

    fn refresh_perms(&mut self) {
        for k in [PermKind::Camera, PermKind::Microphone, PermKind::Screen] {
            self.perms.insert(k, permissions::status(k));
        }
        self.perms_at = Instant::now();
    }

    fn perm(&self, k: PermKind) -> PermStatus {
        self.perms.get(&k).copied().unwrap_or(PermStatus::NotAsked)
    }

    fn setup_issues(&self) -> usize {
        if !self.live {
            return 0;
        }
        let mut n = [PermKind::Camera, PermKind::Microphone, PermKind::Screen].iter().filter(|k| self.perm(**k) != PermStatus::Granted).count();
        if self.vcam_targets.is_empty() {
            n += 1;
        }
        n
    }

    // -- link ----------------------------------------------------------------

    fn load_link(&mut self, url: &str) {
        let Some(u) = link::normalize_url(url) else { return };
        #[cfg(target_os = "macos")]
        if self.link.is_none() {
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                let mut page = LinkPage::new(mtm);
                page.max_width = self.s.mix.size.dims().0;
                self.mixer.set_source(Role::Link, Some(page.slot.clone()));
                self.link = Some(page);
            }
        }
        if let Some(l) = &mut self.link {
            l.load(&u);
        }
        self.link_input = u.clone();
        self.s.link_url = u.clone();
        self.s.recent_links.retain(|x| x != &u);
        self.s.recent_links.insert(0, u);
        self.s.recent_links.truncate(8);
        // Loading a link means you want to show it: go to "Link + Face".
        if let Some(i) = self.s.scenes.iter().position(|sc| sc.layout.roles().contains(&Role::Link)) {
            let cur = self.s.scenes.get(self.s.active_scene).map(|sc| sc.layout.roles().contains(&Role::Link)).unwrap_or(false);
            if !cur {
                self.switch_scene(i);
            }
        }
    }

    fn slide(&mut self, forward: bool) {
        self.synthetic_until = Instant::now() + Duration::from_millis(250);
        if let Some(l) = &self.link {
            if forward {
                l.next();
            } else {
                l.prev();
            }
        }
    }

    // -- pause / mute / camera / audio bridge --------------------------------

    fn toggle_hold(&mut self) {
        let on = !self.mixer.hold();
        self.mixer.set_hold(on);
        if let Some(r) = &self.recording {
            r.set_paused(on);
        }
    }

    fn toggle_mute(&mut self) {
        self.s.mic_muted = !self.s.mic_muted;
        self.mic_muted.store(self.s.mic_muted, Ordering::SeqCst);
        if let Some(track) = self.recording.as_ref().and_then(|rec| rec.microphone.as_ref()) {
            track.set_muted(self.s.mic_muted);
        }
    }

    fn toggle_camera(&mut self) {
        self.s.camera_on = !self.s.camera_on;
    }

    fn toggle_bridge(&mut self) {
        if self.bridge.take().is_some() {
            self.s.bridge_on = false;
            return;
        }
        let out = self.s.bridge_output.clone().or_else(|| self.outputs.iter().find(|n| audio_bridge::is_virtual_name(n)).cloned());
        let Some(out) = out else {
            self.s.bridge_on = false;
            self.toast("Install a virtual microphone first (BlackHole on Mac, VB-CABLE on Windows) — see Setup check.".into(), None, true);
            return;
        };
        self.s.bridge_output = Some(out.clone());
        self.bridge = Some(AudioBridge::start(self.s.mic.clone(), out, self.mic_muted.clone()));
        self.s.bridge_on = true;
    }

    // -- actions -------------------------------------------------------------

    fn switch_scene(&mut self, i: usize) {
        if i < self.s.scenes.len() {
            self.mixer.switch_to(i);
            self.s.active_scene = i;
        }
    }

    fn push_mix_settings(&mut self) {
        self.mixer.state.lock().settings = self.s.mix.clone();
    }

    fn toggle_record(&mut self) {
        if let Some(rec) = self.recording.take() {
            rec.stop();
            self.mixer.remove_output(rec.id);
            self.finishing.push(rec);
            return;
        }
        if !self.finishing.is_empty() {
            return;
        }
        let engine = self.engine.lock().clone();
        let Engine::Ready { path, encoder } = engine else {
            self.toast("Recording needs the free FFmpeg engine — set it up in Output.".into(), None, true);
            return;
        };
        let (w, h) = self.s.mix.size.dims();
        let req = RecordRequest {
            ffmpeg: path,
            encoder,
            folder: self.s.folder(),
            mic: self.s.mic.clone(),
            record_audio: self.s.record_audio,
            mic_volume: self.s.mic_volume,
            screen_audio: (self.s.record_screen_audio && capture::screen_audio_supported()).then(|| ScreenAudioRequest {
                feed: self.screen_audio_feed.clone(),
                volume: self.s.screen_audio_volume,
                muted: self.s.screen_audio_muted,
            }),
            width: w,
            height: h,
            fps: self.s.mix.fps,
            muted: self.mic_muted.clone(),
        };
        match Recording::start(req) {
            Ok(rec) => {
                if self.mixer.hold() {
                    rec.set_paused(true);
                }
                self.mixer.add_output(rec.id, rec.sender());
                self.recording = Some(rec);
            }
            Err(e) => self.toast(e, None, true),
        }
    }

    fn toggle_vcam(&mut self) {
        if let Some(v) = self.vcam.take() {
            self.mixer.remove_output(v.id);
            self.s.vcam_on_launch = false;
            return;
        }
        let (w, h) = self.s.mix.size.dims();
        let v = VirtualCamera::start(w, h, self.s.mix.fps, self.s.vcam_target.clone());
        self.mixer.add_output(v.id, v.sender());
        self.vcam = Some(v);
        self.s.vcam_on_launch = true;
    }

    fn restart_vcam(&mut self) {
        if self.vcam.is_some() {
            self.toggle_vcam();
            self.toggle_vcam();
        }
    }

    fn run(&mut self, ctx: &egui::Context, c: Command) {
        match c {
            Command::Scene(i) => self.switch_scene(i),
            Command::ToggleRecord => self.toggle_record(),
            Command::ToggleVcam => self.toggle_vcam(),
            Command::OutputWindow => self.output_window = !self.output_window,
            Command::MiniControls => self.mini = !self.mini,
            Command::OpenFolder => {
                let f = self.s.folder();
                let _ = std::fs::create_dir_all(&f);
                let _ = open::that(f);
            }
            Command::ToggleTheme => {
                self.s.theme = if theme::current(ctx).dark { ThemeChoice::Light } else { ThemeChoice::Dark };
                theme::set_choice(ctx, self.s.theme);
            }
            Command::Mirror => {
                self.s.mix.mirror_camera = !self.s.mix.mirror_camera;
                self.push_mix_settings();
            }
            Command::Refresh => {
                self.watcher.refresh_now();
                self.mics = recorder::list_microphones();
                self.vcam_targets = vcam::list_targets();
                self.outputs = audio_bridge::list_outputs();
            }
            Command::Setup => self.setup_open = !self.setup_open,
            Command::SetLanguage(language) => self.set_lang(ctx, language),
            Command::NextSlide => self.slide(true),
            Command::PrevSlide => self.slide(false),
            Command::Hold => self.toggle_hold(),
            Command::Mute => self.toggle_mute(),
            Command::CameraOnOff => self.toggle_camera(),
            Command::ShowPage => {
                if let Some(l) = &mut self.link {
                    let s = !l.shown();
                    l.set_shown(s);
                }
            }
        }
    }

    fn toast(&mut self, text: String, action: Option<(String, PathBuf)>, error: bool) {
        self.toasts.push(Toast { text, action, born: Instant::now(), error });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(h) = self.hot_rx.try_recv() {
            match h {
                Hot::Scene(i) => self.s.active_scene = i,
                Hot::Record => self.toggle_record(),
                Hot::Vcam => self.toggle_vcam(),
                Hot::Next => self.slide(true),
                Hot::Prev => self.slide(false),
                Hot::Hold => self.toggle_hold(),
                Hot::Mute => self.toggle_mute(),
            }
        }
        if self.live && self.perms_at.elapsed() > Duration::from_millis(1000) {
            self.refresh_perms();
            self.vcam_targets = vcam::list_targets();
        }
        if let Some(l) = &mut self.link {
            l.tick();
        }
        self.s.active_scene = self.mixer.active();
        let mut done = Vec::new();
        for (i, r) in self.finishing.iter().enumerate() {
            match r.status() {
                RecStatus::Saved(p) => {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let audio_issue = [
                        ("Microphone", r.microphone.as_deref()),
                        ("Screen audio", r.screen_audio.as_deref()),
                    ]
                    .into_iter()
                    .filter_map(|(name, input)| {
                        input.filter(|input| matches!(input.state(), InputState::Error(_))).map(|_| tr(name))
                    })
                    .collect::<Vec<_>>().join(" · ");
                    let text = if audio_issue.is_empty() {
                        trf("Saved \"{}\"", &[&name])
                    } else {
                        trf("Saved \"{}\". Check audio from {}.", &[&name, &audio_issue])
                    };
                    done.push((i, text, Some(p), !audio_issue.is_empty()));
                }
                RecStatus::Failed(e) => done.push((i, e, None, true)),
                _ => {}
            }
        }
        for (i, text, path, err) in done.into_iter().rev() {
            self.finishing.remove(i);
            let label = if cfg!(target_os = "macos") { "Show in Finder" } else { "Show file" };
            self.toast(text, path.map(|p| (label.to_string(), p)), err);
        }
        if let Some(r) = &self.recording {
            if let RecStatus::Failed(e) = r.status() {
                let rec = self.recording.take().unwrap();
                self.mixer.remove_output(rec.id);
                self.toast(e, None, true);
            }
        }
        self.toasts.retain(|t| t.born.elapsed() < Duration::from_secs(if t.action.is_some() { 8 } else { 5 }));
        let _ = ctx;
    }

    fn update_program_texture(&mut self, ctx: &egui::Context) {
        let frame = if self.mixer.hold() { self.mixer.hold_frame.get().or_else(|| self.mixer.program.get()) } else { self.mixer.program.get() };
        if let Some(f) = frame {
            if f.seq != self.program_seq {
                self.program_seq = f.seq;
                let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.data);
                match &mut self.program_tex {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => self.program_tex = Some(ctx.load_texture("program", img, egui::TextureOptions::LINEAR)),
                }
            }
        }
    }
}

fn pick_default_camera(devs: &[DeviceInfo]) -> Option<&DeviceInfo> {
    let cams: Vec<&DeviceInfo> = devs.iter().filter(|d| d.kind == DeviceKind::Camera && !d.is_virtual).collect();
    // An external webcam beats the laptop camera; Continuity Camera last
    // (the phone may not stay nearby).
    cams.iter()
        .find(|d| d.detail.starts_with("USB"))
        .or_else(|| cams.iter().find(|d| d.detail != "Built-in" && d.detail != "Continuity Camera" && d.detail != "Camera"))
        .or_else(|| cams.iter().find(|d| d.detail == "Built-in"))
        .or_else(|| cams.first())
        .copied()
}

fn register_hotkeys(ctx: &egui::Context, mixer: Mixer, tx: crossbeam_channel::Sender<Hot>) -> Option<global_hotkey::GlobalHotKeyManager> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    let mgr = global_hotkey::GlobalHotKeyManager::new().ok()?;
    let mods = Some(Modifiers::CONTROL | Modifiers::ALT);
    let digits = [Code::Digit1, Code::Digit2, Code::Digit3, Code::Digit4, Code::Digit5, Code::Digit6, Code::Digit7, Code::Digit8, Code::Digit9];
    let mut map: HashMap<u32, Hot> = HashMap::new();
    for (i, d) in digits.iter().enumerate() {
        let hk = HotKey::new(mods, *d);
        if mgr.register(hk).is_ok() {
            map.insert(hk.id(), Hot::Scene(i));
        }
    }
    for (code, act) in [
        (Code::KeyR, Hot::Record),
        (Code::KeyC, Hot::Vcam),
        (Code::ArrowRight, Hot::Next),
        (Code::ArrowLeft, Hot::Prev),
        (Code::KeyP, Hot::Hold),
        (Code::KeyM, Hot::Mute),
    ] {
        let hk = HotKey::new(mods, code);
        if mgr.register(hk).is_ok() {
            map.insert(hk.id(), act);
        }
    }
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || {
            let rx = global_hotkey::GlobalHotKeyEvent::receiver();
            while let Ok(ev) = rx.recv() {
                if ev.state != global_hotkey::HotKeyState::Pressed {
                    continue;
                }
                if let Some(a) = map.get(&ev.id) {
                    // Scene changes go straight to the mixer: no waiting
                    // for the window to redraw while it's in the background.
                    if let Hot::Scene(i) = a {
                        mixer.switch_to(*i);
                    }
                    let _ = tx.send(*a);
                    ctx.request_repaint();
                }
            }
        })
        .ok();
    Some(mgr)
}

// ---------------------------------------------------------------------------
// UI

impl eframe::App for StudioApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.live {
            eframe::set_value(storage, eframe::APP_KEY, &self.s);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(r) = self.recording.take() {
            r.stop();
            // Give the encoder a moment to close the file cleanly.
            let t = Instant::now();
            while !matches!(r.status(), RecStatus::Saved(_) | RecStatus::Failed(_)) && t.elapsed() < Duration::from_secs(8) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.mixer.stop();
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(language) = self.s.lang {
            i18n::set(language);
        }
        let t = theme::current(&ctx);
        ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg_base);
        self.sync_devices();
        self.ensure_sources();
        self.poll(&ctx);
        self.update_program_texture(&ctx);
        self.keyboard(&ctx);

        egui::Panel::top("top")
            .exact_size(52.0)
            .frame(egui::Frame::new().fill(t.bg_base).inner_margin(egui::Margin::symmetric(16, 0)))
            .show_separator_line(false)
            .show(ui, |ui| self.top_bar(ui, &t));
        egui::Panel::bottom("status")
            .exact_size(32.0)
            .frame(egui::Frame::new().fill(t.bg_base).inner_margin(egui::Margin::symmetric(16, 0)))
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui, &t));
        let side = if i18n::rtl() { egui::Panel::left("side") } else { egui::Panel::right("side") };
        let side_margin = if i18n::rtl() { egui::Margin { left: 16, right: 4, top: 4, bottom: 8 } } else { egui::Margin { left: 4, right: 16, top: 4, bottom: 8 } };
        side.exact_size(352.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.bg_base).inner_margin(side_margin))
            .show_separator_line(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.sidebar(ui, &t));
            });
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 4, bottom: 8 }))
            .show(ui, |ui| self.stage(ui, &t));

        self.palette_ui(&ctx, &t);
        self.setup_ui(&ctx, &t);
        self.toasts_ui(&ctx, &t);
        self.extra_windows(&ctx, &t);

        // Keep the preview moving even while the window is in the background.
        ctx.request_repaint_after(Duration::from_millis(16));
        self.screenshot_step(&ctx);
    }
}

impl StudioApp {
    fn keyboard(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let (cmd_k, digits, esc, cmd_r) = ctx.input(|i| {
            let cmd_k = i.modifiers.command && i.key_pressed(egui::Key::K);
            let cmd_r = i.modifiers.command && i.key_pressed(egui::Key::R);
            let keys = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9];
            let d = keys.iter().position(|k| i.key_pressed(*k) && !i.modifiers.command && !i.modifiers.alt);
            (cmd_k, d, i.key_pressed(egui::Key::Escape), cmd_r)
        });
        if cmd_k {
            self.palette = if self.palette.is_some() { None } else { Some(Palette { query: String::new(), selected: 0, opened: Instant::now() }) };
        }
        if esc {
            self.palette = None;
        }
        if cmd_r && self.palette.is_none() {
            self.toggle_record();
        }
        if !typing && self.palette.is_none() && !egui::Popup::is_any_open(ctx) {
            if let Some(i) = digits {
                self.switch_scene(i);
            }
            // Presentation clickers send arrows / Page Up / Page Down.
            let ours = Instant::now() < self.synthetic_until;
            let (fwd, back) = ctx.input(|i| {
                if ours || !i.focused {
                    return (false, false);
                }
                (
                    i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::PageDown) || i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp),
                )
            });
            if fwd {
                self.slide(true);
            }
            if back {
                self.slide(false);
            }
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, t: &Tokens) {
        let ctx = ui.ctx().clone();
        widgets::row(ui, |ui| {
            // Wordmark: a tally ring + name.
            let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
            let live = self.recording.is_some() || self.vcam.is_some();
            ui.painter().circle_stroke(r.center(), 7.5, Stroke::new(1.5, if live { t.accent } else { t.text_secondary }));
            ui.painter().circle_filled(r.center(), 3.5, if live { t.accent } else { t.text_secondary });
            ui.add_space(2.0);
            widgets::label(ui, widgets::Txt::new("Studio Record").font(font_semibold(15.0)).color(t.text_primary));
            ui.add_space(S8);
            let scene = self.s.scenes.get(self.s.active_scene).map(|s| s.name.clone()).unwrap_or_default();
            widgets::label(ui, widgets::Txt::new(format!("/  {}", tr(&scene))).font(font(14.0)).color(t.text_tertiary));

            ui.with_layout(widgets::end(Align::Center), |ui| {
                let dark = t.dark;
                if widgets::icon_button(ui, t, if dark { Icon::Sun } else { Icon::Moon }, if dark { "Light appearance" } else { "Dark appearance" }, false).clicked() {
                    self.run(&ctx, Command::ToggleTheme);
                }
                if widgets::icon_button(ui, t, Icon::Window, "Output window — share this window in a call", self.output_window).clicked() {
                    self.output_window = !self.output_window;
                }
                let v = if self.mini { Variant::AccentOutline } else { Variant::Ghost };
                if widgets::button(ui, t, v, Some(Icon::Pin), "Control panel", None, None, 32.0)
                    .on_hover_tr("A small floating panel to switch scenes, record and go live without this window")
                    .clicked()
                {
                    self.mini = !self.mini;
                }
                let issues = self.setup_issues();
                let label = if issues > 0 { trf("Setup check · {}", &[&issues]) } else { "Setup check".to_string() };
                let r = widgets::button(ui, t, if self.setup_open { Variant::Outline } else { Variant::Ghost }, Some(if issues > 0 { Icon::Alert } else { Icon::Check }), &label, None, None, 32.0)
                    .on_hover_tr("Camera, microphone, screen and call-camera permissions in one place");
                if r.clicked() {
                    self.setup_open = !self.setup_open;
                }
                let cmd = if cfg!(target_os = "macos") { "⌘K" } else { "Ctrl K" };
                if widgets::button(ui, t, Variant::Ghost, Some(Icon::Command), "Commands", Some(cmd), None, 32.0).clicked() {
                    self.palette = Some(Palette { query: String::new(), selected: 0, opened: Instant::now() });
                }
                ui.add_space(S8);
                if let Some(rec) = &self.recording {
                    let el = rec.elapsed().as_secs();
                    let txt = trf("REC  {}", &[&format!("{:02}:{:02}:{:02}", el / 3600, (el / 60) % 60, el % 60)]);
                    tally_chip(ui, t, &txt, !rec.is_paused());
                }
                if self.mixer.hold() {
                    tally_chip(ui, t, "PAUSED", false);
                }
                if self.s.mic_muted {
                    tally_chip(ui, t, "MIC MUTED", false);
                }
                if let Some(v) = &self.vcam {
                    if let VcamStatus::On { .. } = v.status() {
                        tally_chip(ui, t, "CAMERA OUT", false);
                    }
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, t: &Tokens) {
        widgets::row(ui, |ui| {
            let n = self.s.scenes.len();
            let hint = if cfg!(target_os = "macos") {
                trf("1–{} scenes  ·  ← → slides  ·  from any app: ⌃⌥1–{} scenes, ⌃⌥← → slides, ⌃⌥P pause, ⌃⌥M mute, ⌃⌥R record", &[&n, &n])
            } else {
                trf("1–{} scenes  ·  ← → slides  ·  from any app: Ctrl+Alt+1–{}, Ctrl+Alt+← →, Ctrl+Alt+P pause, Ctrl+Alt+M mute", &[&n, &n])
            };
            widgets::label(ui, widgets::Txt::new(hint).font(mono(11.0)).color(t.text_tertiary));
            ui.with_layout(widgets::end(Align::Center), |ui| {
                let st = *self.mixer.stats.lock();
                let (w, h) = self.s.mix.size.dims();
                widgets::label(ui, widgets::Txt::new(format!("{w}×{h}  ·  {:>2.0} fps  ·  {:>4.1} ms", st.fps, st.render_ms)).font(mono(11.0)).color(t.text_tertiary));
                if let Engine::Ready { encoder, .. } = &*self.engine.lock() {
                    widgets::label(ui, widgets::Txt::new(format!("{}   ·", tr(recorder::encoder_label(encoder)))).font(mono(11.0)).color(t.text_tertiary));
                }
            });
        });
    }

    // -- the stage: program monitor + scenes ---------------------------------

    fn stage(&mut self, ui: &mut Ui, t: &Tokens) {
        self.link_bar(ui, t);
        ui.add_space(S12);
        let avail = ui.available_size();
        let scenes_h = 132.0;
        let mon_h_max = (avail.y - scenes_h - S24).max(200.0);
        let mon_w = avail.x.min(mon_h_max * 16.0 / 9.0);
        let mon_h = mon_w * 9.0 / 16.0;
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(mon_w, mon_h), Sense::hover());
            self.monitor(ui, t, rect);
        });
        ui.add_space(S20);
        self.scene_strip(ui, t, mon_w);
    }

    /// The Link bar: slide Back / Next, the address, Load, page controls.
    fn link_bar(&mut self, ui: &mut Ui, t: &Tokens) {
        let has_link = self.link.as_ref().is_some_and(|l| !l.url.is_empty());
        let h = 36.0;
        widgets::row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_enabled_ui(has_link, |ui| {
                if widgets::button(ui, t, Variant::Outline, None, "‹  Back", None, Some(84.0), h).on_hover_tr("Previous slide  (←)").clicked() {
                    self.slide(false);
                }
                if widgets::button(ui, t, Variant::Outline, None, "Next  ›", None, Some(84.0), h).on_hover_tr("Next slide  (→, Space)").clicked() {
                    self.slide(true);
                }
            });
            ui.add_space(6.0);
            let right_w = 84.0 + 6.0 + 32.0 * 4.0 + 6.0 * 4.0;
            let field_w = (ui.available_width() - right_w).max(160.0);
            let (frect, _) = ui.allocate_exact_size(vec2(field_w, h), Sense::hover());
            let focused_id = egui::Id::unique("link-field");
            let focused = ui.ctx().memory(|m| m.has_focus(focused_id));
            ui.painter().rect(
                frect,
                CornerRadius::same(R_SM + 2),
                t.bg_surface,
                Stroke::new(1.0, if focused { t.accent.gamma_multiply(0.6) } else { t.border_default }),
                StrokeKind::Inside,
            );
            if focused {
                ui.painter().rect_stroke(frect.expand(2.0), CornerRadius::same(R_SM + 4), Stroke::new(2.0, t.accent_soft), StrokeKind::Outside);
            }
            let rtl = i18n::rtl();
            let icon_c = if rtl { frect.right_center() - vec2(18.0, 0.0) } else { frect.left_center() + vec2(18.0, 0.0) };
            icons::paint(ui.painter(), Rect::from_center_size(icon_c, vec2(16.0, 16.0)), Icon::External, t.text_tertiary);
            let inner = if rtl {
                Rect::from_min_max(frect.min + vec2(8.0, 0.0), frect.max - vec2(34.0, 0.0))
            } else {
                Rect::from_min_max(frect.min + vec2(34.0, 0.0), frect.max - vec2(8.0, 0.0))
            };
            let field_align = if rtl && self.link_input.is_empty() { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER };
            let te = egui::TextEdit::singleline(&mut self.link_input)
                .id(focused_id)
                .hint_text(widgets::Txt::new("Paste a link to your slides — Google Slides, PowerPoint, Canva, a PDF…").font(font(14.0)))
                .font(font(13.0))
                .frame(egui::Frame::NONE)
                .align(field_align)
                .desired_width(inner.width());
            let resp = ui.put(inner, te);
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let load = widgets::button(ui, t, Variant::Outline, None, "Load", None, Some(84.0), h).on_hover_tr("Show this page in the Link scenes");
            if enter || load.clicked() {
                let u = self.link_input.clone();
                self.load_link(&u);
            }
            ui.add_enabled_ui(has_link, |ui| {
                if widgets::icon_button(ui, t, Icon::Refresh, "Reload the page", false).clicked() {
                    if let Some(l) = &self.link {
                        l.reload();
                    }
                }
                if cfg!(target_os = "macos") {
                    if widgets::icon_button(ui, t, Icon::Back, "Previous page", false).clicked() {
                        if let Some(l) = &self.link {
                            l.back();
                        }
                    }
                    if widgets::icon_button(ui, t, Icon::Forward, "Next page", false).clicked() {
                        if let Some(l) = &self.link {
                            l.forward();
                        }
                    }
                    let shown = self.link.as_ref().is_some_and(|l| l.shown());
                    if widgets::icon_button(ui, t, Icon::Window, "Show the page to click, scroll or sign in", shown).clicked() {
                        if let Some(l) = &mut self.link {
                            l.set_shown(!shown);
                        }
                    }
                }
            });
        });
    }

    fn monitor(&mut self, ui: &mut Ui, t: &Tokens, rect: Rect) {
        let p = ui.painter().clone();
        let recording = self.recording.is_some();
        let radius = CornerRadius::same(R);
        p.rect_filled(rect, radius, t.monitor_bg);
        if let Some(tex) = &self.program_tex {
            p.add(egui::Shape::Rect(
                egui::epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
            ));
        }

        // Explain empty slots right where they appear.
        let layout = self.s.scenes.get(self.s.active_scene).map(|s| s.layout).unwrap_or(Layout::Full(Role::Camera));
        let any_live = layout.roles().iter().any(|r| self.role_live(*r));
        for (role, b, round) in mixer::layout_boxes(layout, &self.s.mix) {
            if self.role_live(role) {
                continue;
            }
            let r = Rect::from_min_size(rect.min + vec2(b[0] * rect.width(), b[1] * rect.height()), vec2(b[2] * rect.width(), b[3] * rect.height()));
            let full = b[2] > 0.9;
            if !full || any_live {
                let rr = if round { CornerRadius::same(255) } else { CornerRadius::same(R_SM + 2) };
                p.rect(r.shrink(if full { 0.0 } else { 1.0 }), rr, Color32::from_white_alpha(6), Stroke::new(1.0, Color32::from_white_alpha(28)), StrokeKind::Inside);
            }
            let (title, detail) = self.role_message(role);
            let big = r.width() > 360.0;
            let icon_size = if big { 28.0 } else { 18.0 };
            let c = r.center();
            let ic = match role {
                Role::Camera => Icon::Camera,
                Role::Screen => Icon::Screen,
                Role::Tablet => Icon::Tablet,
                Role::Link => Icon::Link,
            };
            let fg = Color32::from_white_alpha(200);
            let fg2 = Color32::from_white_alpha(120);
            if big {
                icons::paint(&p, Rect::from_center_size(c - vec2(0.0, 36.0), vec2(icon_size, icon_size)), ic, fg2);
                widgets::paint_text(&p, c, Align2::CENTER_CENTER, &tr(&title), font_medium(16.0), fg);
                widgets::paint_text(&p, c + vec2(0.0, 24.0), Align2::CENTER_CENTER, &tr(&detail), font(13.0), fg2);
            } else {
                icons::paint(&p, Rect::from_center_size(c - vec2(0.0, 12.0), vec2(icon_size, icon_size)), ic, fg2);
                widgets::paint_text(&p, c + vec2(0.0, 10.0), Align2::CENTER_CENTER, &tr(&title), font_medium(12.0), fg);
            }
        }

        // Overlays: what is on air.
        let name = self.s.scenes.get(self.s.active_scene).map(|s| s.name.as_str()).unwrap_or("");
        let chip = trf("PROGRAM  ·  {}", &[&tr(name).to_uppercase()]);
        let g = widgets::text_galley(&p, &chip, widgets::mono_medium(11.0), Color32::from_white_alpha(220));
        let csize = g.size() + vec2(16.0, 10.0);
        let cr = if i18n::rtl() {
            Rect::from_min_size(pos2(rect.right() - 12.0 - csize.x, rect.top() + 12.0), csize)
        } else {
            Rect::from_min_size(rect.min + vec2(12.0, 12.0), csize)
        };
        p.rect_filled(cr, CornerRadius::same(R_SM), Color32::from_black_alpha(140));
        p.galley(cr.min + vec2(8.0, 5.0), g, Color32::WHITE);

        // Tally frame.
        let border = if recording { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.border_subtle) };
        p.rect_stroke(rect, radius, border, StrokeKind::Outside);
    }

    fn role_live(&self, role: Role) -> bool {
        if role == Role::Link && cfg!(target_os = "macos") {
            return self.link.as_ref().is_some_and(|l| l.slot.get().is_some());
        }
        if !self.live {
            return false;
        }
        if role == Role::Camera && !self.s.camera_on {
            return false;
        }
        if role == Role::Link && cfg!(target_os = "macos") {
            return self.link.as_ref().is_some_and(|l| l.slot.get().is_some());
        }
        self.sources.get(&role).map(|h| h.slot.get().is_some()).unwrap_or(false)
    }

    fn role_message(&self, role: Role) -> (String, String) {
        let label = role.label();
        if role == Role::Camera && !self.s.camera_on {
            return ("Video is off".into(), "Turn it back on with Video in the control panel.".into());
        }
        if role == Role::Link && cfg!(target_os = "macos") {
            return match &self.link {
                Some(l) if !l.url.is_empty() => {
                    if let Some(e) = l.error.lock().clone() {
                        ("The page didn't open".into(), e)
                    } else {
                        ("Opening the page…".into(), short_url(&l.url, 64))
                    }
                }
                _ => ("No link yet".into(), "Paste a link to your slides in the bar above and press Load.".into()),
            };
        }
        if role == Role::Link && self.s.link_window.is_none() {
            return (
                "No link window".into(),
                "Load the link (it opens in your browser), then pick that browser window under Sources → Link.".into(),
            );
        }
        let Some(id) = self.s.device_for(role) else {
            return match role {
                Role::Tablet => (
                    "No iPad yet".into(),
                    if cfg!(target_os = "macos") {
                        "Connect it with a cable and unlock it — it appears automatically.".into()
                    } else {
                        "Pick a capture adapter or display under Sources.".into()
                    },
                ),
                _ => (
                    match role {
                        Role::Camera => "No camera chosen",
                        Role::Screen => "No screen or window chosen",
                        _ => "No link chosen",
                    }
                    .into(),
                    "Choose one under Sources.".into(),
                ),
            };
        };
        if !self.live {
            let name = self.device(id).map(|d| i18n::tr_device_name(&d.name)).unwrap_or_else(|| label.to_string());
            return (name, "Starts when Studio Record opens".into());
        }
        let Some(dev) = self.device(id) else {
            return (trf("{} disconnected", &[&tr(label)]), "Plug it back in and it resumes on its own.".into());
        };
        match self.sources.get(&role).map(|h| h.state()) {
            Some(SourceState::NeedsPermission(Permission::Camera)) => ("Camera access is off".into(), "Allow Studio Record in Privacy settings.".into()),
            Some(SourceState::NeedsPermission(Permission::ScreenRecording)) => {
                ("Screen recording is off".into(), "Allow Studio Record in Privacy settings, then reopen it.".into())
            }
            Some(SourceState::Error(e)) => (trf("{} isn't available", &[&i18n::tr_device_name(&dev.name)]), e),
            Some(SourceState::Waiting(w)) => (i18n::tr_device_name(&dev.name), w),
            _ => (i18n::tr_device_name(&dev.name), "Starting…".into()),
        }
    }

    fn scene_strip(&mut self, ui: &mut Ui, t: &Tokens, width: f32) {
        let n = self.s.scenes.len().max(1);
        let gap = S12;
        let left = (ui.available_width() - width) / 2.0;
        let card_w = ((width - gap * (n as f32 - 1.0)) / n as f32).floor();
        let card_h = 112.0;
        let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), card_h), Sense::hover());
        let x0 = strip.left() + left.max(0.0);
        {
            for i in 0..n {
                let rect = Rect::from_min_size(pos2(x0 + i as f32 * (card_w + gap), strip.top()), vec2(card_w, card_h));
                let resp = ui.interact(rect, ui.scope_id().with(("scene-card", i)), Sense::click());
                let active = i == self.s.active_scene;
                let hovered = resp.hovered();
                let p = ui.painter();
                let k = ui.ctx().animate_bool_with_time(resp.id.with("act"), active, 0.18);
                p.rect(
                    rect,
                    CornerRadius::same(R),
                    if hovered && !active { t.bg_elevated } else { t.bg_surface },
                    Stroke::new(1.0 + k * 0.5, if active { t.accent } else if hovered { t.border_default } else { t.border_subtle }),
                    StrokeKind::Inside,
                );
                // Schematic thumbnail.
                let th = Rect::from_min_size(rect.min + vec2(12.0, 12.0), vec2(card_w - 24.0, ((card_w - 24.0) * 9.0 / 16.0).min(card_h - 52.0)));
                let th = Rect::from_center_size(pos2(rect.center().x, th.center().y), vec2(th.height() * 16.0 / 9.0, th.height()));
                p.rect_filled(th, CornerRadius::same(R_SM), t.bg_inset);
                let scene = &self.s.scenes[i];
                for (role, b, round) in mixer::layout_boxes(scene.layout, &self.s.mix) {
                    let r = Rect::from_min_size(th.min + vec2(b[0] * th.width(), b[1] * th.height()), vec2(b[2] * th.width(), b[3] * th.height()));
                    let full = b[2] > 0.9;
                    let r = if full { r } else { r.shrink(0.5) };
                    let fill = match role {
                        Role::Camera => if active { t.accent_soft } else { t.hover_tint },
                        _ => Color32::TRANSPARENT,
                    };
                    let rr = if round { CornerRadius::same(255) } else { CornerRadius::same(if full { R_SM } else { 3 }) };
                    if !full || role == Role::Camera {
                        p.rect(r, rr, fill, Stroke::new(1.0, if active { t.accent.gamma_multiply(0.6) } else { t.border_default }), StrokeKind::Inside);
                    }
                    let ic = match role {
                        Role::Camera => Icon::Camera,
                        Role::Screen => Icon::Screen,
                        Role::Tablet => Icon::Tablet,
                Role::Link => Icon::Link,
                    };
                    let s = (r.height() * 0.42).clamp(10.0, 18.0);
                    icons::paint(p, Rect::from_center_size(r.center(), vec2(s, s)), ic, if active { t.accent } else { t.text_tertiary });
                }
                // Name + key.
                let ty = rect.bottom() - 20.0;
                let name_c = if active { t.text_primary } else { t.text_secondary };
                if i18n::rtl() {
                    widgets::paint_text(p, pos2(rect.right() - 12.0, ty), Align2::RIGHT_CENTER, &tr(&scene.name), font_medium(13.0), name_c);
                    widgets::keycap(p, t, pos2(rect.left() + 10.0, ty), &(i + 1).to_string(), Align2::LEFT_CENTER);
                } else {
                    widgets::paint_text(p, pos2(rect.left() + 12.0, ty), Align2::LEFT_CENTER, &tr(&scene.name), font_medium(13.0), name_c);
                    widgets::keycap(p, t, pos2(rect.right() - 10.0, ty), &(i + 1).to_string(), Align2::RIGHT_CENTER);
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.switch_scene(i);
                }
            }
        }
    }

    // -- sidebar ---------------------------------------------------------------

    fn sidebar(&mut self, ui: &mut Ui, t: &Tokens) {
        ui.spacing_mut().item_spacing.y = S8;
        widgets::caption(ui, t, "Sources");
        for role in Role::DEVICES {
            self.source_card(ui, t, role);
        }
        if cfg!(target_os = "macos") {
            self.link_card(ui, t);
        } else {
            self.source_card(ui, t, Role::Link);
        }
        ui.add_space(S16);
        if !self.live && std::env::var_os("STUDIO_RECORD_TEST_AUDIO").is_some() {
            ui.scroll_to_cursor(Some(Align::Min));
        }
        widgets::caption(ui, t, "Output");
        self.output_card(ui, t);
        ui.add_space(S16);
        widgets::caption(ui, t, "Settings");
        self.look_card(ui, t);
        ui.add_space(S8);
    }

    /// macOS: the Link source (the embedded page).
    fn link_card(&mut self, ui: &mut Ui, t: &Tokens) {
        let (title, url, fps, live, loading) = match &self.link {
            Some(l) if !l.url.is_empty() => {
                let fr = l.slot.get().map(|f| (f.width, f.height));
                (l.title(), l.current_url().unwrap_or_else(|| l.url.clone()), l.fps.lock().fps(), fr, l.is_loading())
            }
            _ => (String::new(), String::new(), 0.0, None, false),
        };
        widgets::card(ui, t, |ui| {
            widgets::row(ui, |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                ui.painter().rect(r, CornerRadius::same(R_SM + 2), t.bg_inset, Stroke::new(1.0, t.border_subtle), StrokeKind::Inside);
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), Icon::Link, t.text_primary);
                ui.add_space(S4);
                widgets::col(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    widgets::label(ui, widgets::Txt::new("Link").font(font_medium(14.0)).color(t.text_primary));
                    let (dot, text, pulse) = if url.is_empty() {
                        (t.text_tertiary, "No link loaded".to_string(), false)
                    } else if loading && live.is_none() {
                        (t.warning, "Opening the page…".to_string(), true)
                    } else if let Some((w, h)) = live {
                        (t.success, trf("Live  ·  {}×{}  ·  {} fps", &[&w, &h, &format!("{fps:.0}")]), false)
                    } else {
                        (t.warning, "Waiting for the page…".to_string(), true)
                    };
                    widgets::row(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        widgets::status_dot(ui, dot, pulse);
                        widgets::label(ui, widgets::Txt::new(text).font(font(12.0)).color(t.text_secondary));
                    });
                });
            });
            if !url.is_empty() {
                let shown_title = if title.is_empty() { url.clone() } else { title };
                ui.add(egui::Label::new(widgets::Txt::new(short_url(&shown_title, 48)).font(font_medium(13.0)).color(t.text_primary)).truncate());
                ui.add(egui::Label::new(widgets::Txt::new(short_url(&url, 60)).font(mono(11.0)).color(t.text_tertiary)).truncate());
                let shown = self.link.as_ref().is_some_and(|l| l.shown());
                let label = if shown { "Hide the page window" } else { "Show page to click or sign in" };
                if widgets::button(ui, t, Variant::Outline, Some(Icon::Window), label, None, Some(ui.available_width()), 32.0).clicked() {
                    if let Some(l) = &mut self.link {
                        l.set_shown(!shown);
                    }
                }
            } else {
                widgets::label(
                    ui,
                    widgets::Txt::new("Paste a link to your slides in the bar above the preview. Next and Back move the slides — also from the control panel and ⌃⌥← →.")
                        .font(font(12.0))
                        .color(t.text_tertiary),
                );
            }
            if !self.s.recent_links.is_empty() {
                let mut pick = None;
                egui::ComboBox::from_id_salt("recent-links")
                    .selected_text(widgets::Txt::new("Recent links").font(font(13.0)))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for u in &self.s.recent_links {
                            if ui.selectable_label(false, widgets::Txt::new(short_url(u, 52)).font(font(12.0))).clicked() {
                                pick = Some(u.clone());
                            }
                        }
                    });
                if let Some(u) = pick {
                    self.load_link(&u);
                }
            }
        });
    }

    // -- setup check -------------------------------------------------------------

    fn setup_ui(&mut self, ctx: &egui::Context, t: &Tokens) {
        if !self.setup_open {
            return;
        }
        let screen = ctx.content_rect();
        let popup_was_open = egui::Popup::is_any_open(ctx);
        egui::Area::new(egui::Id::unique("setup-dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(110));
            if resp.clicked() {
                self.setup_open = false;
            }
        });
        let width = 600.0;
        let mut close = false;
        egui::Area::new(egui::Id::unique("setup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - width / 2.0, (screen.top() + 64.0).max(screen.center().y - 330.0)))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(t.bg_elevated)
                    .stroke(Stroke::new(1.0, t.border_default))
                    .corner_radius(CornerRadius::same(theme::R_LG))
                    .shadow(ui.visuals().popup_shadow)
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(width - 48.0);
                        widgets::row(ui, |ui| {
                            widgets::col(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                widgets::label(ui, widgets::Txt::new("Setup check").font(font_semibold(20.0)).color(t.text_primary));
                                widgets::label(
                                    ui,
                                    widgets::Txt::new("Everything Studio Record needs, with one button to fix each. It updates live.")
                                        .font(font(13.0))
                                        .color(t.text_secondary),
                                );
                            });
                            ui.with_layout(widgets::end(Align::Min), |ui| {
                                if widgets::button(ui, t, Variant::Outline, None, "Done", None, None, 32.0).clicked() {
                                    close = true;
                                }
                            });
                        });
                        ui.add_space(S16);
                        // Language first: it's the first thing a new person chooses.
                        widgets::row(ui, |ui| {
                            widgets::label(ui, widgets::Txt::new("Language").font(font_medium(14.0)).color(t.text_primary));
                            ui.with_layout(widgets::end(Align::Center), |ui| {
                                self.lang_picker(ui, 200.0, "lang-setup");
                            });
                        });
                        ui.add_space(S8);
                        #[cfg(target_os = "macos")]
                        if self.live && permissions::bundle_id().is_none() {
                            // A bare program can't show macOS permission dialogs:
                            // the request is charged to whatever app started it.
                            let mut reopen = false;
                            Self::setup_row(
                                ui,
                                t,
                                Icon::Alert,
                                "Open Studio Record as an app first",
                                Some(false),
                                "macOS only shows the Allow dialogs to a real app. This installs Studio Record in your Applications folder and reopens it there.",
                                |ui| {
                                    if widgets::button(ui, t, Variant::Primary, None, "Reopen as app", None, Some(128.0), 32.0).clicked() {
                                        reopen = true;
                                    }
                                },
                            );
                            if reopen {
                                if crate::macapp::relaunch_as_app_if_needed(&[]) {
                                    std::process::exit(0);
                                }
                                self.perm_note = Some("Couldn't reopen as an app. Build it with the bundle script and open Studio Record.app.".into());
                            }
                            ui.add_space(S8);
                        }
                        for k in [PermKind::Camera, PermKind::Microphone, PermKind::Screen] {
                            self.perm_row(ui, t, k);
                        }
                        self.vcam_row(ui, t);
                        self.bridge_row(ui, t);
                        if let Some(n) = &self.perm_note {
                            ui.add_space(S8);
                            widgets::label(ui, widgets::Txt::new(n).font(font(12.0)).color(t.text_tertiary));
                        }
                    });
            });
        if close || (!popup_was_open && ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.setup_open = false;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setup_row(ui: &mut Ui, t: &Tokens, icon: Icon, title: &str, ok: Option<bool>, detail: &str, add: impl FnOnce(&mut Ui)) {
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(t.bg_surface)
            .stroke(Stroke::new(1.0, t.border_subtle))
            .corner_radius(CornerRadius::same(R))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::row(ui, |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                    let c = match ok {
                        Some(true) => t.success,
                        Some(false) => t.warning,
                        None => t.text_tertiary,
                    };
                    ui.painter().rect(r, CornerRadius::same(R_SM + 2), t.bg_inset, Stroke::new(1.0, t.border_subtle), StrokeKind::Inside);
                    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), icon, t.text_primary);
                    ui.painter().circle_filled(r.right_top() + vec2(-3.0, 3.0), 4.0, c);
                    ui.add_space(S4);
                    widgets::col(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.set_max_width(290.0);
                        widgets::label(ui, widgets::Txt::new(title).font(font_medium(14.0)).color(t.text_primary));
                        widgets::label(ui, widgets::Txt::new(detail).font(font(12.0)).color(t.text_secondary));
                    });
                    ui.with_layout(widgets::end(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        add(ui);
                    });
                });
            });
    }

    fn perm_row(&mut self, ui: &mut Ui, t: &Tokens, k: PermKind) {
        let st = if self.live { self.perm(k) } else { PermStatus::NotAsked };
        let icon = match k {
            PermKind::Camera => Icon::Camera,
            PermKind::Microphone => Icon::Mic,
            PermKind::Screen => Icon::Screen,
        };
        let live_note = match k {
            PermKind::Camera => self.sources.get(&Role::Camera).and_then(|h| {
                h.slot.get().map(|f| trf("Working — {} · {}×{} · {} fps", &[&h.device.name, &f.width, &f.height, &format!("{:.0}", h.fps())]))
            }),
            PermKind::Screen => self.sources.get(&Role::Screen).and_then(|h| h.slot.get().map(|_| trf("Working — sharing {}", &[&h.device.name]))),
            PermKind::Microphone => None,
        };
        let detail = match st {
            PermStatus::Granted => live_note.unwrap_or_else(|| "Allowed".into()),
            PermStatus::NotAsked => "Not asked yet — press Allow and the system asks once.".into(),
            PermStatus::Denied => match k {
                PermKind::Screen => "Not allowed yet. Allow, switch Studio Record on in Settings, then Reopen.".into(),
                _ => "Turned off for Studio Record. Reset asks again from scratch.".into(),
            },
            PermStatus::Restricted => "Blocked by this computer's management profile.".into(),
        };
        let mut action: Option<u8> = None;
        Self::setup_row(ui, t, icon, k.label(), Some(st == PermStatus::Granted), &detail, |ui| match st {
            PermStatus::Granted => {
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Check, t.success);
            }
            PermStatus::NotAsked => {
                let can_ask = !cfg!(target_os = "macos") || !self.live || permissions::bundle_id().is_some();
                let r = ui.add_enabled_ui(can_ask, |ui| widgets::button(ui, t, Variant::Primary, None, "Allow", None, Some(92.0), 32.0)).inner;
                if r.clicked() {
                    action = Some(1);
                }
            }
            PermStatus::Denied | PermStatus::Restricted => {
                if k == PermKind::Screen {
                    if widgets::button(ui, t, Variant::Outline, None, "Reopen", None, Some(80.0), 32.0).on_hover_tr("Screen recording applies after a restart").clicked() {
                        action = Some(4);
                    }
                    if widgets::button(ui, t, Variant::Primary, None, "Allow", None, Some(80.0), 32.0).clicked() {
                        action = Some(1);
                    }
                } else {
                    if widgets::button(ui, t, Variant::Ghost, None, "Settings", None, None, 32.0).clicked() {
                        action = Some(3);
                    }
                    if widgets::button(ui, t, Variant::Primary, None, "Reset & ask", None, Some(112.0), 32.0)
                        .on_hover_tr("Clears a stuck approval and shows the system question again")
                        .clicked()
                    {
                        action = Some(2);
                    }
                }
            }
        });
        match action {
            Some(1) => {
                if k == PermKind::Screen {
                    permissions::request(k, |_| {});
                    permissions::open_settings(k);
                    self.perm_note = Some("In Settings, switch Studio Record on under Screen & System Audio Recording, then press Reopen.".into());
                } else {
                    let ctx2 = ui.ctx().clone();
                    permissions::request(k, move |_| ctx2.request_repaint());
                    self.perm_note = Some(trf("macOS is asking about the {} — answer the dialog.", &[&tr(k.label()).to_lowercase()]));
                }
            }
            Some(2) => match permissions::reset(k) {
                Ok(()) => {
                    permissions::request(k, |_| {});
                    self.perm_note = Some(trf("{} permission reset — answer the question that just appeared.", &[&tr(k.label())]));
                }
                Err(e) => {
                    self.perm_note = Some(trf("Couldn't reset ({}). Opening Settings instead.", &[&e]));
                    permissions::open_settings(k);
                }
            },
            Some(3) => permissions::open_settings(k),
            Some(4) => permissions::relaunch(),
            _ => {}
        }
        if action.is_some() {
            self.perms_at = Instant::now() - Duration::from_secs(5);
        }
    }

    fn vcam_row(&mut self, ui: &mut Ui, t: &Tokens) {
        let ready = !self.vcam_targets.is_empty();
        let on = self.vcam.as_ref().map(|v| v.status());
        let target = self.s.vcam_target.clone().or_else(|| self.vcam_targets.first().cloned()).unwrap_or_default();
        let detail = match (&on, ready) {
            (Some(VcamStatus::On { target }), _) => trf("Live — in Meet, Zoom or Teams choose the camera \"{}\".", &[target]),
            (_, true) => trf("Driver ready (\"{}\"). Turn it on, then pick it as your camera in the call.", &[&target]),
            (_, false) if cfg!(target_os = "macos") => {
                "One-time: install the free OBS Studio, open it once, click Start Virtual Camera and Allow, then quit OBS.".into()
            }
            (_, false) if cfg!(target_os = "linux") => "One-time: sudo modprobe v4l2loopback exclusive_caps=1 card_label=\"Studio Record\"".into(),
            _ => "One-time: install the free OBS Studio (it adds \u{201c}OBS Virtual Camera\u{201d}).".into(),
        };
        let mut toggle = false;
        Self::setup_row(ui, t, Icon::Broadcast, "Camera for calls", Some(ready), &detail, |ui| {
            if ready {
                let label = if on.is_some() { "Turn off" } else { "Turn on" };
                if widgets::button(ui, t, if on.is_some() { Variant::Outline } else { Variant::Primary }, None, label, None, Some(92.0), 32.0).clicked() {
                    toggle = true;
                }
            } else if widgets::button(ui, t, Variant::Outline, Some(Icon::External), "Get driver", None, None, 32.0).clicked() {
                let url = if cfg!(target_os = "linux") { "https://github.com/umlaeute/v4l2loopback#install" } else { "https://obsproject.com/download" };
                let _ = open::that(url);
            }
        });
        if toggle {
            self.toggle_vcam();
        }
    }

    fn bridge_row(&mut self, ui: &mut Ui, t: &Tokens) {
        let virt = self.outputs.iter().find(|n| audio_bridge::is_virtual_name(n)).cloned();
        let on = self.bridge.is_some();
        let detail = match (&virt, on) {
            (_, true) => "Live — choose it as the microphone in your call (Mute here mutes the call too).".to_string(),
            (Some(v), false) => trf("Optional. \"{}\" found — turn on to send your voice through Studio Record.", &[v]),
            (None, false) => "Optional. Calls can use your microphone directly. For one Mute button for everything, add a free virtual mic.".into(),
        };
        let mut toggle = false;
        Self::setup_row(ui, t, Icon::Mic, "Audio for calls", if on { Some(true) } else { None }, &detail, |ui| {
            if virt.is_some() {
                if widgets::button(ui, t, Variant::Outline, None, if on { "Turn off" } else { "Turn on" }, None, Some(92.0), 32.0).clicked() {
                    toggle = true;
                }
            } else if widgets::button(ui, t, Variant::Ghost, Some(Icon::External), "Get virtual mic", None, None, 32.0).clicked() {
                let url = if cfg!(target_os = "macos") {
                    "https://existential.audio/blackhole/"
                } else if cfg!(windows) {
                    "https://vb-audio.com/Cable/"
                } else {
                    "https://wiki.archlinux.org/title/PulseAudio/Examples"
                };
                let _ = open::that(url);
            }
        });
        if toggle {
            self.toggle_bridge();
        }
    }

    fn source_card(&mut self, ui: &mut Ui, t: &Tokens, role: Role) {
        let candidates: Vec<DeviceInfo> = self
            .devices
            .iter()
            .filter(|d| match role {
                Role::Camera => d.kind == DeviceKind::Camera,
                Role::Screen => d.kind == DeviceKind::Display || d.kind == DeviceKind::Window,
                Role::Tablet => d.kind != DeviceKind::Window,
                Role::Link => d.kind == DeviceKind::Window,
            })
            .cloned()
            .collect();
        let mut candidates = candidates;
        if role == Role::Camera {
            candidates.sort_by_key(|d| d.is_virtual);
        }
        if role == Role::Tablet {
            // iPads first, then displays (Sidecar / AirPlay), then cameras (capture adapters).
            candidates.sort_by_key(|d| match d.kind {
                DeviceKind::MobileScreen => 0,
                DeviceKind::Display => 1,
                DeviceKind::Camera => 2,
                DeviceKind::Window => 3,
            });
        }
        let current = self.s.device_for(role).clone();
        let current_dev = current.as_ref().and_then(|id| self.device(id)).cloned();
        let state = self.sources.get(&role).map(|h| h.state());
        let fps = self.sources.get(&role).map(|h| h.fps()).unwrap_or(0.0);
        let frame_dims = self.sources.get(&role).and_then(|h| h.slot.get()).map(|f| (f.width, f.height));

        widgets::card(ui, t, |ui| {
            widgets::row(ui, |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                ui.painter().rect(r, CornerRadius::same(R_SM + 2), t.bg_inset, Stroke::new(1.0, t.border_subtle), StrokeKind::Inside);
                let ic = match role {
                    Role::Camera => Icon::Camera,
                    Role::Screen => Icon::Screen,
                    Role::Tablet => Icon::Tablet,
                Role::Link => Icon::Link,
                };
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), ic, t.text_primary);
                ui.add_space(S4);
                widgets::col(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    widgets::label(ui, widgets::Txt::new(role.label()).font(font_medium(14.0)).color(t.text_primary));
                    let (dot, text, pulse) = status_line(t, self.live, &current, current_dev.as_ref(), state.as_ref(), fps, frame_dims);
                    widgets::row(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        widgets::status_dot(ui, dot, pulse);
                        widgets::label(ui, widgets::Txt::new(text).font(font(12.0)).color(t.text_secondary));
                    });
                });
                if role == Role::Camera {
                    ui.with_layout(widgets::end(Align::Center), |ui| {
                        let mut on = self.s.camera_on;
                        if widgets::switch(ui, t, &mut on).on_hover_tr("Video on / off (the camera light goes out)").changed() {
                            self.toggle_camera();
                        }
                        if widgets::icon_button(ui, t, Icon::Mirror, "Mirror the camera", self.s.mix.mirror_camera).clicked() {
                            self.s.mix.mirror_camera = !self.s.mix.mirror_camera;
                            self.push_mix_settings();
                        }
                    });
                }
            });
            ui.add_space(S4);
            let selected_text = current_dev.as_ref().map(|d| i18n::tr_device_name(&d.name)).unwrap_or_else(|| {
                if current.is_some() {
                    "Disconnected".into()
                } else {
                    "None".into()
                }
            });
            let mut choice = current.clone();
            egui::ComboBox::from_id_salt(("dev", role.label()))
                .selected_text(widgets::Txt::new(selected_text).font(font(13.0)))
                .width(ui.available_width())
                .height(360.0)
                .show_ui(ui, |ui| {
                    ui.set_min_width(280.0);
                    ui.selectable_value(&mut choice, None, widgets::Txt::new("None").color(t.text_secondary));
                    let mut last_kind = None;
                    for d in &candidates {
                        if (role == Role::Tablet || role == Role::Screen) && last_kind != Some(d.kind) {
                            last_kind = Some(d.kind);
                            ui.add_space(4.0);
                            widgets::label(
                                ui,
                                widgets::Txt::new(match d.kind {
                                    DeviceKind::MobileScreen => "iPad & iPhone (cable)",
                                    DeviceKind::Display => "Displays (Sidecar, AirPlay receiver)",
                                    DeviceKind::Camera => "Capture adapters & cameras",
                                    DeviceKind::Window => "Single windows (only that window is shared)",
                                })
                                .font(font_medium(11.0))
                                .color(t.text_tertiary),
                            );
                        }
                        let label = widgets::Txt::new(format!("{}   {}", i18n::tr_device_name(&d.name), i18n::tr_parts(&d.detail))).font(font(13.0));
                        ui.selectable_value(&mut choice, Some(d.id.clone()), label);
                    }
                    if candidates.is_empty() {
                        widgets::label(ui, widgets::Txt::new("Nothing found yet").color(t.text_tertiary));
                    }
                });
            if choice != current {
                *self.s.device_for_mut(role) = choice;
            }
            // Contextual help.
            match (&state, role) {
                (Some(SourceState::NeedsPermission(_)), _) => {
                    widgets::label(ui, widgets::Txt::new("Waiting for your permission — it starts by itself once allowed.").font(font(12.0)).color(t.text_tertiary));
                    if widgets::button(ui, t, Variant::Primary, Some(Icon::Check), "Fix in Setup check", None, Some(ui.available_width()), 32.0).clicked() {
                        self.setup_open = true;
                    }
                }
                (_, Role::Screen) if self.live && self.perm(PermKind::Screen) != PermStatus::Granted => {
                    widgets::label(
                        ui,
                        widgets::Txt::new("Allow screen recording in Setup check to share your screen or a single window.")
                            .font(font(12.0))
                            .color(t.text_tertiary),
                    );
                }
                (_, Role::Link) if current_dev.is_none() => {
                    widgets::label(
                        ui,
                        widgets::Txt::new("Load your link in the bar above — it opens in your browser. Then choose that browser window here.")
                            .font(font(12.0))
                            .color(t.text_tertiary),
                    );
                }
                (_, Role::Tablet) if current_dev.is_none() => {
                    let help = if cfg!(target_os = "macos") {
                        "Plug the iPad in with a USB cable, unlock it and tap Trust. It shows up here on its own — no app needed on the iPad."
                    } else {
                        "Connect the iPad through an HDMI capture adapter (it appears as a camera), or mirror it to an AirPlay receiver app and choose that display."
                    };
                    widgets::label(ui, widgets::Txt::new(help).font(font(12.0)).color(t.text_tertiary));
                }
                _ => {}
            }
        });
    }

    fn recording_audio_ui(&mut self, ui: &mut Ui, t: &Tokens) {
        let active = self.recording.is_some();
        let full = ui.available_width();
        widgets::caption(ui, t, "Recording audio");
        widgets::row(ui, |ui| {
            widgets::label(ui, widgets::Txt::new("Microphone").font(font_medium(13.0)).color(t.text_primary));
            ui.with_layout(widgets::end(Align::Center), |ui| {
                ui.add_enabled_ui(!active, |ui| {
                    widgets::switch(ui, t, &mut self.s.record_audio)
                        .accessible_name(tr("Record your voice with the video"))
                        .on_hover_tr("Record your voice with the video");
                });
            });
        });
        if self.s.record_audio {
            let sel = self.s.mic.clone().unwrap_or_else(|| "System default".into());
            let mut choice = self.s.mic.clone();
            ui.add_enabled_ui(!active, |ui| {
                egui::ComboBox::from_id_salt("mic")
                    .selected_text(widgets::Txt::new(sel).font(font(13.0)))
                    .width(ui.available_width())
                    .truncate()
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut choice, None, widgets::Txt::new("System default"));
                        for m in &self.mics {
                            ui.selectable_value(&mut choice, Some(m.clone()), m);
                        }
                    })
                    .response
                    .accessible_name(tr("Microphone"));
            });
            if choice != self.s.mic {
                self.s.mic = choice;
                if self.bridge.is_some() {
                    self.toggle_bridge();
                    self.toggle_bridge();
                }
            }
            let rec = self.recording.as_ref();
            Self::audio_fader(
                ui, t, "Microphone volume", &mut self.s.mic_volume,
                rec.and_then(|r| r.microphone.as_deref()), rec.is_some_and(Recording::is_paused),
            );
            if self.live && self.perm(PermKind::Microphone) != PermStatus::Granted
                && widgets::button(ui, t, Variant::Outline, None, "Allow microphone in Setup check", None, Some(full), 32.0).clicked()
            {
                self.setup_open = true;
            }
        }
        let muted = self.s.mic_muted;
        if widgets::button(
            ui, t, if muted { Variant::AccentOutline } else { Variant::Ghost },
            Some(if muted { Icon::MicOff } else { Icon::Mic }),
            if muted { "Unmute microphone" } else { "Mute microphone" },
            Some(if cfg!(target_os = "macos") { "⌃⌥M" } else { "Ctrl Alt M" }),
            Some(full), 32.0,
        ).clicked() {
            self.toggle_mute();
        }

        let supported = capture::screen_audio_supported();
        ui.separator();
        widgets::row(ui, |ui| {
            widgets::label(ui, widgets::Txt::new("Screen audio").font(font_medium(13.0)).color(t.text_primary));
            ui.with_layout(widgets::end(Align::Center), |ui| {
                ui.add_enabled_ui(supported && !active, |ui| {
                    if widgets::switch(ui, t, &mut self.s.record_screen_audio)
                        .accessible_name(tr("Record screen audio"))
                        .on_hover_tr("Record screen audio")
                        .changed()
                    {
                        self.sources.remove(&Role::Screen);
                        self.mixer.set_source(Role::Screen, None);
                    }
                });
            });
        });
        if !supported {
            widgets::label(ui, widgets::Txt::new("Screen audio requires macOS 13 or later.").font(font(12.0)).color(t.text_tertiary));
            return;
        }
        if self.s.record_screen_audio {
            if self.s.screen.as_deref().and_then(|id| self.device(id)).is_none() {
                widgets::label(ui, widgets::Txt::new("Choose a screen or window in Sources.").font(font(12.0)).color(t.text_tertiary));
            } else if self.live && self.perm(PermKind::Screen) != PermStatus::Granted
                && widgets::button(ui, t, Variant::Outline, None, "Allow screen recording in Setup check", None, Some(full), 32.0).clicked()
            {
                self.setup_open = true;
            }
            let rec = self.recording.as_ref();
            Self::audio_fader(
                ui, t, "Screen audio volume", &mut self.s.screen_audio_volume,
                rec.and_then(|r| r.screen_audio.as_deref()), rec.is_some_and(Recording::is_paused),
            );
            let muted = self.s.screen_audio_muted;
            if widgets::button(
                ui, t, if muted { Variant::AccentOutline } else { Variant::Ghost }, None,
                if muted { "Unmute screen audio" } else { "Mute screen audio" }, None, Some(full), 32.0,
            ).clicked() {
                self.s.screen_audio_muted = !muted;
                if let Some(track) = self.recording.as_ref().and_then(|rec| rec.screen_audio.as_ref()) {
                    track.set_muted(!muted);
                }
            }
        } else {
            widgets::label(ui, widgets::Txt::new("Include sound from the selected screen or window’s app.").font(font(12.0)).color(t.text_tertiary));
        }
    }

    fn audio_fader(ui: &mut Ui, t: &Tokens, name: &str, volume: &mut u8, track: Option<&InputTrack>, paused: bool) {
        ui.scope(|ui| {
            widgets::col(ui, |ui| widgets::label(ui, widgets::Txt::new(name).font(font(12.0)).color(t.text_secondary)));
            ui.spacing_mut().slider_width = (ui.available_width() - 64.0).max(48.0);
            let changed = ui.add(egui::Slider::new(volume, 0..=100).suffix("%").trailing_fill(true))
                .accessible_name(tr(name))
                .changed();
            if let Some(track) = track {
                if changed {
                    track.set_volume(*volume);
                }
                widgets::meter(ui, t, if paused { 0.0 } else { track.level() }, ui.available_width());
                match track.state() {
                    InputState::Waiting if !paused => {
                        widgets::label(ui, widgets::Txt::new("Waiting for audio…").font(font(12.0)).color(t.text_tertiary));
                    }
                    InputState::Error(message) => {
                        widgets::label(ui, widgets::Txt::new("Audio unavailable").font(font(12.0)).color(t.danger)).on_hover_tr(message);
                    }
                    _ => {}
                }
            }
        });
    }

    fn output_card(&mut self, ui: &mut Ui, t: &Tokens) {
        let ctx = ui.ctx().clone();
        widgets::card(ui, t, |ui| {
            // Recording: THE primary action.
            let engine = self.engine.lock().clone();
            let full = ui.available_width();
            if let Some(rec) = &self.recording {
                let el = rec.elapsed().as_secs();
                let label = trf("Stop recording   {}", &[&format!("{:02}:{:02}:{:02}", el / 3600, (el / 60) % 60, el % 60)]);
                if widgets::button(ui, t, Variant::AccentOutline, Some(Icon::Stop), &label, None, Some(full), 40.0).clicked() {
                    self.toggle_record();
                }
            } else if !self.finishing.is_empty() {
                widgets::button(ui, t, Variant::Outline, None, "Saving recording…", None, Some(full), 40.0);
            } else {
                let hint = if cfg!(target_os = "macos") { "⌘R" } else { "Ctrl R" };
                let enabled = matches!(engine, Engine::Ready { .. });
                let r = ui.add_enabled_ui(enabled, |ui| widgets::button(ui, t, Variant::Primary, Some(Icon::Record), "Start recording", Some(hint), Some(full), 40.0)).inner;
                if r.clicked() {
                    self.toggle_record();
                }
            }
            match engine {
                Engine::Looking => {
                    widgets::label(ui, widgets::Txt::new("Checking the recording engine…").font(font(12.0)).color(t.text_tertiary));
                }
                Engine::Missing => {
                    widgets::label(
                        ui,
                        widgets::Txt::new("Recording uses FFmpeg, a free video engine. It isn't on this computer yet.")
                            .font(font(12.0))
                            .color(t.text_secondary),
                    );
                    if widgets::button(ui, t, Variant::Outline, None, "Set up recording (≈80 MB)", None, Some(full), 32.0).clicked() {
                        let msg = Arc::new(Mutex::new("Starting…".to_string()));
                        *self.engine.lock() = Engine::Downloading(msg.clone());
                        let e = self.engine.clone();
                        let c2 = ctx.clone();
                        std::thread::spawn(move || {
                            let r = recorder::download_ffmpeg(msg);
                            *e.lock() = match r {
                                Ok(p) => {
                                    let enc = recorder::pick_encoder(&p);
                                    Engine::Ready { path: p, encoder: enc }
                                }
                                Err(err) => Engine::Failed(err),
                            };
                            c2.request_repaint();
                        });
                    }
                }
                Engine::Downloading(m) => {
                    let m = m.lock().clone();
                    widgets::label(ui, widgets::Txt::new(m).font(font(12.0)).color(t.text_secondary));
                }
                Engine::Failed(e) => {
                    widgets::label(ui, widgets::Txt::new(trf("Setup failed: {}", &[&tr(&e)])).font(font(12.0)).color(t.danger));
                }
                Engine::Ready { .. } => {}
            }

            ui.add_space(S4);
            self.recording_audio_ui(ui, t);
            ui.add_space(S4);
            ui.separator();
            ui.add_space(S4);

            // Camera output for calls.
            let mut on = self.vcam.is_some();
            widgets::row(ui, |ui| {
                icons::paint(ui.painter(), Rect::from_center_size(ui.cursor().min + vec2(9.0, 10.0), vec2(18.0, 18.0)), Icon::Broadcast, t.text_primary);
                ui.add_space(26.0);
                widgets::col(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    widgets::label(ui, widgets::Txt::new("Camera for calls").font(font_medium(14.0)).color(t.text_primary));
                    widgets::label(ui, widgets::Txt::new("Teams, Meet and Zoom see this picture").font(font(12.0)).color(t.text_tertiary));
                });
                ui.with_layout(widgets::end(Align::Center), |ui| {
                    if widgets::switch(ui, t, &mut on).changed() {
                        self.toggle_vcam();
                    }
                });
            });
            if let Some(v) = &self.vcam {
                match v.status() {
                    VcamStatus::Starting => {
                        widgets::label(ui, widgets::Txt::new("Connecting…").font(font(12.0)).color(t.text_tertiary));
                    }
                    VcamStatus::On { target } => {
                        widgets::row_wrapped(ui, |ui| {
                            widgets::status_dot(ui, t.success, false);
                            widgets::label(
                                ui,
                                widgets::Txt::new(trf("Live. In your call, pick the camera \"{}\".", &[&target]))
                                    .font(font(12.0))
                                    .color(t.text_secondary),
                            );
                        });
                    }
                    VcamStatus::NeedsDriver(msg) => {
                        widgets::label(ui, widgets::Txt::new(msg).font(font(12.0)).color(t.text_secondary));
                        if widgets::button(ui, t, Variant::Outline, Some(Icon::External), "Get the free camera driver", None, Some(full), 32.0).clicked() {
                            let url = if cfg!(target_os = "linux") {
                                "https://github.com/umlaeute/v4l2loopback#install"
                            } else {
                                "https://obsproject.com/download"
                            };
                            let _ = open::that(url);
                        }
                        widgets::label(
                            ui,
                            widgets::Txt::new(if cfg!(target_os = "macos") {
                                "After installing OBS, open it once and click Start Virtual Camera to approve the driver, then quit OBS."
                            } else {
                                "Studio Record connects automatically as soon as it's there."
                            })
                            .font(font(12.0))
                            .color(t.text_tertiary),
                        );
                    }
                    VcamStatus::Error(e) => {
                        widgets::label(ui, widgets::Txt::new(e).font(font(12.0)).color(t.danger));
                    }
                }
            }
            if self.vcam_targets.len() > 1 {
                let sel = self.s.vcam_target.clone().unwrap_or_else(|| "Automatic".into());
                let mut choice = self.s.vcam_target.clone();
                egui::ComboBox::from_id_salt("vcam-target").selected_text(widgets::Txt::new(&sel)).width(ui.available_width()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, None, widgets::Txt::new("Automatic"));
                    for n in &self.vcam_targets {
                        ui.selectable_value(&mut choice, Some(n.clone()), n);
                    }
                });
                if choice != self.s.vcam_target {
                    self.s.vcam_target = choice;
                    self.restart_vcam();
                }
            }
            // Audio for calls.
            let mut bon = self.bridge.is_some();
            widgets::row(ui, |ui| {
                icons::paint(ui.painter(), Rect::from_center_size(ui.cursor().min + vec2(9.0, 10.0), vec2(18.0, 18.0)), Icon::Mic, t.text_primary);
                ui.add_space(26.0);
                widgets::col(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    widgets::label(ui, widgets::Txt::new("Audio for calls").font(font_medium(14.0)).color(t.text_primary));
                    widgets::label(ui, widgets::Txt::new("Your voice through Studio Record, with Mute").font(font(12.0)).color(t.text_tertiary));
                });
                ui.with_layout(widgets::end(Align::Center), |ui| {
                    if widgets::switch(ui, t, &mut bon).changed() {
                        self.toggle_bridge();
                    }
                });
            });
            if let Some(b) = &self.bridge {
                match b.status() {
                    BridgeStatus::On(dev) => {
                        let mic_name = audio_bridge_input_name(&dev);
                        widgets::row_wrapped(ui, |ui| {
                            widgets::status_dot(ui, t.success, false);
                            widgets::label(ui, widgets::Txt::new(trf("Live. In your call, pick the microphone \"{}\".", &[&mic_name])).font(font(12.0)).color(t.text_secondary));
                        });
                    }
                    BridgeStatus::Starting => {
                        widgets::label(ui, widgets::Txt::new("Connecting…").font(font(12.0)).color(t.text_tertiary));
                    }
                    BridgeStatus::Error(e) => {
                        widgets::label(ui, widgets::Txt::new(e).font(font(12.0)).color(t.danger));
                    }
                }
            }
            if widgets::button(ui, t, Variant::Ghost, Some(Icon::Window), "Open output window to share", None, Some(full), 32.0)
                .on_hover_tr("A clean window with just the picture — choose it when you share a window in a call.")
                .clicked()
            {
                self.output_window = true;
            }

            ui.add_space(S4);
            ui.separator();
            ui.add_space(S4);
            widgets::row(ui, |ui| {
                widgets::label(ui, widgets::Txt::new("Quality").font(font(13.0)).color(t.text_secondary));
                ui.with_layout(widgets::end(Align::Center), |ui| {
                    let busy = self.recording.is_some();
                    ui.add_enabled_ui(!busy, |ui| {
                        let sel = if self.s.mix.size == OutputSize::P720 { 0 } else { 1 };
                        if let Some(i) = widgets::segmented(ui, t, "quality", &["720p", "1080p"], sel, 140.0) {
                            let new = if i == 0 { OutputSize::P720 } else { OutputSize::P1080 };
                            if new != self.s.mix.size && !busy {
                                self.s.mix.size = new;
                                self.push_mix_settings();
                                self.restart_sources();
                                self.restart_vcam();
                            }
                        }
                    });
                });
            });
            widgets::row(ui, |ui| {
                widgets::label(ui, widgets::Txt::new("Saves to").font(font(13.0)).color(t.text_secondary));
                ui.with_layout(widgets::end(Align::Center), |ui| {
                    if widgets::icon_button(ui, t, Icon::Folder, "Open the recordings folder", false).clicked() {
                        self.run(&ctx, Command::OpenFolder);
                    }
                    let f = self.s.folder();
                    let short = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let parent = f.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    widgets::label(ui, widgets::Txt::new(format!("{parent} / {short}")).font(mono(12.0)).color(t.text_primary)).on_hover_tr(f.display().to_string());
                });
            });
        });
    }

    fn look_card(&mut self, ui: &mut Ui, t: &Tokens) {
        widgets::card(ui, t, |ui| {
            let w = ui.available_width();
            let label_w = 92.0;
            let ctl = w - label_w - ui.spacing().item_spacing.x;
            let mut changed = false;
            let row = |ui: &mut Ui, name: &str| {
                let (r, _) = ui.allocate_exact_size(vec2(label_w, 28.0), Sense::hover());
                if i18n::rtl() {
                    widgets::paint_text(ui.painter(), r.right_center(), Align2::RIGHT_CENTER, &tr(name), font(13.0), t.text_secondary);
                } else {
                    ui.painter().text(r.left_center(), Align2::LEFT_CENTER, name, font(13.0), t.text_secondary);
                }
            };
            widgets::row(ui, |ui| {
                row(ui, "Face corner");
                // Corners are physical places: keep ↖ ↗ ↙ ↘ in that order in
                // both languages (the segmented control mirrors in Hebrew).
                let mut corners = vec![Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight];
                let mut glyphs = vec!["↖", "↗", "↙", "↘"];
                if i18n::rtl() {
                    corners.reverse();
                    glyphs.reverse();
                }
                let sel = corners.iter().position(|c| *c == self.s.mix.pip_corner).unwrap_or(3);
                if let Some(i) = widgets::segmented(ui, t, "corner", &glyphs, sel, ctl) {
                    self.s.mix.pip_corner = corners[i];
                    changed = true;
                }
            });
            widgets::row(ui, |ui| {
                row(ui, "Face size");
                let sizes = [PipSize::Small, PipSize::Medium, PipSize::Large];
                let sel = sizes.iter().position(|c| *c == self.s.mix.pip_size).unwrap_or(1);
                if let Some(i) = widgets::segmented(ui, t, "size", &["S", "M", "L"], sel, ctl) {
                    self.s.mix.pip_size = sizes[i];
                    changed = true;
                }
            });
            widgets::row(ui, |ui| {
                row(ui, "Face shape");
                let sel = if self.s.mix.pip_shape == PipShape::Rounded { 0 } else { 1 };
                if let Some(i) = widgets::segmented(ui, t, "shape", &["Rounded", "Circle"], sel, ctl) {
                    self.s.mix.pip_shape = if i == 0 { PipShape::Rounded } else { PipShape::Circle };
                    changed = true;
                }
            });
            widgets::row(ui, |ui| {
                row(ui, "Switch");
                let sel = if self.s.mix.transition == Transition::Cut { 0 } else { 1 };
                if let Some(i) = widgets::segmented(ui, t, "transition", &["Cut", "Quick fade"], sel, ctl) {
                    self.s.mix.transition = if i == 0 { Transition::Cut } else { Transition::Fade };
                    changed = true;
                }
            });
            widgets::row(ui, |ui| {
                row(ui, "Language");
                self.lang_picker(ui, ctl, "lang-settings");
            });
            if changed {
                self.push_mix_settings();
            }
        });
    }

    // -- overlays --------------------------------------------------------------

    fn palette_ui(&mut self, ctx: &egui::Context, t: &Tokens) {
        let Some(pal) = &mut self.palette else { return };
        let k = (pal.opened.elapsed().as_secs_f32() / 0.16).min(1.0);
        let ease = 1.0 - (1.0 - k).powi(3);
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("palette-dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha((90.0 * ease) as u8));
            if resp.clicked() {
                self.palette = None;
            }
        });
        let Some(pal) = &mut self.palette else { return };
        let q = pal.query.to_lowercase();
        let mut list =
            StudioCommands { scenes: self.s.scenes.iter().map(|x| x.name.clone()).collect(), rec: self.recording.is_some(), vc: self.vcam.is_some() }.build();
        list.retain(|(n, _, _)| q.is_empty() || fuzzy(&n.to_lowercase(), &q));
        let Some(pal) = &mut self.palette else { return };
        if pal.selected >= list.len() {
            pal.selected = list.len().saturating_sub(1);
        }
        let (up, down, enter) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter)));
        if down {
            pal.selected = (pal.selected + 1).min(list.len().saturating_sub(1));
        }
        if up {
            pal.selected = pal.selected.saturating_sub(1);
        }
        let mut run: Option<Command> = None;
        if enter {
            run = list.get(pal.selected).map(|c| c.2.clone());
        }
        let width = 520.0;
        egui::Area::new(egui::Id::unique("palette"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - width / 2.0, screen.top() + 96.0 - 8.0 * (1.0 - ease)))
            .show(ctx, |ui| {
                ui.set_opacity(ease);
                egui::Frame::new()
                    .fill(t.bg_elevated)
                    .stroke(Stroke::new(1.0, t.border_default))
                    .corner_radius(CornerRadius::same(theme::R_LG))
                    .shadow(ui.visuals().popup_shadow)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(width - 16.0);
                        let te = egui::TextEdit::singleline(&mut pal.query)
                            .hint_text(widgets::Txt::new("Type a command…").font(font(16.0)))
                            .font(font(16.0))
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::symmetric(12, 10))
                            .desired_width(f32::INFINITY);
                        let r = ui.add(te);
                        r.request_focus();
                        ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.border_subtle));
                        ui.add_space(6.0);
                        if list.is_empty() {
                            ui.add_space(S12);
                            ui.vertical_centered(|ui| {
                                widgets::label(ui, widgets::Txt::new("No command matches.").color(t.text_tertiary));
                            });
                            ui.add_space(S12);
                        }
                        for (i, (name, key, cmd)) in list.iter().enumerate() {
                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                            if resp.hovered() {
                                pal.selected = i;
                            }
                            let sel = i == pal.selected;
                            if sel {
                                ui.painter().rect_filled(r, CornerRadius::same(R_SM + 2), t.hover_tint.gamma_multiply(2.0));
                            }
                            let nc = if sel { t.text_primary } else { t.text_secondary };
                            if i18n::rtl() {
                                widgets::paint_text(ui.painter(), r.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, &tr(name), font(14.0), nc);
                                if let Some(k) = key {
                                    widgets::keycap(ui.painter(), t, r.left_center() + vec2(10.0, 0.0), k, Align2::LEFT_CENTER);
                                }
                            } else {
                                widgets::paint_text(ui.painter(), r.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, &tr(name), font(14.0), nc);
                                if let Some(k) = key {
                                    widgets::keycap(ui.painter(), t, r.right_center() - vec2(10.0, 0.0), k, Align2::RIGHT_CENTER);
                                }
                            }
                            if resp.clicked() {
                                run = Some(cmd.clone());
                            }
                        }
                    });
            });
        if let Some(c) = run {
            self.palette = None;
            self.run(ctx, c);
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context, t: &Tokens) {
        if self.toasts.is_empty() {
            return;
        }
        let screen = ctx.content_rect();
        let mut open_path: Option<PathBuf> = None;
        egui::Area::new(egui::Id::unique("toasts"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
            .show(ctx, |ui| {
                for toast in &self.toasts {
                    let age = toast.born.elapsed().as_secs_f32();
                    let k = (age / 0.18).min(1.0);
                    ui.set_opacity(1.0 - (1.0 - k).powi(3));
                    egui::Frame::new()
                        .fill(t.bg_elevated)
                        .stroke(Stroke::new(1.0, if toast.error { t.danger.gamma_multiply(0.6) } else { t.border_default }))
                        .corner_radius(CornerRadius::same(R))
                        .shadow(ui.visuals().popup_shadow)
                        .inner_margin(egui::Margin::symmetric(16, 10))
                        .show(ui, |ui| {
                            ui.set_max_width((screen.width() - 64.0).min(560.0));
                            widgets::row(ui, |ui| {
                                if toast.error {
                                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                                    icons::paint(ui.painter(), r, Icon::Alert, t.danger);
                                }
                                widgets::label(ui, widgets::Txt::new(&toast.text).font(font(13.0)).color(t.text_primary));
                                if let Some((label, path)) = &toast.action {
                                    if ui.link(widgets::Txt::new(label).font(font_medium(13.0)).color(t.accent)).clicked() {
                                        open_path = Some(path.clone());
                                    }
                                }
                            });
                        });
                    ui.add_space(S8);
                }
            });
        if let Some(p) = open_path {
            reveal(&p);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn extra_windows(&mut self, ctx: &egui::Context, t: &Tokens) {
        if self.output_window {
            let tex = self.program_tex.as_ref().map(|t| t.id());
            let mut close = false;
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("output-window"),
                egui::ViewportBuilder::default().with_title(tr("Studio Record — Output")).with_inner_size([960.0, 540.0]).with_min_inner_size([320.0, 180.0]),
                |ui, _| {
                    egui::CentralPanel::no_frame().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
                        let r = ui.max_rect();
                        let w = r.width().min(r.height() * 16.0 / 9.0);
                        let rr = Rect::from_center_size(r.center(), vec2(w, w * 9.0 / 16.0));
                        if let Some(id) = tex {
                            ui.painter().image(id, rr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        }
                    });
                    if ui.ctx().input(|i| i.viewport().close_requested()) {
                        close = true;
                    }
                },
            );
            if close {
                self.output_window = false;
            }
        }
        if self.mini {
            self.control_panel(ctx, t);
        }
    }

    /// The floating control panel: everything you need while presenting,
    /// without the studio window. Always on top; works while the main
    /// window is minimised.
    fn control_panel(&mut self, ctx: &egui::Context, t: &Tokens) {
        #[derive(Clone, Copy)]
        enum A {
            Scene(usize),
            Live,
            Rec,
            Hold,
            Mute,
            Video,
            Prev,
            Next,
            Studio,
        }
        let mut act: Option<A> = None;
        let mut close = false;
        let names: Vec<String> = self.s.scenes.iter().map(|s| s.name.clone()).collect();
        let active = self.s.active_scene;
        let rec = self.recording.as_ref().map(|r| (r.elapsed().as_secs(), r.is_paused()));
        let live = self.vcam.as_ref().map(|v| matches!(v.status(), VcamStatus::On { .. }));
        let hold = self.mixer.hold();
        let muted = self.s.mic_muted;
        let cam_on = self.s.camera_on;
        let has_link = self.link.as_ref().is_some_and(|l| !l.url.is_empty());
        let tile = vec2(58.0, 58.0);
        let stile = vec2(50.0, 58.0);
        let gap = 6.0;
        let sep = 14.0;
        let width = 16.0 * 2.0 + tile.x * 8.0 + stile.x * names.len() as f32 + gap * (8.0 + names.len() as f32) + sep * 3.0;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("control-panel"),
            egui::ViewportBuilder::default()
                .with_title(tr("Studio Record — Control panel"))
                .with_inner_size([width, 58.0 + 28.0])
                .with_resizable(false)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_always_on_top(),
            |ui, _| {
                egui::CentralPanel::no_frame()
                    .frame(egui::Frame::new().fill(t.bg_base).inner_margin(egui::Margin::symmetric(16, 14)))
                    .show(ui, |ui| {
                        widgets::row(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            let live_cap = match live {
                                Some(true) => "Live",
                                Some(false) => "Connecting",
                                None => "Go live",
                            };
                            if widgets::tile(ui, t, tile, Some(Icon::Live), None, live_cap, live.is_some(), false)
                                .on_hover_tr("Camera for calls — Meet, Zoom and Teams see this picture")
                                .clicked()
                            {
                                act = Some(A::Live);
                            }
                            let (rc, ri) = match rec {
                                Some((s, _)) => (format!("{:02}:{:02}", s / 60, s % 60), Icon::Stop),
                                None => ("Record".to_string(), Icon::Record),
                            };
                            if widgets::tile(ui, t, tile, Some(ri), None, &rc, rec.is_some(), false).on_hover_tr("Start / stop recording").clicked() {
                                act = Some(A::Rec);
                            }
                            if widgets::tile(ui, t, tile, Some(Icon::Pause), None, if hold { "Paused" } else { "Pause" }, hold, false)
                                .on_hover_tr("Pause: calls see a still picture and the recording pauses")
                                .clicked()
                            {
                                act = Some(A::Hold);
                            }
                            ui.add_space(sep - gap);
                            for (i, n) in names.iter().enumerate() {
                                let nt = tr(n);
                                let short: String = nt.split(" + ").next().unwrap_or(&nt).chars().take(7).collect();
                                let num = (i + 1).to_string();
                                if widgets::tile(ui, t, stile, None, Some(&num), &short, i == active, false).on_hover_tr(n).clicked() {
                                    act = Some(A::Scene(i));
                                }
                            }
                            ui.add_space(sep - gap);
                            if widgets::tile(ui, t, tile, Some(if muted { Icon::MicOff } else { Icon::Mic }), None, if muted { "Muted" } else { "Mic" }, false, muted)
                                .on_hover_tr("Mute / unmute your microphone")
                                .clicked()
                            {
                                act = Some(A::Mute);
                            }
                            if widgets::tile(ui, t, tile, Some(if cam_on { Icon::Camera } else { Icon::CameraOff }), None, if cam_on { "Video" } else { "Video off" }, false, !cam_on)
                                .on_hover_tr("Turn your camera on / off")
                                .clicked()
                            {
                                act = Some(A::Video);
                            }
                            ui.add_space(sep - gap);
                            ui.add_enabled_ui(has_link, |ui| {
                                if widgets::tile(ui, t, tile, Some(Icon::Back), None, "Back", false, false).on_hover_tr("Previous slide").clicked() {
                                    act = Some(A::Prev);
                                }
                                if widgets::tile(ui, t, tile, Some(Icon::Forward), None, "Next", false, false).on_hover_tr("Next slide").clicked() {
                                    act = Some(A::Next);
                                }
                            });
                            if widgets::tile(ui, t, tile, Some(Icon::Window), None, "Studio", false, false).on_hover_tr("Open the studio window").clicked() {
                                act = Some(A::Studio);
                            }
                        });
                    });
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            },
        );
        match act {
            Some(A::Scene(i)) => self.switch_scene(i),
            Some(A::Live) => self.toggle_vcam(),
            Some(A::Rec) => self.toggle_record(),
            Some(A::Hold) => self.toggle_hold(),
            Some(A::Mute) => self.toggle_mute(),
            Some(A::Video) => self.toggle_camera(),
            Some(A::Prev) => self.slide(false),
            Some(A::Next) => self.slide(true),
            Some(A::Studio) => {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            }
            None => {}
        }
        if close {
            self.mini = false;
            self.s.panel_on_launch = false;
        }
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.shot else { return };
        job.frames += 1;
        ctx.request_repaint();
        if job.done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let test_link = std::env::var("STUDIO_RECORD_TEST_LINK").is_ok();
        if test_link && job.frames == 300 && std::env::var("STUDIO_RECORD_TEST_NEXT").is_ok() {
            self.synthetic_until = Instant::now() + Duration::from_millis(250);
            if let Some(l) = &self.link {
                l.next();
                l.next();
            }
        }
        let shoot_at = if test_link {
            std::env::var("STUDIO_RECORD_TEST_SHOT_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(520)
        } else {
            120
        };
        if job.frames >= shoot_at && !job.requested {
            job.requested = true;
            let path = job.path.clone();
            let done = job.done.clone();
            let c2 = ctx.clone();
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                let mut buf = Vec::with_capacity(w * h * 4);
                for p in &img.pixels {
                    buf.extend_from_slice(&p.to_srgba_unmultiplied());
                }
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match image::RgbaImage::from_raw(w as u32, h as u32, buf) {
                    Some(im) => {
                        if let Err(e) = im.save(&path) {
                            eprintln!("screenshot: {e}");
                        }
                    }
                    None => eprintln!("screenshot: bad image"),
                }
                done.store(true, Ordering::SeqCst);
                c2.request_repaint();
            });
        }
        if job.frames > 1500 {
            eprintln!("screenshot: timed out");
            std::process::exit(2);
        }
        let _ = self.started;
    }
}

struct StudioCommands {
    scenes: Vec<String>,
    rec: bool,
    vc: bool,
}

impl StudioCommands {
    fn build(self) -> Vec<(String, Option<String>, Command)> {
        let mac = cfg!(target_os = "macos");
        let mut v: Vec<(String, Option<String>, Command)> =
            self.scenes.iter().enumerate().map(|(i, s)| (trf("Switch to {}", &[&tr(s)]), Some((i + 1).to_string()), Command::Scene(i))).collect();
        v.push((
            if self.rec { "Stop recording".into() } else { "Start recording".into() },
            Some(if mac { "⌘R".into() } else { "Ctrl R".into() }),
            Command::ToggleRecord,
        ));
        v.push((
            if self.vc { "Turn camera for calls off".into() } else { "Turn camera for calls on".into() },
            Some(if mac { "⌃⌥C".into() } else { "Ctrl Alt C".into() }),
            Command::ToggleVcam,
        ));
        v.push(("Open output window".into(), None, Command::OutputWindow));
        v.push(("Toggle mini controls".into(), None, Command::MiniControls));
        v.push(("Open recordings folder".into(), None, Command::OpenFolder));
        v.push(("Mirror camera".into(), None, Command::Mirror));
        v.push(("Switch light / dark".into(), None, Command::ToggleTheme));
        v.push(("Look for cameras and iPads again".into(), None, Command::Refresh));
        v.extend(Lang::ALL.iter().copied().filter(|&language| language != i18n::lang()).map(|language| {
            (trf("Change language to {}", &[&language.native()]), None, Command::SetLanguage(language))
        }));
        v.push(("Open Setup check".into(), None, Command::Setup));
        v
    }
}

fn short_url(u: &str, n: usize) -> String {
    let u = u.trim_start_matches("https://").trim_start_matches("http://");
    if u.chars().count() <= n {
        u.to_string()
    } else {
        format!("{}…", u.chars().take(n - 1).collect::<String>())
    }
}

/// The name a call app shows for the other end of a virtual cable.
fn audio_bridge_input_name(output: &str) -> String {
    let l = output.to_lowercase();
    if l.contains("cable input") {
        output.replace("Input", "Output").replace("input", "output")
    } else {
        output.to_string()
    }
}

fn fuzzy(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| c == ' ' || it.any(|h| h == c))
}

fn reveal(p: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = open::that(p.parent().unwrap_or(p));
    }
}

fn tally_chip(ui: &mut Ui, t: &Tokens, text: &str, blink: bool) {
    let g = widgets::text_galley(ui.painter(), &tr(text), widgets::mono_medium(11.0), t.accent);
    let size = vec2(g.size().x + 30.0, 24.0);
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect(r, CornerRadius::same(R_SM), t.accent_soft, Stroke::new(1.0, t.accent.gamma_multiply(0.5)), StrokeKind::Inside);
    let time = ui.input(|i| i.time);
    let on = !blink || (time * 1.2).fract() < 0.6;
    let dot_c = if on { t.accent } else { t.accent.gamma_multiply(0.3) };
    if i18n::rtl() {
        p.circle_filled(pos2(r.right() - 12.0, r.center().y), 3.5, dot_c);
        p.galley(pos2(r.right() - 22.0 - g.size().x, r.center().y - g.size().y / 2.0), g, t.accent);
    } else {
        p.circle_filled(pos2(r.left() + 12.0, r.center().y), 3.5, dot_c);
        p.galley(pos2(r.left() + 22.0, r.center().y - g.size().y / 2.0), g, t.accent);
    }
}

#[allow(clippy::too_many_arguments)]
fn status_line(
    t: &Tokens,
    live: bool,
    chosen: &Option<String>,
    dev: Option<&DeviceInfo>,
    state: Option<&SourceState>,
    fps: f32,
    dims: Option<(u32, u32)>,
) -> (Color32, String, bool) {
    if chosen.is_none() {
        return (t.text_tertiary, "Not chosen".into(), false);
    }
    let Some(dev) = dev else {
        return (t.warning, "Disconnected — waiting for it".into(), true);
    };
    if !live {
        return (t.text_tertiary, i18n::tr_parts(&dev.detail), false);
    }
    match state {
        Some(SourceState::Live) => {
            let d = dims.map(|(w, h)| format!("{w}×{h}")).unwrap_or_default();
            (t.success, trf("Live  ·  {}  ·  {} fps", &[&d, &format!("{fps:.0}")]), false)
        }
        Some(SourceState::Starting) | None => (t.warning, "Starting…".into(), true),
        Some(SourceState::Waiting(w)) => (t.warning, w.clone(), true),
        Some(SourceState::NeedsPermission(_)) => (t.danger, "Needs your permission".into(), false),
        Some(SourceState::Error(e)) => (t.danger, e.clone(), false),
        Some(SourceState::Off) => (t.text_tertiary, "Off".into(), false),
    }
}
