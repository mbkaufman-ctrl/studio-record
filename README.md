# Studio Record

Studio Record is an open-source recording studio for lessons, demos and
presentations. Put your face over your screen, switch to your iPad or a
slide deck with one key, record to MP4, or send the same picture into
Teams, Meet or Zoom as a camera.

## Download

Get the Apple silicon macOS build on the [Releases page](https://github.com/mbkaufman-ctrl/studio-record/releases).
Unzip the download, move **Studio Record.app** to Applications, then open it
and follow **Setup check**. See the release notes for macOS signing details.

The source includes Windows and Linux builds. See [Building](#building)
for platform scripts.

## First launch

The **Setup check** opens by itself. It lists the camera, microphone,
screen-and-window recording, the call camera and (optionally) audio for
calls, each with one button: **Allow**, **Reset & ask** (clears a stuck
approval) or **Reopen**. It updates live, and each source starts on its own
as soon as it's allowed, with no restart needed (macOS needs one only for
screen recording).

On macOS, Studio Record always runs as a real app. If the bare program is
started by another tool (a launcher, a terminal), macOS charges the camera
request to that tool, and when that tool doesn't declare camera use, the
request is refused with no dialog at all. So the bare program installs
itself as a signed `~/Applications/Studio Record.app` and reopens from
there (set `STUDIO_RECORD_NO_RELAUNCH=1` to opt out).

Approvals stick because the app is signed with a stable identity (see
`scripts/bundle-macos.sh`). Ad-hoc signatures change on every build, and
macOS then ignores the earlier approval.

## Languages

Studio Record is available in English and Hebrew (עברית). It follows your
computer's preferred supported language until you choose one yourself.
Use the **Language** dropdown in **Setup check** or **Settings**, or search
for the language by name in the command palette (`⌘K` / `Ctrl+K`). Your
choice applies immediately and is remembered for the next launch.

In Hebrew, the sidebar moves to the left, rows read right to left, and
Back/Next mirror. Hebrew text uses Heebo. Studio Record orders sections
of mixed text with the Unicode Bidirectional Algorithm and lets egui
shape the Hebrew letters. To add a language, register its native name,
locale codes, direction and translations in `src/i18n.rs`. Both dropdowns
and the command palette use that language list.

## Using it

1. **Sources.** Studio Record picks your camera and main screen on first
   launch. Virtual cameras are never chosen automatically.
   - **iPad (macOS):** plug it in with a cable, unlock it and tap *Trust*. It
     appears within a few seconds and is used automatically. Nothing is
     needed on the iPad.
   - **iPad (Windows/Linux):** use an HDMI capture adapter (it shows up as a
     camera), or mirror to an AirPlay receiver app and pick that display.
2. **Scenes.** Press `1`–`6` in the app, or `Ctrl+Option+1–6`
   (`Ctrl+Alt+1–6` on Windows and Linux) from any app while you present:
   Screen + Face, Face, iPad + Face, Screen, iPad, iPad beside Face.
3. **Record.** Press `⌘R` (`Ctrl+R` on Windows and Linux) or *Start recording*.
   Files go to `Movies/Studio Record` (or `Videos/Studio Record`) as
   `Lecture <date> at <time>.mp4`, with your microphone. Recordings in
   progress are written as Matroska (`.mkv`), so a crash never loses a
   lecture.
4. **Calls.** Turn on *Camera for calls* (`Ctrl+Option+C`, or `Ctrl+Alt+C`
   on Windows and Linux) and pick that camera in Teams/Meet/Zoom. Or open
   the *Output window* and share it as a window.
5. **Control panel.** A floating, always-on-top panel with Go live, Record,
   Pause, the numbered scenes, Mic (mute), Video (camera on/off), slide
   Back/Next and Studio (reopens this window). It keeps working while the
   main window is minimised.
6. **Links.** Paste a link to your slides in the bar above the preview and
   press Load. The page shows in the *Link* and *Link + Face* scenes.
   *Back* and *Next* (or `←` `→`, a presentation clicker, or `⌃⌥←` `⌃⌥→`
   from any app) move the slides. *Show page* lets you click or sign in.
   On Windows and Linux the link opens in your browser; choose that
   browser window under Sources → Link.
7. **Single windows.** Under *Screen or window*, pick one app window: only
   that window is shared, even when something covers it. On macOS, Studio
   Record's own windows are always left out of a shared screen.
8. **Pause** (`⌃⌥P`): calls see a calm still with a pause mark, and the
   recording pauses. **Mute** (`⌃⌥M`) mutes the recording and the
   audio for calls.
9. `⌘K` / `Ctrl+K` opens the command palette.

## Audio for calls (optional)

A call can use your microphone directly. If you'd rather have one Mute
button for everything, install a free virtual microphone (BlackHole on
macOS, VB-CABLE on Windows, a PulseAudio/PipeWire null sink on Linux). Turn
on *Audio for calls*, then pick that device as the microphone in the call.

## Camera for calls: one-time driver

An app can't install a camera driver on its own, so Studio Record writes
into a free, open-source virtual camera driver:

| System  | Driver | Setup |
|---------|--------|-------|
| macOS   | Any Camera Extension with an input ("sink") stream, such as **OBS Virtual Camera** | Install OBS Studio, open it once, click *Start Virtual Camera* to approve, then quit OBS. |
| Windows | **OBS Virtual Camera** | Install OBS Studio. |
| Linux   | **v4l2loopback** | `sudo apt install v4l2loopback-dkms && sudo modprobe v4l2loopback exclusive_caps=1 card_label="Studio Record"` |

In your call, choose that camera (for example "OBS Virtual Camera").

## Recording engine

Recording uses FFmpeg with the computer's hardware encoder (VideoToolbox,
NVENC, Quick Sync, AMF or Media Foundation), falling back to x264.
Studio Record looks for FFmpeg next to the app, in its data folder, on
`PATH`, and in Homebrew's folder. If it isn't found, *Set up recording*
downloads a static build with the system's `curl` and `tar`.

## Building

Use Rust 1.95 or later. Run the commands from the repository root.

```sh
cargo build --locked --release
./scripts/bundle-macos.sh        # dist/Studio Record.app (camera/mic permission prompts need the bundle)
./scripts/package-linux.sh       # needs libasound2-dev libudev-dev libxkbcommon-dev pkg-config
./scripts/package-windows.ps1
```

Developer flags:

- `--screenshot out.png` draws about 120 frames, saves the window and quits.
  It never opens devices.
- `--selftest [seconds]` runs discovery, capture, the mixer and a short
  recording without a window, and prints a report.

## How it works

- `capture/`: one thread per source, each keeping only its newest frame.
  macOS uses AVFoundation (cameras, cable-connected iPads after opting in
  with `kCMIOHardwarePropertyAllowScreenCaptureDevices`, and displays).
  Windows and Linux use nokhwa (Media Foundation / V4L2) and xcap
  (DXGI / PipeWire / X11).
- `mixer.rs`: a fixed-rate compositor with picture-in-picture,
  side-by-side, a 200 ms fade, SIMD scaling and a reused frame pool.
- `recorder.rs`: an FFmpeg pipe plus a WAV of the microphone, joined into
  MP4 when you stop. Video is paced by the wall clock, so it can't drift
  away from the voice.
- `vcam/`: CoreMediaIO sink stream on macOS, the OBS shared-memory queue on
  Windows, and v4l2loopback on Linux.

## License

Studio Record is available under your choice of the [MIT License](LICENSE-MIT)
or [Apache License 2.0](LICENSE-APACHE). The bundled Geist, Geist Mono and
Heebo fonts are covered by the [SIL Open Font License](assets/fonts).
