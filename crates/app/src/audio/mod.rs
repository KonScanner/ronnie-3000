//! Audio device backends and callback-exclusive processing.

mod buffer_optimizer;
mod cpal_backend;
mod delay;
mod device_list;
mod devices;
mod engine;
mod spectrum;
mod telemetry;
mod tuner;
mod worker;

pub use cpal_backend::{CpalBackend, CpalStream};
pub use device_list::{
    COMMON_SAMPLE_RATES, MAX_DEVICE_CHANNELS, RawDevice, ResolvedDevice, ResolvedRoute,
    SYSTEM_DEFAULT_DEVICE, automatic_input, clean_alsa_devices, clean_server_devices,
    resolve_route, system_default_label,
};
pub use devices::{
    AudioBackend, AudioDeviceManager, AudioStream, DeviceCapabilities, DeviceDescriptor,
    DummyBackend, DummyLimits, DummyStream, NegotiatedStream, Route,
};
#[cfg(test)]
pub(crate) use engine::PrepareChain;
pub use engine::{AudioBackendFactory, AudioEngine, AudioEngineStatus, new_backend};
pub use spectrum::LiveSpectrum;
pub use telemetry::{AudioEvent, AudioEvents, AudioTelemetry, FailedBlocksReport, NonFiniteReport};
pub use tuner::LiveTuner;
pub use worker::{
    AudioWorker, HEAR_YOURSELF_FADE_MILLISECONDS, TEST_TONE_FADE_MILLISECONDS, TEST_TONE_HERTZ,
    TEST_TONE_MILLISECONDS, TEST_TONE_PEAK,
};

use std::time::Duration;

use ronie_core::PropertiesError;
use ronie_dsp::ChainError;
use thiserror::Error;

/// Audio backend selection, plus a deterministic test backend.
///
/// [`Self::Alsa`] and [`Self::Jack`] exist only on Linux. On another operating system, these
/// kinds stay in the type, but [`Self::is_available`] is false for them, and a backend refuses to
/// open them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AudioBackendKind {
    /// The sound server of the system. On Linux, this is its PulseAudio socket, and without an
    /// answer from that socket, this route runs on ALSA. On macOS, this is CoreAudio. On Windows,
    /// this is WASAPI.
    #[default]
    System,
    /// Advanced Linux Sound Architecture. Only on Linux.
    Alsa,
    /// JACK Audio Connection Kit. Only on Linux.
    Jack,
    /// In-memory backend used by tests and diagnostics.
    Dummy,
}

/// Whether this build has the Linux drivers.
const LINUX: bool = cfg!(target_os = "linux");

impl AudioBackendKind {
    /// The drivers of this operating system that open real devices, in the order that the
    /// settings show them.
    #[must_use]
    pub const fn real_drivers() -> &'static [Self] {
        real_drivers_on(LINUX)
    }

    /// The drivers of this operating system, the dummy driver included.
    #[must_use]
    pub const fn available() -> &'static [Self] {
        available_on(LINUX)
    }

    /// Whether this operating system has this driver.
    #[must_use]
    pub const fn is_available(self) -> bool {
        is_available_on(self, LINUX)
    }

    /// The kind for the value of the `--backend` flag.
    ///
    /// # Errors
    /// Returns an error for an unknown value, and for a driver that this operating system does
    /// not have.
    pub fn from_flag(value: &str) -> Result<Self, AudioError> {
        from_flag_on(value, LINUX)
    }

    /// The values of the `--backend` flag on this operating system, for the help text.
    #[must_use]
    pub const fn flag_values() -> &'static str {
        if LINUX {
            "system|alsa|jack|dummy"
        } else {
            "system|dummy"
        }
    }

    /// The name that the settings show and save.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Alsa => "ALSA",
            Self::Jack => "JACK",
            Self::Dummy => "Dummy",
        }
    }

    /// The kind for a saved or shown name. Another text means the default route.
    ///
    /// A name of a driver that this operating system does not have also means the default route,
    /// so that a setting from Linux opens the system driver on macOS and Windows.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        from_name_on(name, LINUX)
    }
}

const fn real_drivers_on(linux: bool) -> &'static [AudioBackendKind] {
    use AudioBackendKind::{Alsa, Jack, System};
    if linux {
        &[System, Alsa, Jack]
    } else {
        &[System]
    }
}

const fn available_on(linux: bool) -> &'static [AudioBackendKind] {
    use AudioBackendKind::{Alsa, Dummy, Jack, System};
    if linux {
        &[System, Alsa, Jack, Dummy]
    } else {
        &[System, Dummy]
    }
}

const fn is_available_on(kind: AudioBackendKind, linux: bool) -> bool {
    match kind {
        AudioBackendKind::System | AudioBackendKind::Dummy => true,
        AudioBackendKind::Alsa | AudioBackendKind::Jack => linux,
    }
}

fn from_name_on(name: &str, linux: bool) -> AudioBackendKind {
    let kind = match name {
        "ALSA" | "Alsa" | "alsa" => AudioBackendKind::Alsa,
        "JACK" | "Jack" | "jack" => AudioBackendKind::Jack,
        "Dummy" | "dummy" => AudioBackendKind::Dummy,
        _ => AudioBackendKind::System,
    };
    if is_available_on(kind, linux) {
        kind
    } else {
        AudioBackendKind::System
    }
}

fn from_flag_on(value: &str, linux: bool) -> Result<AudioBackendKind, AudioError> {
    let kind = match value {
        "system" => AudioBackendKind::System,
        "alsa" => AudioBackendKind::Alsa,
        "jack" => AudioBackendKind::Jack,
        "dummy" => AudioBackendKind::Dummy,
        _ => {
            return Err(AudioError::InvalidConfig(format!(
                "unknown backend {value}. Use {}.",
                flag_names(linux)
            )));
        }
    };
    if is_available_on(kind, linux) {
        Ok(kind)
    } else {
        Err(AudioError::InvalidConfig(format!(
            "the {value} backend is only on Linux. Use {}.",
            flag_names(linux)
        )))
    }
}

/// The values of the `--backend` flag on this operating system, for an error message.
fn flag_names(linux: bool) -> &'static str {
    if linux {
        "system, alsa, jack, or dummy"
    } else {
        "system or dummy"
    }
}

/// Requested input/output configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceConfig {
    /// Backend that owns the stream.
    pub backend: AudioBackendKind,
    /// Identity of the input device. `None` means no choice was made, so that the automatic order
    /// of [`resolve_route`] applies. [`SYSTEM_DEFAULT_DEVICE`] follows the system default.
    pub input_device: Option<String>,
    /// Identity of the output device. `None` follows the system default.
    pub output_device: Option<String>,
    /// The label of the input device when the user chose it, for a message after it is gone.
    pub input_label: Option<String>,
    /// The label of the output device when the user chose it, for a message after it is gone.
    pub output_label: Option<String>,
    /// Requested sample rate, or an automatic device-dependent choice.
    pub sample_rate: Option<u32>,
    /// Requested callback size. `None` asks for [`DEFAULT_BUFFER_FRAMES`].
    pub buffer_frames: Option<u32>,
    /// Number of selected input channels, which is the layout that the chain receives.
    pub input_channels: usize,
    /// Number of selected output channels, which is the layout that the chain makes.
    pub output_channels: usize,
    /// Device channels that feed the chain, from zero. A mono route uses only the first.
    pub input_route: [usize; 2],
    /// First device channel of the output pair, from zero.
    pub output_first: usize,
}

/// Callback size for a user with no saved choice. It keeps the delay low enough to play through.
pub const DEFAULT_BUFFER_FRAMES: u32 = 256;

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            backend: AudioBackendKind::System,
            input_device: None,
            output_device: None,
            input_label: None,
            output_label: None,
            sample_rate: None,
            buffer_frames: None,
            input_channels: 1,
            output_channels: 2,
            input_route: [0, 1],
            output_first: 0,
        }
    }
}

/// Errors returned by audio, MIDI, and standalone settings boundaries.
#[derive(Debug, Error)]
pub enum AudioError {
    /// A request violates channel, rate, buffer, or stream-state constraints.
    #[error("invalid audio configuration: {0}")]
    InvalidConfig(String),
    /// A callback buffer is malformed. This error never allocates on the audio thread.
    #[error("invalid audio callback: {0}")]
    InvalidCallback(&'static str),
    /// The processing chain rejected a callback. This error never allocates on the audio thread.
    #[error("audio chain failed: {0}")]
    Chain(#[source] ChainError),
    /// The selected device or backend is not connected.
    #[error("audio device is disconnected")]
    Disconnected,
    /// A device with this label is not connected any more. A label holds no serial number.
    #[error("{0} is not connected. Plug it back in.")]
    DeviceGone(String),
    /// Another program holds the device with this label.
    #[error("{0} is in use by another program. Close that program, or use the System driver.")]
    DeviceBusy(String),
    /// Callback-owned state was accessed from a different thread.
    #[error("audio worker used from a different callback thread")]
    WrongThread,
    /// The operating-system audio or MIDI backend rejected an operation.
    #[error("audio backend failed: {0}")]
    Backend(String),
    /// Persistent settings failed.
    #[error(transparent)]
    Properties(#[from] PropertiesError),
}

/// Chooses the lowest rate at or above 44.1 kHz, otherwise the rate nearest 48 kHz.
#[must_use]
pub fn choose_sample_rate(rates: &[u32], requested: Option<u32>) -> Option<u32> {
    if let Some(requested) = requested {
        return rates.contains(&requested).then_some(requested);
    }
    if rates.contains(&44_100) {
        return Some(44_100);
    }
    let minimum = rates.iter().copied().min()?;
    let maximum = rates.iter().copied().max()?;
    if minimum < 48_000 && maximum > 48_000 {
        return Some(48_000);
    }
    rates
        .iter()
        .copied()
        .filter(|rate| *rate >= 44_100)
        .min()
        .or_else(|| {
            rates.iter().copied().min_by_key(|rate| {
                let distance = rate.abs_diff(48_000);
                (distance, *rate > 48_000, *rate)
            })
        })
}

/// Computes bounded stream-close grace: 400 ms plus at most one I/O period, capped at 2 s.
#[must_use]
pub fn close_timeout(sample_rate: u32, buffer_frames: u32) -> Duration {
    let period_ms = if sample_rate == 0 {
        0
    } else {
        (u64::from(buffer_frames) * 1_000 / u64::from(sample_rate)).min(2_000)
    };
    let additional_ms = if period_ms < 4 { 0 } else { period_ms };
    Duration::from_millis(400 + additional_ms)
}

#[cfg(test)]
mod tests {
    use super::{
        AudioBackendKind::{self, Alsa, Dummy, Jack, System},
        AudioError, LINUX, available_on, from_flag_on, from_name_on, real_drivers_on,
    };

    #[test]
    fn linux_offers_each_driver_and_another_system_offers_system_and_dummy() {
        assert_eq!(available_on(true), [System, Alsa, Jack, Dummy]);
        assert_eq!(real_drivers_on(true), [System, Alsa, Jack]);
        assert_eq!(available_on(false), [System, Dummy]);
        assert_eq!(real_drivers_on(false), [System]);
        assert_eq!(AudioBackendKind::available(), available_on(LINUX));
        for kind in AudioBackendKind::available() {
            assert!(kind.is_available());
        }
    }

    #[test]
    fn a_saved_linux_driver_falls_back_to_system_on_another_system() {
        assert_eq!(from_name_on("ALSA", true), Alsa);
        assert_eq!(from_name_on("JACK", true), Jack);
        assert_eq!(from_name_on("ALSA", false), System);
        assert_eq!(from_name_on("JACK", false), System);
        assert_eq!(from_name_on("Dummy", false), Dummy);
        assert_eq!(from_name_on("CoreAudio", false), System);
    }

    #[test]
    fn the_backend_flag_refuses_a_driver_that_the_system_does_not_have() {
        assert_eq!(from_flag_on("alsa", true).ok(), Some(Alsa));
        assert_eq!(from_flag_on("system", false).ok(), Some(System));
        assert_eq!(from_flag_on("dummy", false).ok(), Some(Dummy));
        for driver in ["alsa", "jack"] {
            let Err(AudioError::InvalidConfig(text)) = from_flag_on(driver, false) else {
                panic!("{driver} must be refused");
            };
            assert!(text.contains("only on Linux"), "{text}");
            assert!(text.contains("system or dummy"), "{text}");
        }
        let Err(AudioError::InvalidConfig(text)) = from_flag_on("asio", true) else {
            panic!("an unknown driver must be refused");
        };
        assert!(text.contains("unknown backend asio"), "{text}");
    }
}
