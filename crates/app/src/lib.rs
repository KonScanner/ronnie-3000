//! Standalone application support for Ronnie.

/// Real-time audio device discovery, streams, and callback ownership.
pub mod audio;
/// Persistent standalone audio and MIDI settings.
pub mod audio_settings;
/// Production OAuth configuration and session restoration.
pub mod auth;
/// The diagnostic log file of the standalone app.
pub mod diagnostics;
/// Opens a folder of the app in the file manager of the desktop.
pub mod file_manager;
/// MIDI input filtering and merge routing.
pub mod midi;
/// Native window capability probes used by the toolkit spike.
pub mod platform;
/// Standalone UI, audio-manager, and settings adapter.
pub mod standalone_backend;
