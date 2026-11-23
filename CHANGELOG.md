# Changelog

All notable changes to Ronnie 3000 are in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.4] - 2026-11-18

### Changed

- The Stage header fits the window, from the smallest window up to a wide 2560x1440 screen.
  The preset manager keeps to the right edge and grows with the window. The chain sits in the
  middle of the space that is left, and the + to add a block now follows the last gear icon.
- In the Stage layout, the account menu opens just above the account name in the bottom bar.
  Before, it opened at the top right of the window.
- The default amp and cab renders have a blank name plate, and Ronnie prints the active
  capture's name on it. Before, the plate showed a fixed sample name such as "MY-RIG-LEAD".
  The committed renders still show the sample name until `tools/render/build_skins.sh` renders
  them again.

## [0.1.3] - 2026-11-18

### Added

- On Linux, `make install-vst3` builds the VST3 plugin from source and installs it in `~/.vst3`.
  `make uninstall-vst3` removes it again.
- A Stage layout. The standalone app opens it by default, and `ronnie --layout reference`
  opens the classic layout. It has a light window
  whose header shows the signal chain as gear icons, with a + to add a block and the preset
  manager on the right, with undo and redo. The selected block fills the stage, the global
  controls and a mono/stereo lanes switch sit in a light panel, and a black utility bar holds
  the tuner, MIDI, optimize, TONE3000, settings and your account.
- In the Stage layout, a block's page shows a 3D render of its gear: a pedal, an amp or a
  cab, chosen from what the capture models. Drag the GAIN IN, OUTPUT and MIX knobs on the render
  to set the block's input gain, output gain and mix. The renders are original designs, made
  with the Blender scripts in `tools/render`.
- Tiles without artwork show the gear family of their capture (for example a chime combo, a
  British head, a 4x12 or a green overdrive), recognised from the capture's name, its
  TONE3000 makes, or the make and model in the `.nam` file.
- A cab block can blend a second impulse response (IR B) from the same tone: a BLEND control,
  per-IR level trims with 0.5 as unity, and a polarity switch. A missing or unavailable B plays
  the active IR alone.

### Changed

- The app now uses the name Ronnie everywhere. The command is `ronnie`, not `ronie`, and the
  crates are `ronnie-*`. `make install` and `install.sh` remove the old `ronie` binary.
- The presets and settings are now in `~/.config/Ronnie` on Linux,
  `~/Library/Application Support/Ronnie` on macOS, and `%APPDATA%\Ronnie` on Windows. At the
  first start, Ronnie moves the old `Ronie` folder there.
- The `.env` settings use the prefix `RONNIE_`. `script/with-env.sh` still reads the old
  prefix `RONIE_` and shows a warning.

### Fixed

- `.nam` files imported from disk lost their gear type, because Ronnie read the metadata
  from the wrong place. Ronnie now also keeps their make, model and tone type.
- `make install` builds Ronnie before it installs it. Before, it copied the last release
  binary, so the menu entry could start an old build.
- `make install` replaces the binary and the menu entry in one step each. Before, it wrote the
  menu entry two times, and GNOME could keep an old entry that started a removed binary.

## [0.1.2] - 2026-11-12

### Added

- Ronnie 3000 is also a VST3 plugin for your DAW, on Linux, macOS, and Windows. The plugin is
  experimental. The DAW project keeps the full Ronnie session of each track, with the editor
  zoom. Switches and choices show in the DAW as stepped parameters, so you can automate them.

### Fixed

- The output level no longer jumps for a moment when the signal chain changes.

## [0.1.1] - 2026-11-11

### Added

- Ronnie checks GitHub for a new release at start and once a day, and shows a notice with a
  download link. Turn the check off in Settings → System Settings. The check sends no account
  data and no key.

## [0.1.0] - 2026-11-11

The first public release.

### Added

- Play Neural Amp Modeler (`.nam`) captures of amps, pedals, and full rigs with a native Rust
  engine. Choose LITE or FULL model size for each block.
- Load impulse responses (`.wav`) for cabinets and rooms.
- Build a signal chain of pedals, amps, and cabinets. Run two lanes in stereo with Spread,
  double-track Offset, Auto Balance, and Align.
- Shape the sound with a noise gate, a three-band tone stack, and a parametric EQ with a live
  spectrum.
- Shift the pitch by up to ±24 semitones. Pick a tuning (Eb Standard, D Standard, and more) for 6- and
  7-string guitar and 4- and 5-string bass. Use a MIDI expression pedal for whammy sweeps.
- Tune with the built-in chromatic tuner.
- Browse, search, and filter the TONE3000 community library. Favorite tones and download them;
  they stay available offline.
- Connect your own TONE3000 account in the app with your personal API key.
- Save and load presets as plain files that you can back up and share.
- Map MIDI pedals and knobs to controls.
- OPTIMIZE keeps the delay as low as your system allows: it picks the pitch window for your
  instrument, sizes the input queue and the device buffer, and shows the estimated delay.
- Calibrate the input level in dBu, and normalize each block.
- Oversample 2x, 4x, or 8x for less aliasing.
- Use PipeWire, PulseAudio, ALSA, or JACK on Linux.
- Undo and redo every edit. Use the keyboard and a screen reader.
- Install on Linux with an AppImage, a tarball, or a one-line script. Experimental builds for
  macOS and Windows.

[Unreleased]: https://github.com/KonScanner/ronnie-3000/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/KonScanner/ronnie-3000/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/KonScanner/ronnie-3000/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/KonScanner/ronnie-3000/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/KonScanner/ronnie-3000/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/KonScanner/ronnie-3000/releases/tag/v0.1.0
