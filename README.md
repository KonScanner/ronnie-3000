<div align="center">

<img src="assets/ronnie-3000-256.png" alt="Ronnie 3000 logo" width="144" height="144">

# Ronnie 3000

**A fast, native guitar and bass amp rig for your computer.**<br>
Play Neural Amp Modeler captures and impulse responses, browse thousands of free tones from the
TONE3000 community, and keep the delay low enough to forget it is there.

[![CI](https://github.com/KonScanner/ronnie-3000/actions/workflows/ci.yml/badge.svg)](https://github.com/KonScanner/ronnie-3000/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/KonScanner/ronnie-3000?include_prereleases&sort=semver&label=release)](https://github.com/KonScanner/ronnie-3000/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-555)](#install)
[![Written in Rust](https://img.shields.io/badge/written%20in-Rust-b7410e?logo=rust)](https://www.rust-lang.org)

[**Download**](https://github.com/KonScanner/ronnie-3000/releases/latest) ·
[Features](#features) ·
[Install](#install) ·
[First steps](#first-steps) ·
[FAQ](#faq)

<img src="assets/screenshots/main.png" alt="Ronnie 3000 with the parametric EQ over a live spectrum" width="100%">

</div>

## Why Ronnie?

- **It sounds like the real thing.** Ronnie plays `.nam` captures of real amps, pedals, and full
  rigs, and `.wav` impulse responses of real cabinets and rooms.
- **It feels like the real thing.** OPTIMIZE tunes the audio path for the lowest delay that your
  system can play without dropouts, and it shows you the result in milliseconds.
- **Your next tone is one search away.** Browse, filter, and download from the TONE3000 community
  library without leaving the app. Downloaded tones stay on your computer and work offline.
- **It is free and open.** MIT licensed, with no subscription and no telemetry. Ronnie goes online
  only for the community browser and for a daily check for a new version, which you can turn off.

## Features

<table>
<tr>
<td width="50%" valign="top">

**Tone**

- Neural Amp Modeler (`.nam`) captures with LITE and FULL model sizes
- Impulse responses (`.wav`) for cabinets and rooms
- Drag-and-drop signal chain of pedals, amps, and cabinets
- Two lanes in stereo with Spread, double-track Offset, Auto Balance, and Align
- Noise gate, three-band tone stack, and parametric EQ with a live spectrum
- Input calibration in dBu and per-block normalization
- 2x, 4x, or 8x oversampling

</td>
<td width="50%" valign="top">

**Play**

- Pitch shift of ±24 semitones with a tuning picker: Eb Standard, D Standard, C Standard, and more
- Guitar (6 and 7 strings) and bass (4 and 5 strings)
- Whammy sweeps from a MIDI expression pedal
- Chromatic tuner
- MIDI mapping for pedals and knobs
- OPTIMIZE: the lowest delay for your instrument and your system, with a live readout
- Presets as plain files that you can back up and share

</td>
</tr>
<tr>
<td width="50%" valign="top">

**Community**

- Search the TONE3000 library by title, gear, tag, or creator
- Filter by format, make, and calibration, and sort the results
- Favorites and your recent tones in your profile
- Downloads stay available offline

</td>
<td width="50%" valign="top">

**Everyday use**

- Undo and redo for every edit
- Full keyboard control and screen reader support
- Hover help with shortcuts, and a CPU meter
- PipeWire, PulseAudio, ALSA, and JACK on Linux

</td>
</tr>
</table>

<div align="center">
<img src="assets/screenshots/pitch.png" alt="The pitch panel with the tuning picker and OPTIMIZE" width="49%">
<img src="assets/screenshots/browser.png" alt="The community browser with search results" width="49%">
</div>

## Install

Download the file for your system from the
[latest release](https://github.com/KonScanner/ronnie-3000/releases/latest). You do not need Rust
or any build tools.

| System | Download | Status |
|---|---|---|
| Linux x86_64 | `…-linux-x86_64.AppImage` or `.tar.gz` | Supported |
| Linux ARM64 | `…-linux-aarch64.AppImage` or `.tar.gz` | Supported |
| macOS 11 or later (Apple silicon and Intel) | `…-macos-universal.zip` | Experimental |
| Windows 10 and 11 (x86_64) | `…-windows-x86_64.zip` | Experimental |

### Linux

**One line**, which installs Ronnie for your user with a menu entry and an icon (no sudo):

```bash
curl -fsSL https://raw.githubusercontent.com/KonScanner/ronnie-3000/main/install.sh | sh
```

The script checks the download against the `SHA256SUMS` file of the release. Remove Ronnie again
with `sh install.sh --uninstall`.

**AppImage**, one file that runs on most distributions:

```bash
chmod +x ronnie-3000-*-linux-x86_64.AppImage
./ronnie-3000-*-linux-x86_64.AppImage
```

### macOS (experimental)

1. Unzip the download and move **Ronnie 3000** to your Applications folder.
2. The app has no Apple signature yet, so macOS blocks the first start. Open **System Settings →
   Privacy & Security**, and click **Open Anyway** next to the message about Ronnie 3000.
3. Allow the microphone access when macOS asks. Ronnie needs it to hear your audio interface.

### Windows (experimental)

1. Extract the zip, and open **Ronnie 3000.exe**.
2. The app has no Microsoft signature yet. If SmartScreen stops it, click **More info → Run
   anyway**.

### VST3 plugin (experimental)

Ronnie 3000 also runs as a VST3 effect in your DAW. Download the `ronnie-3000-vst3-…` zip for
your system from the same release.

1. Close your DAW.
2. Copy the folder **Ronnie 3000.vst3** to the VST3 folder of your system:
   - Linux: `~/.vst3`
   - macOS: `~/Library/Audio/Plug-Ins/VST3`
   - Windows: `C:\Program Files\Common Files\VST3`
3. Open your DAW, scan for new plugins, and add **Ronnie 3000** to a track.

The DAW project keeps the full Ronnie session of each track, with the editor zoom. Switches and
choices show in the DAW as stepped parameters, so you can automate them. The plugin is tested in
REAPER on Linux. Reports from other DAWs are welcome in the issues.

### Build from source

With [Rust](https://rustup.rs) installed, on Linux:

```bash
git clone https://github.com/KonScanner/ronnie-3000.git
cd ronnie-3000
make setup
```

`make setup` builds Ronnie and installs it under `~/.local`. On a computer without Rust, it
downloads the latest release instead. See [CONTRIBUTING.md](CONTRIBUTING.md) for the build tools.

## First steps

1. **Choose your audio interface.** Open **Settings → System Settings**, select your input and output,
   and play a note to find the input channel.
2. **Load a tone.** Drop a `.nam` file on an amp or pedal block, and a `.wav` impulse response on
   the cabinet block. Or open the community browser and pick a tone.
3. **Leave OPTIMIZE on.** It sits in the pitch panel and keeps the delay as low as your system
   allows. The readout under it shows the estimated delay.

### Connect your TONE3000 account

The community browser uses your own TONE3000 account. The first time you click **Sign in**,
Ronnie asks for your personal API key:

1. Sign in at [tone3000.com](https://www.tone3000.com).
2. Open **Settings → API Keys**, and create a key. It starts with `t3k_pub_`.
3. In Ronnie, click **Sign in**, paste the key, and click **Save**. Then sign in to TONE3000 in
   your browser.

The key stays on your computer. Remove it at any time in **Settings → System Settings →
Community**.

## Low-latency tips

- Keep **OPTIMIZE** on. It tries buffers of 64, 128, and 256 frames, steps up only after
  dropouts, and remembers the result for each device.
- On Linux with PipeWire, a quantum of 64 or 128 frames gives a good result for most interfaces.
  JACK users set the buffer in JACK.
- Use the inputs of a real audio interface. Built-in sound cards often add delay.
- Turn oversampling off (OPTIMIZE does this for you) on a slow computer.

## FAQ

**Is Ronnie free?** Yes. Ronnie is open source under the MIT License.

**How do I update?** Ronnie checks GitHub once a day and tells you when a new version is out. On
Linux, run the one-line install command again, or download the new AppImage. On macOS and
Windows, download the new zip. Your presets and settings stay. Turn the check off in
**Settings → System Settings → Check for updates**.

**Do I need a TONE3000 account?** Only for the community browser. Local `.nam` and `.wav` files
work without an account and without the internet.

**Is there a plugin for my DAW?** Yes, an experimental VST3 plugin. See
[VST3 plugin](#vst3-plugin-experimental). A CLAP version comes later.

**Where are my presets and settings?** In `~/.config/Ronie` on Linux,
`~/Library/Application Support/Ronie` on macOS, and `%APPDATA%\Ronie` on Windows.

**Something sounds wrong or crackles.** Open an issue with the bug report form, and attach the
diagnostic log from **Settings → Plugin Settings → Diagnostics → Copy Logs**.

## Roadmap

- A CLAP version of the plugin, and tests in more DAWs
- MIDI program change for presets
- A tuner reference pitch (for example A = 432 Hz or 442 Hz), and drop tunings in the tuning picker
- Signed builds for macOS and Windows

Ideas and votes are welcome in the issues.

## Inspired by TONE3000

Ronnie 3000 is a love letter to [TONE3000](https://www.tone3000.com), the community where players
share free captures of their amps, pedals, and cabinets. Ronnie takes its inspiration from the
TONE3000 desktop experience and is written again from the ground up in Rust, with its own audio
engine, interface, and look. Thank you to the TONE3000 team and every creator who shares their
tones.

> [!NOTE]
> Ronnie 3000 is an independent, unofficial project. It is **not affiliated with, endorsed by, or
> sponsored by TONE3000** or its community and development team. TONE3000 and related names are
> trademarks of their owners. Ronnie connects to TONE3000 only through its public API, with your
> own account and API key.

## Acknowledgements

- [Neural Amp Modeler](https://github.com/sdatkinson/neural-amp-modeler) by Steven Atkinson
  created the `.nam` capture format that Ronnie plays.
- [egui](https://github.com/emilk/egui) draws the interface, and
  [cpal](https://github.com/RustAudio/cpal) talks to the audio devices.
- The [Arimo](https://fonts.google.com/specimen/Arimo) and
  [Roboto Mono](https://fonts.google.com/specimen/Roboto+Mono) fonts are under the Apache License
  2.0. Their license files are next to the fonts.

## Contributing

Bug reports, tests on real hardware, and pull requests are welcome. Read
[CONTRIBUTING.md](CONTRIBUTING.md) to start, and [SECURITY.md](SECURITY.md) to report a security
problem in private.

## License

Ronnie 3000 is available under the [MIT License](LICENSE).
