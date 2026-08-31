<p align="center">
  <img src="site/autocut.webp" alt="autocut with a clip loaded, silent regions marked red on the timeline" width="700">
</p>

<p align="center">
  Drop a video in, the silences come out.<br>
  Export an MP4, or hand the cut timeline to DaVinci Resolve and Premiere.
</p>

<p align="center">
  <a href="https://github.com/cobanov/autocut/releases/latest"><img alt="release" src="https://img.shields.io/github/v/release/cobanov/autocut?color=0874f7&labelColor=1a1a1a"></a>
  <a href="https://github.com/cobanov/autocut/actions/workflows/ci.yml"><img alt="ci" src="https://img.shields.io/github/actions/workflow/status/cobanov/autocut/ci.yml?branch=main&color=0874f7&labelColor=1a1a1a"></a>
  <img alt="tests" src="https://img.shields.io/badge/tests-108-0874f7?labelColor=1a1a1a">
  <img alt="platforms" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows-shipping-0874f7?labelColor=1a1a1a">
</p>

---

Cutting the pauses out of a talking head video is mechanical work: find the gap,
drag the edges in, ripple everything left, do it two hundred more times. autocut
is a desktop app that does it in one pass. The footage stays on your disk, there
is nothing to install beside it (no Python, no ffmpeg), and what comes out is
either a finished MP4 or a timeline your NLE opens without a relink dialog.

```sh
brew install --cask cobanov/tap/autocut
```

- **The sliders are instant.** The speech model reads the track once, so tuning
  reshapes a result autocut already holds instead of re-running anything.
- **The preview is the export.** The player skips removed regions as it plays.
- **Multi-track shoots stay in sync.** Other angles and a separate sound recorder
  are cut at the same points, aligned by their embedded timecode.
- **FCPXML that relinks itself.** Source timecode is preserved, so DaVinci and
  Premiere bind each clip to its original media.
- **Nothing is uploaded.** Silero V5 runs locally, ffmpeg ships inside the app.

## Install

**macOS, Apple Silicon.** Homebrew is the easier path, because it clears the
Gatekeeper quarantine flag for you:

```sh
brew install --cask cobanov/tap/autocut
```

Installing the `.dmg` from [the latest release][latest] by hand works too: drag
**autocut** into Applications, then run `xattr -cr /Applications/autocut.app`
once, or macOS calls the app damaged and refuses to open it. The bundle is not
notarized yet, and that command is the whole difference between the two paths.

**Windows, x86_64.** `autocut_X.Y.Z_x64-setup.exe` (NSIS) or the `.msi`, from
[the latest release][latest]. Unsigned, so SmartScreen wants **More info**, then
**Run anyway** on first launch.

**Linux.** Build from source for now.

## Use

1. **Drop a video** on the window. MP4, MOV, MKV, WebM and AVI all work.
2. **Hit *detect silences***. Spoken regions turn green, silent ones red.
3. **Watch the preview.** Space plays and pauses, and what you hear is what
   exports.
4. **Refine.** Drag the green edge handles, type exact times in the **cuts**
   panel, or click the x on a row to park a keep without deleting it.
5. **Export** an **MP4** to send, or an **FCPXML** to keep editing.

| Slider | Default | What it does |
| --- | --- | --- |
| `threshold` | `0.50` | How sure the model must be that a chunk is speech. Higher cuts more |
| `pad` | `0.30s` | Breathing room kept on both sides of every spoken region |
| `min silence` | `100ms` | Shorter gaps are not worth cutting, so they stay in |
| `min speech` | `150ms` | Shorter bursts are dropped, which kills clicks and lip noise |

Hold **shift** for fine steps. Scroll the timeline to pan, drag in the navigator
below it to zoom. The first detect on a long video takes a moment, because that
is the model reading the whole track; every re-detect after it is free.

## Multi-track shoots

Two cameras and a recorder on the table are one performance, and they have to be
cut identically or they drift apart. Drop your **main camera** first: that is the
reference, the thing you preview and everything else lines up against.

In the **export** panel, **+ add** under *linked tracks* takes the other angles
and audio files. Files carrying timecode are aligned from it (`tc`); files
without it are assumed to have started together (`≈`), so type the real offset if
they did not. If you recorded clean sound separately, point **listen to** in the
detection panel at it, and the analysis reads the good microphone while the cuts
still apply everywhere.

Export **FCPXML** and the main camera lands on V1, the other video above it, the
audio below, every track cut at the same frames. MP4 export still writes one file
from the reference clip: a flat video has one picture and one mix, and choosing
which angle is on top is an edit, not a cut.

## Development

Svelte 5 and TypeScript in front, Rust and Tauri 2 behind, with ffmpeg and
ffprobe as bundled sidecars.

```sh
pnpm install
pnpm tauri:dev                                # the first build fetches ~200MB of ffmpeg

pnpm test                                     # vitest, the cut algebra
cd src-tauri && AUTOCUT_STUB_SIDECARS=1 cargo test --lib
```

`AUTOCUT_STUB_SIDECARS=1` drops empty placeholders where the ffmpeg binaries go,
so a fresh checkout runs the unit tests without fetching them first. Never set it
for a real build: the bundle cannot process video, and it says so at compile time.

The version lives in `package.json`. `tauri.conf.json` reads it from there, and CI
checks that `src-tauri/Cargo.toml` agrees.

## Built by

[mert cobanov](https://cobanov.dev) · 2026

[latest]: https://github.com/cobanov/autocut/releases/latest
