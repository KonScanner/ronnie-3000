# Changelog

All notable changes to Ronnie 3000 are in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/KonScanner/ronnie-3000/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/KonScanner/ronnie-3000/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/KonScanner/ronnie-3000/releases/tag/v0.1.0
