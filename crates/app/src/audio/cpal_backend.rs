use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicUsize, Ordering},
};

use cpal::{
    BufferSize, Device, ErrorKind, FromSample, Host, I24, Sample, SampleFormat, SizedSample,
    Stream, StreamConfig, SupportedBufferSize, SupportedStreamConfigRange, U24,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use super::device_list::{alsa_device_is_virtual, common_rates, sane_channel_count};
use super::devices::{
    AudioBackend, AudioStream, DeviceCapabilities, DeviceDescriptor, NegotiatedStream, Route,
    fit_layout,
};
use super::{
    AudioBackendKind, AudioError, AudioEvent, AudioTelemetry, AudioWorker, DEFAULT_BUFFER_FRAMES,
    DeviceConfig, RawDevice, SYSTEM_DEFAULT_DEVICE, clean_alsa_devices, clean_server_devices,
};
use ronie_dsp::rt_chain::ChainCommandSender;

/// Prepared callback size when the host chooses its own buffer and reports no range.
const HOST_CHOSEN_CALLBACK_FRAMES: u32 = 1024;

/// The length of the login cookie of a PulseAudio server.
#[cfg(target_os = "linux")]
const PULSE_COOKIE_BYTES: usize = 256;

/// The queue between the capture and the playback can shrink this many times in each second. A
/// shrink changes the delay, so that it stays rare.
const QUEUE_WINDOWS_PER_SECOND: usize = 1;

/// The frames of the crossfade from the old to the new read point of each shrink. A hard jump
/// clicks. This is 2.7 ms at 48 kHz, which is too short to hear as two notes.
const QUEUE_CROSSFADE_FRAMES: usize = 128;

/// With OPTIMIZE off, the queue keeps this many callbacks at its lowest point.
const QUEUE_FIXED_CALLBACKS: usize = 2;

/// With OPTIMIZE on, the queue starts at this many callbacks, and keeps at least this many.
const QUEUE_MIN_CALLBACKS: usize = 1;

/// With OPTIMIZE on, each underrun adds one callback to the queue, up to this many.
const QUEUE_MAX_CALLBACKS: usize = 3;

/// With OPTIMIZE on, the queue keeps one callback less after this many seconds with no underrun.
const QUEUE_CLEAN_SECONDS: usize = 30;

/// The longest clean period, 8 minutes. An underrun soon after a step down doubles the clean
/// period of the next step down, up to this many seconds, so that a system that needs the larger
/// queue does not get a dropout each 30 s.
const QUEUE_MAX_CLEAN_SECONDS: usize = 16 * QUEUE_CLEAN_SECONDS;

/// An underrun in the first 1/2 second of a stream does not count, because the capture and the
/// playback start at different times.
const QUEUE_WARMUP_PARTS_OF_A_SECOND: usize = 2;

/// The channels to open when a device gives no usable count.
const FALLBACK_CHANNELS: usize = 2;

/// CPAL adapter for the system sound server and for real ALSA and JACK devices.
pub struct CpalBackend {
    kind: AudioBackendKind,
    /// The system route has no answer from its server, so that this route runs on ALSA.
    on_alsa: bool,
    /// The host of all work of this backend. A host of a sound server holds a connection with a
    /// thread, and the client library does not end these, so that this backend makes only one.
    host: Option<Host>,
}

/// The kept host, or a new host for a call that came before [`AudioBackend::connect`].
enum HostHandle<'a> {
    Kept(&'a Host),
    New(Host),
}

impl std::ops::Deref for HostHandle<'_> {
    type Target = Host;

    fn deref(&self) -> &Host {
        match self {
            Self::Kept(host) => host,
            Self::New(host) => host,
        }
    }
}

impl CpalBackend {
    /// Selects a real host backend.
    #[must_use]
    pub const fn new(kind: AudioBackendKind) -> Self {
        Self {
            kind,
            on_alsa: false,
            host: None,
        }
    }

    /// Opens a real stream through the common backend contract.
    ///
    /// # Errors
    /// Returns invalid-config, disconnected, or host-backend errors.
    pub fn open(
        &mut self,
        config: &DeviceConfig,
        worker: AudioWorker,
    ) -> Result<AudioStream, AudioError> {
        <Self as AudioBackend>::open(self, config, worker)
    }

    /// The host that really runs.
    #[cfg(target_os = "linux")]
    fn driver(&self) -> Result<cpal::HostId, AudioError> {
        match self.kind {
            AudioBackendKind::System if self.on_alsa => Ok(cpal::HostId::Alsa),
            AudioBackendKind::System => Ok(cpal::HostId::PulseAudio),
            AudioBackendKind::Alsa => Ok(cpal::HostId::Alsa),
            AudioBackendKind::Jack => Ok(cpal::HostId::Jack),
            AudioBackendKind::Dummy => Err(dummy_error()),
        }
    }

    /// Whether the host that really runs is ALSA, which shows each layout of a device.
    #[cfg(target_os = "linux")]
    fn on_alsa_host(&self) -> bool {
        self.driver().ok() == Some(cpal::HostId::Alsa)
    }

    /// Only Linux has ALSA.
    #[cfg(not(target_os = "linux"))]
    const fn on_alsa_host(&self) -> bool {
        false
    }

    /// A new host for the driver.
    #[cfg(target_os = "linux")]
    fn new_host(&self) -> Result<Host, AudioError> {
        cpal::host_from_id(self.driver()?).map_err(|error| AudioError::Backend(error.to_string()))
    }

    /// A new host for the driver. The system driver is the default host of CPAL: CoreAudio on
    /// macOS, and WASAPI on Windows.
    #[cfg(not(target_os = "linux"))]
    fn new_host(&self) -> Result<Host, AudioError> {
        match self.kind {
            AudioBackendKind::System => Ok(cpal::default_host()),
            AudioBackendKind::Alsa | AudioBackendKind::Jack => Err(AudioError::InvalidConfig(
                format!("the {} driver is only on Linux", self.kind.name()),
            )),
            AudioBackendKind::Dummy => Err(dummy_error()),
        }
    }

    fn host(&self) -> Result<HostHandle<'_>, AudioError> {
        if let Some(host) = &self.host {
            return Ok(HostHandle::Kept(host));
        }
        self.new_host().map(HostHandle::New)
    }

    /// The channels to open: the real count of the device.
    fn device_channels(
        &self,
        host: &Host,
        device: &Device,
        input: bool,
        ranges: &[SupportedStreamConfigRange],
    ) -> usize {
        let largest = |ranges: &[SupportedStreamConfigRange]| {
            ranges
                .iter()
                .map(|range| usize::from(range.channels()))
                .max()
        };
        if !self.on_alsa_host() {
            // A server takes any layout, so that the ranges say nothing about the device.
            let reported = if input {
                device.default_input_config()
            } else {
                device.default_output_config()
            }
            .ok()
            .map(|config| usize::from(config.channels()));
            return sane_channel_count(
                reported.or(largest(ranges)).unwrap_or(FALLBACK_CHANNELS),
                false,
            );
        }
        let Ok(id) = device.id() else {
            return FALLBACK_CHANNELS;
        };
        let id = id.id().to_owned();
        if alsa_device_is_virtual(&id) {
            return sane_channel_count(largest(ranges).unwrap_or(FALLBACK_CHANNELS), true);
        }
        // A conversion device reports what it could convert. The direct device tells the truth,
        // and it can be busy, so that the common layout is the answer in that case.
        let direct = id.strip_prefix("plug").and_then(|direct| {
            let device = find_device(host, direct, input).ok().flatten()?;
            let ranges = supported_ranges(&device, input).ok()?;
            largest(&ranges)
        });
        if id.starts_with("plug") {
            return sane_channel_count(direct.unwrap_or(FALLBACK_CHANNELS), false);
        }
        sane_channel_count(largest(ranges).unwrap_or(FALLBACK_CHANNELS), false)
    }

    /// One complete choice for one direction.
    fn choose_stream(
        &self,
        host: &Host,
        device: &Device,
        input: bool,
        rate: Option<u32>,
        buffer: Option<u32>,
    ) -> Result<(StreamConfig, SampleFormat), AudioError> {
        let label = device_label(device);
        let ranges = supported_ranges(device, input).map_err(|error| host_error(&error, &label))?;
        let channels = self.device_channels(host, device, input, &ranges);
        // ALSA already has 48 kHz in most ranges, and only a server has a rate of its own.
        let usual = (!self.on_alsa_host())
            .then(|| {
                if input {
                    device.default_input_config()
                } else {
                    device.default_output_config()
                }
                .ok()
                .map(|config| config.sample_rate())
            })
            .flatten();
        choose_config(&ranges, channels, rate, usual, buffer).map_err(|error| match error {
            ConfigError::NoChannels => {
                AudioError::InvalidConfig(format!("{label} has no layout with {channels} channels"))
            }
            ConfigError::NoRate(rate) => {
                AudioError::InvalidConfig(format!("{label} does not accept {rate} Hz"))
            }
        })
    }

    /// Selects both devices and both layouts for one request. The output decides the rate.
    fn plan(&self, requested: &DeviceConfig) -> Result<StreamPlan, AudioError> {
        if requested.backend != self.kind || requested.buffer_frames == Some(0) {
            return Err(AudioError::InvalidConfig(
                "backend mismatch or zero buffer size".to_owned(),
            ));
        }
        let handle = self.host()?;
        let host: &Host = &handle;
        let output_device = select_device(host, requested.output_device.as_deref(), false)?
            .ok_or_else(|| gone(requested.output_label.as_deref()))?;
        let (output_config, output_format) = self.choose_stream(
            host,
            &output_device,
            false,
            requested.sample_rate,
            requested.buffer_frames,
        )?;
        let input = match select_device(host, requested.input_device.as_deref(), true)? {
            Some(device) => {
                let (config, format) = self.choose_stream(
                    host,
                    &device,
                    true,
                    Some(output_config.sample_rate),
                    requested.buffer_frames,
                )?;
                Some((device, config, format))
            }
            None if requested
                .input_device
                .as_deref()
                .is_none_or(|id| id == SYSTEM_DEFAULT_DEVICE) =>
            {
                None
            }
            None => return Err(gone(requested.input_label.as_deref())),
        };
        let mut config = requested.clone();
        let capture = input
            .as_ref()
            .map_or(config.input_channels, |(_, stream, _)| {
                usize::from(stream.channels)
            });
        fit_layout(&mut config, capture, usize::from(output_config.channels));
        Ok(StreamPlan {
            config,
            input,
            output: (output_device, output_config, output_format),
        })
    }
}

/// The devices and the layouts that one request means.
struct StreamPlan {
    config: DeviceConfig,
    input: Option<(Device, StreamConfig, SampleFormat)>,
    output: (Device, StreamConfig, SampleFormat),
}

/// The error for a dummy kind on a CPAL backend.
fn dummy_error() -> AudioError {
    AudioError::InvalidConfig("CPAL cannot open the dummy backend".to_owned())
}

/// The error for a device that is gone. A log can show this, so that it never holds an identity.
fn gone(label: Option<&str>) -> AudioError {
    label.map_or(AudioError::Disconnected, |label| {
        AudioError::DeviceGone(label.to_owned())
    })
}

/// The text of one host device for a person. A label holds no node name.
fn device_label(device: &Device) -> String {
    device
        .description()
        .map_or_else(|_| device.to_string(), |text| text.name().to_owned())
}

/// Maps a host error to the error that the user can act on.
fn host_error(error: &cpal::Error, label: &str) -> AudioError {
    match error.kind() {
        ErrorKind::DeviceBusy => AudioError::DeviceBusy(label.to_owned()),
        ErrorKind::DeviceNotAvailable | ErrorKind::HostUnavailable => {
            AudioError::DeviceGone(label.to_owned())
        }
        _ => AudioError::Backend(error.to_string()),
    }
}

fn supported_ranges(
    device: &Device,
    input: bool,
) -> Result<Vec<SupportedStreamConfigRange>, cpal::Error> {
    Ok(if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    })
}

fn find_device(host: &Host, id: &str, input: bool) -> Result<Option<Device>, AudioError> {
    let devices = host
        .devices()
        .map_err(|error| AudioError::Backend(error.to_string()))?;
    for device in devices {
        let same = device.id().is_ok_and(|device_id| device_id.id() == id);
        let can = if input {
            device.supports_input()
        } else {
            device.supports_output()
        };
        if same && can {
            return Ok(Some(device));
        }
    }
    Ok(None)
}

fn select_device(
    host: &Host,
    requested: Option<&str>,
    input: bool,
) -> Result<Option<Device>, AudioError> {
    match requested.filter(|id| *id != SYSTEM_DEFAULT_DEVICE) {
        Some(id) => find_device(host, id, input),
        None => Ok(if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        }),
    }
}

impl AudioBackend for CpalBackend {
    fn connect(&mut self) -> Route {
        // A server can restart, and a kept host then has a dead connection.
        if self
            .host
            .as_ref()
            .is_some_and(|host| host.devices().is_err())
        {
            self.host = None;
        }
        if self.host.is_some() {
            return system_route(self.on_alsa);
        }
        #[cfg(target_os = "linux")]
        if self.kind == AudioBackendKind::System {
            if let Some(home) = std::env::var_os("HOME")
                && std::env::var_os("PULSE_COOKIE").is_none()
            {
                let _ = ensure_pulse_cookie(std::path::Path::new(&home));
            }
            // The host waits for the handshake of the server, within its own limit of two seconds.
            match cpal::host_from_id(cpal::HostId::PulseAudio) {
                Ok(host) => {
                    self.on_alsa = false;
                    self.host = Some(host);
                    return Route::default();
                }
                Err(_) => self.on_alsa = true,
            }
        }
        self.host = self.new_host().ok();
        system_route(self.on_alsa)
    }

    fn negotiate(&self, requested: &DeviceConfig) -> Result<NegotiatedStream, AudioError> {
        let plan = self.plan(requested)?;
        let (output_device, output_config, _) = &plan.output;
        let (buffer_frames, max_callback_frames) = match output_config.buffer_size {
            BufferSize::Fixed(frames) => (Some(frames), frames),
            _ => (None, HOST_CHOSEN_CALLBACK_FRAMES),
        };
        let capture_channels = plan
            .input
            .as_ref()
            .map_or(plan.config.input_channels, |(_, config, _)| {
                usize::from(config.channels)
            });
        let mut config = plan.config.clone();
        config.sample_rate = Some(output_config.sample_rate);
        config.buffer_frames = buffer_frames;
        Ok(NegotiatedStream {
            sample_rate: output_config.sample_rate,
            max_callback_frames,
            input_device: plan.input.as_ref().map(|(device, ..)| device_label(device)),
            output_device: Some(device_label(output_device)),
            capture_channels,
            playback_channels: usize::from(output_config.channels),
            config,
        })
    }

    fn enumerate(&self) -> Result<Vec<DeviceDescriptor>, AudioError> {
        let host = self.host()?;
        let default_id = |device: Option<Device>| {
            device.and_then(|device| device.id().ok().map(|id| id.id().to_owned()))
        };
        let default_input = default_id(host.default_input_device());
        let default_output = default_id(host.default_output_device());
        let devices = host
            .devices()
            .map_err(|error| AudioError::Backend(error.to_string()))?;
        let raw: Vec<RawDevice> = devices
            .filter_map(|device| {
                let id = device.id().ok()?.id().to_owned();
                let description = device.description().ok()?;
                Some(RawDevice {
                    default_input: default_input.as_deref() == Some(id.as_str()),
                    default_output: default_output.as_deref() == Some(id.as_str()),
                    input: device.supports_input(),
                    output: device.supports_output(),
                    name: description.name().to_owned(),
                    detail: description.extended().map(str::to_owned).collect(),
                    id,
                })
            })
            .collect();
        Ok(if self.on_alsa_host() {
            clean_alsa_devices(raw)
        } else {
            clean_server_devices(raw)
        })
    }

    fn capabilities(
        &self,
        device: Option<&str>,
        input: bool,
    ) -> Result<DeviceCapabilities, AudioError> {
        let handle = self.host()?;
        let host: &Host = &handle;
        let device = select_device(host, device, input)?.ok_or(AudioError::Disconnected)?;
        let label = device_label(&device);
        let ranges =
            supported_ranges(&device, input).map_err(|error| host_error(&error, &label))?;
        let channels = self.device_channels(host, &device, input, &ranges);
        let layout = usable_ranges(&ranges, channels);
        let device_rate = if input {
            device.default_input_config()
        } else {
            device.default_output_config()
        }
        .ok()
        .map(|config| config.sample_rate());
        Ok(DeviceCapabilities {
            sample_rates: common_rates(|rate| layout.iter().any(|range| range.contains_rate(rate))),
            device_rate,
            channels,
            buffer_frames: layout.iter().find_map(|range| match range.buffer_size() {
                SupportedBufferSize::Range { min, max } => Some(*min..=*max),
                SupportedBufferSize::Unknown => None,
            }),
        })
    }

    fn open(
        &mut self,
        config: &DeviceConfig,
        worker: AudioWorker,
    ) -> Result<AudioStream, AudioError> {
        let plan = self.plan(config)?;
        let stream = CpalStream::build(plan.config, worker, plan.input, plan.output)?;
        Ok(AudioStream::Cpal(Box::new(stream)))
    }
}

/// Makes the login cookie of the sound server when none exists, as each PulseAudio client does.
///
/// PipeWire accepts any cookie but refuses a client with none, and a desktop with no PulseAudio
/// client has none. With no cookie, the system route would run on ALSA for no good reason.
#[cfg(target_os = "linux")]
fn ensure_pulse_cookie(home: &std::path::Path) -> std::io::Result<()> {
    use std::{io::Read as _, io::Write as _, os::unix::fs::OpenOptionsExt as _};
    let cookie = home.join(".config/pulse/cookie");
    if cookie.exists() || home.join(".pulse-cookie").exists() {
        return Ok(());
    }
    let mut bytes = [0_u8; PULSE_COOKIE_BYTES];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    if let Some(directory) = cookie.parent() {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(cookie)?
        .write_all(&bytes)
}

/// What a system route says to the user about its server.
fn system_route(on_alsa: bool) -> Route {
    if !on_alsa {
        return Route::default();
    }
    Route {
        active: Some(AudioBackendKind::Alsa),
        notice: Some("The system sound server does not answer, so Ronnie uses ALSA.".to_owned()),
    }
}

/// Open CPAL input/output pair. The DSP worker lives only in the output callback.
pub struct CpalStream {
    config: DeviceConfig,
    input: std::sync::Mutex<Option<Stream>>,
    output: std::sync::Mutex<Option<Stream>>,
    /// The label of each stream, for a message when it cannot start.
    labels: (String, String),
    pub(super) commands: ChainCommandSender,
    pub(super) telemetry: Arc<AudioTelemetry>,
}

impl CpalStream {
    fn build(
        config: DeviceConfig,
        worker: AudioWorker,
        input: Option<(Device, StreamConfig, SampleFormat)>,
        (output_device, output_config, output_format): (Device, StreamConfig, SampleFormat),
    ) -> Result<Self, AudioError> {
        let telemetry = worker.telemetry();
        let max_frames = worker.max_frames();
        let input_channels = input
            .as_ref()
            .map_or(config.input_channels, |(_, stream_config, _)| {
                usize::from(stream_config.channels)
            });
        let frames = (output_config.sample_rate as usize)
            .clamp(1, 192_000)
            .checked_div(10)
            .unwrap_or(4_800)
            .max(max_frames);
        let ring = Arc::new(InputRing::new(frames * input_channels * 2 + 1));
        // The reader plays one callback of silence first, in place of a prefill of the ring, so
        // that a host that chose its own buffer gets one real callback and not 1024 frames.
        let host_chosen = matches!(output_config.buffer_size, BufferSize::Default);
        let trim = QueueTrim::new(max_frames, output_config.sample_rate as usize, host_chosen);
        let reader = QueueReader::new(trim, input_channels, input.is_some());
        let input_label = input
            .as_ref()
            .map(|(device, ..)| device_label(device))
            .unwrap_or_default();
        let output_label = device_label(&output_device);
        let input_stream = match input {
            Some((device, stream_config, format)) => Some(
                build_input_stream(
                    &device,
                    &stream_config,
                    format,
                    Arc::clone(&ring),
                    Arc::clone(&telemetry),
                )
                .map_err(|error| host_error(&error, &input_label))?,
            ),
            None => None,
        };
        let commands = worker.command_sender();
        let output = build_output_stream(
            &output_device,
            &output_config,
            output_format,
            worker,
            ring,
            reader,
            Arc::clone(&telemetry),
        )
        .map_err(|error| host_error(&error, &output_label))?;
        Ok(Self {
            config,
            input: std::sync::Mutex::new(input_stream),
            output: std::sync::Mutex::new(Some(output)),
            labels: (input_label, output_label),
            commands,
            telemetry,
        })
    }

    /// Starts playback, then capture.
    ///
    /// # Errors
    /// Returns the host stream error and releases playback if capture cannot start.
    pub fn start(&mut self) -> Result<(), AudioError> {
        let output_guard = self
            .output
            .lock()
            .map_err(|error| AudioError::Backend(error.to_string()))?;
        let Some(output) = output_guard.as_ref() else {
            return Err(AudioError::InvalidConfig("stream is closed".to_owned()));
        };
        output
            .play()
            .map_err(|error| host_error(&error, &self.labels.1))?;
        drop(output_guard);
        let input_guard = self
            .input
            .lock()
            .map_err(|error| AudioError::Backend(error.to_string()))?;
        if let Some(input) = input_guard.as_ref()
            && let Err(error) = input.play()
        {
            drop(input_guard);
            self.output
                .lock()
                .map_err(|error| AudioError::Backend(error.to_string()))?
                .take();
            return Err(host_error(&error, &self.labels.0));
        }
        Ok(())
    }

    /// Closes both streams immediately instead of relying on optional PCM pause support.
    ///
    /// # Errors
    /// Returns a lock error; host callback failures are reported through [`Self::failed`].
    pub fn close(&self) -> Result<(), AudioError> {
        self.input
            .lock()
            .map_err(|error| AudioError::Backend(error.to_string()))?
            .take();
        self.output
            .lock()
            .map_err(|error| AudioError::Backend(error.to_string()))?
            .take();
        Ok(())
    }

    /// Requested stream configuration.
    #[must_use]
    pub const fn config(&self) -> &DeviceConfig {
        &self.config
    }

    /// Whether the host or the callback reported a failure that ends this stream's audio.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.telemetry.stream_failed()
    }
}

/// Why no layout fits a request.
#[derive(Debug, PartialEq, Eq)]
enum ConfigError {
    /// The device reports no range at all.
    NoChannels,
    /// No range of the device holds the requested rate.
    NoRate(u32),
}

/// The ranges with the wanted channel count. A device with no such range gives its largest layout
/// that fits, and then its smallest layout, so that a stream can still open.
fn usable_ranges(
    ranges: &[SupportedStreamConfigRange],
    channels: usize,
) -> Vec<SupportedStreamConfigRange> {
    let with = |count: usize| {
        ranges
            .iter()
            .filter(|range| usize::from(range.channels()) == count)
            .copied()
            .collect::<Vec<_>>()
    };
    let exact = with(channels);
    if !exact.is_empty() {
        return exact;
    }
    let below = ranges
        .iter()
        .map(|range| usize::from(range.channels()))
        .filter(|count| *count <= channels)
        .max();
    let above = ranges
        .iter()
        .map(|range| usize::from(range.channels()))
        .min();
    below.or(above).map(with).unwrap_or_default()
}

/// Chooses a layout from all ranges of a device. ALSA gives one range for each rate, so that the
/// range for a rate can be any of them.
fn choose_config(
    ranges: &[SupportedStreamConfigRange],
    channels: usize,
    requested_rate: Option<u32>,
    usual_rate: Option<u32>,
    requested_frames: Option<u32>,
) -> Result<(StreamConfig, SampleFormat), ConfigError> {
    let mut layout = usable_ranges(ranges, channels);
    if layout.is_empty() {
        return Err(ConfigError::NoChannels);
    }
    layout.sort_by_key(|range| sample_format_rank(range.sample_format()));
    let holds = |rate: u32| layout.iter().any(|range| range.contains_rate(rate));
    let rate = match requested_rate {
        Some(rate) if holds(rate) => rate,
        Some(rate) => return Err(ConfigError::NoRate(rate)),
        None => [usual_rate, Some(48_000), Some(44_100)]
            .into_iter()
            .flatten()
            .find(|rate| holds(*rate))
            .unwrap_or_else(|| nearest_rate(&layout)),
    };
    let Some(range) = layout.iter().find(|range| range.contains_rate(rate)) else {
        return Err(ConfigError::NoRate(rate));
    };
    let frames = requested_frames.unwrap_or(DEFAULT_BUFFER_FRAMES);
    let buffer_size = match range.buffer_size() {
        SupportedBufferSize::Range { min, max } => BufferSize::Fixed(frames.clamp(*min, *max)),
        SupportedBufferSize::Unknown if requested_frames.is_some() => BufferSize::Fixed(frames),
        SupportedBufferSize::Unknown => BufferSize::Default,
    };
    Ok((
        StreamConfig {
            channels: range.channels(),
            sample_rate: rate,
            buffer_size,
        },
        range.sample_format(),
    ))
}

/// The rate of a range nearest to 48 kHz, for a device that has neither common rate.
fn nearest_rate(ranges: &[SupportedStreamConfigRange]) -> u32 {
    ranges
        .iter()
        .flat_map(|range| [range.min_sample_rate(), range.max_sample_rate()])
        .min_by_key(|rate| rate.abs_diff(48_000))
        .unwrap_or(48_000)
}

fn sample_format_rank(format: SampleFormat) -> usize {
    match format {
        SampleFormat::F32 => 0,
        SampleFormat::I32 => 1,
        SampleFormat::I24 => 2,
        SampleFormat::I16 => 3,
        SampleFormat::F64 => 4,
        _ => 5,
    }
}

/// Classifies one host error. Only a condition that ends the stream's audio is fatal.
const fn host_error_event(kind: ErrorKind) -> Option<AudioEvent> {
    match kind {
        ErrorKind::Xrun => Some(AudioEvent::OutputUnderrun),
        ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => None,
        ErrorKind::DeviceNotAvailable
        | ErrorKind::HostUnavailable
        | ErrorKind::StreamInvalidated => Some(AudioEvent::DeviceDisconnected),
        _ => Some(AudioEvent::StreamFailed),
    }
}

fn report_host_error(telemetry: &AudioTelemetry, kind: ErrorKind) {
    if let Some(event) = host_error_event(kind) {
        telemetry.publish_event(event);
    }
}

fn build_input_stream(
    device: &Device,
    config: &StreamConfig,
    format: SampleFormat,
    ring: Arc<InputRing>,
    telemetry: Arc<AudioTelemetry>,
) -> Result<Stream, cpal::Error> {
    match format {
        SampleFormat::I8 => build_input::<i8>(device, config, ring, telemetry),
        SampleFormat::I16 => build_input::<i16>(device, config, ring, telemetry),
        SampleFormat::I24 => build_input::<I24>(device, config, ring, telemetry),
        SampleFormat::I32 => build_input::<i32>(device, config, ring, telemetry),
        SampleFormat::I64 => build_input::<i64>(device, config, ring, telemetry),
        SampleFormat::U8 => build_input::<u8>(device, config, ring, telemetry),
        SampleFormat::U16 => build_input::<u16>(device, config, ring, telemetry),
        SampleFormat::U24 => build_input::<U24>(device, config, ring, telemetry),
        SampleFormat::U32 => build_input::<u32>(device, config, ring, telemetry),
        SampleFormat::U64 => build_input::<u64>(device, config, ring, telemetry),
        SampleFormat::F32 => build_input::<f32>(device, config, ring, telemetry),
        SampleFormat::F64 => build_input::<f64>(device, config, ring, telemetry),
        other => Err(cpal::Error::with_message(
            ErrorKind::UnsupportedConfig,
            format!("unsupported input sample format {other}"),
        )),
    }
}

fn build_input<T>(
    device: &Device,
    config: &StreamConfig,
    ring: Arc<InputRing>,
    telemetry: Arc<AudioTelemetry>,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    if channels == 0 {
        return Err(cpal::Error::with_message(
            ErrorKind::InvalidInput,
            "the input stream needs channels",
        ));
    }
    let error_telemetry = Arc::clone(&telemetry);
    device.build_input_stream::<T, _, _>(
        *config,
        move |samples, _| {
            for frame in samples.chunks_exact(channels) {
                let frame = frame.iter().map(|sample| f32::from_sample(*sample));
                if !ring.push_frame(frame) {
                    telemetry.publish_event(AudioEvent::InputOverrun);
                    break;
                }
            }
        },
        move |error| {
            error_telemetry.note_capture_error();
            report_host_error(&error_telemetry, error.kind());
        },
        None,
    )
}

fn build_output_stream(
    device: &Device,
    config: &StreamConfig,
    format: SampleFormat,
    worker: AudioWorker,
    ring: Arc<InputRing>,
    reader: QueueReader,
    telemetry: Arc<AudioTelemetry>,
) -> Result<Stream, cpal::Error> {
    match format {
        SampleFormat::I8 => build_output::<i8>(device, config, worker, ring, reader, telemetry),
        SampleFormat::I16 => build_output::<i16>(device, config, worker, ring, reader, telemetry),
        SampleFormat::I24 => build_output::<I24>(device, config, worker, ring, reader, telemetry),
        SampleFormat::I32 => build_output::<i32>(device, config, worker, ring, reader, telemetry),
        SampleFormat::I64 => build_output::<i64>(device, config, worker, ring, reader, telemetry),
        SampleFormat::U8 => build_output::<u8>(device, config, worker, ring, reader, telemetry),
        SampleFormat::U16 => build_output::<u16>(device, config, worker, ring, reader, telemetry),
        SampleFormat::U24 => build_output::<U24>(device, config, worker, ring, reader, telemetry),
        SampleFormat::U32 => build_output::<u32>(device, config, worker, ring, reader, telemetry),
        SampleFormat::U64 => build_output::<u64>(device, config, worker, ring, reader, telemetry),
        SampleFormat::F32 => build_output::<f32>(device, config, worker, ring, reader, telemetry),
        SampleFormat::F64 => build_output::<f64>(device, config, worker, ring, reader, telemetry),
        other => Err(cpal::Error::with_message(
            ErrorKind::UnsupportedConfig,
            format!("unsupported output sample format {other}"),
        )),
    }
}

fn build_output<T>(
    device: &Device,
    config: &StreamConfig,
    mut worker: AudioWorker,
    ring: Arc<InputRing>,
    mut reader: QueueReader,
    telemetry: Arc<AudioTelemetry>,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let input_channels = reader.channels;
    let max_frames = reader.trim.prepared;
    let output_channels = usize::from(config.channels);
    if input_channels == 0 || output_channels == 0 || max_frames == 0 {
        return Err(cpal::Error::with_message(
            ErrorKind::InvalidInput,
            "the output stream needs channels and frames",
        ));
    }
    let part_samples = max_frames * output_channels;
    let mut captured = vec![0.0_f32; max_frames * input_channels];
    let mut rendered = vec![0.0_f32; part_samples];
    let error_telemetry = Arc::clone(&telemetry);
    device.build_output_stream::<T, _, _>(
        *config,
        move |samples, _| {
            // One atomic load. A change of OPTIMIZE reaches the next callback with no lock.
            let optimize = telemetry.optimize();
            // A host can deliver more than it promised. Prepared-size parts keep audio
            // running in that case, and the part length bounds every slice below.
            for part in samples.chunks_mut(part_samples) {
                let frames = part.len() / output_channels;
                let input_len = frames * input_channels;
                if reader.read(&ring, &mut captured[..input_len], optimize) {
                    telemetry.note_input_underrun();
                }
                telemetry.publish_queue(reader.trim.level, reader.trim.unit());
                let output_len = frames * output_channels;
                if worker
                    .process_interleaved(
                        &captured[..input_len],
                        input_channels,
                        &mut rendered[..output_len],
                        output_channels,
                    )
                    .is_err()
                {
                    rendered[..output_len].fill(0.0);
                    telemetry.publish_event(AudioEvent::StreamFailed);
                }
                for (destination, source) in part.iter_mut().zip(&rendered[..output_len]) {
                    *destination = T::from_sample(*source);
                }
                for destination in part.iter_mut().skip(output_len) {
                    *destination = T::from_sample(0.0);
                }
            }
        },
        move |error| report_host_error(&error_telemetry, error.kind()),
        None,
    )
}

/// Keeps the queue between the capture and the playback short.
///
/// Two streams start at different times and can drift, and each frame that stays in the queue is
/// delay that a guitar player hears. A short peak is normal, so that only frames that stayed for
/// the whole window go.
///
/// With OPTIMIZE off, the queue keeps [`QUEUE_FIXED_CALLBACKS`]. With OPTIMIZE on, it starts at
/// [`QUEUE_MIN_CALLBACKS`], adds one callback after each underrun, and gives one back after
/// [`QUEUE_CLEAN_SECONDS`] with no underrun. An underrun less than one clean period after a step
/// down doubles the clean period, up to [`QUEUE_MAX_CLEAN_SECONDS`]. A change of OPTIMIZE sets it
/// back to [`QUEUE_CLEAN_SECONDS`]. The clock is the count of played frames, so that a test needs
/// no real time. No call allocates or locks.
struct QueueTrim {
    /// The callback size that the chain was prepared for.
    prepared: usize,
    /// Whether the host chose the callback size. The real callbacks then give the unit.
    host_chosen: bool,
    /// The largest callback that the host gave.
    callback: usize,
    /// The callbacks to keep at the lowest point of a window.
    keep_callbacks: usize,
    /// Whether OPTIMIZE was on in the latest callback.
    adaptive: bool,
    window: usize,
    seen: usize,
    lowest: usize,
    /// The frames that stayed in the queue for the latest whole window, after its shrink.
    level: usize,
    /// Frames played since the latest underrun or change of the queue.
    clean: usize,
    /// The clean frames that the next step down needs.
    clean_limit: usize,
    /// The first and the longest clean period in frames.
    clean_start: usize,
    clean_max: usize,
    /// Frames played since the latest step down, or `None` when an underrun or a change of
    /// OPTIMIZE came after it.
    since_step_down: Option<usize>,
    /// Frames played since the start, up to the end of the warm-up.
    played: usize,
    warmup: usize,
}

impl QueueTrim {
    fn new(max_frames: usize, sample_rate: usize, host_chosen: bool) -> Self {
        Self {
            prepared: max_frames,
            host_chosen,
            callback: 0,
            keep_callbacks: QUEUE_MIN_CALLBACKS,
            adaptive: true,
            window: (sample_rate / QUEUE_WINDOWS_PER_SECOND).max(max_frames),
            seen: 0,
            lowest: usize::MAX,
            level: 0,
            clean: 0,
            clean_limit: sample_rate.saturating_mul(QUEUE_CLEAN_SECONDS),
            clean_start: sample_rate.saturating_mul(QUEUE_CLEAN_SECONDS),
            clean_max: sample_rate.saturating_mul(QUEUE_MAX_CLEAN_SECONDS),
            since_step_down: None,
            played: 0,
            warmup: sample_rate / QUEUE_WARMUP_PARTS_OF_A_SECOND,
        }
    }

    /// The frames of one callback: the prepared size, or the real size when the host chose it.
    const fn unit(&self) -> usize {
        if self.host_chosen && self.callback > 0 {
            self.callback
        } else {
            self.prepared
        }
    }

    /// The frames to keep at the lowest point of a window.
    const fn keep(&self) -> usize {
        self.unit() * self.keep_callbacks
    }

    /// Records one callback, and returns the frames to drop at the end of a window.
    fn excess(&mut self, available: usize, frames: usize, optimize: bool) -> usize {
        if optimize != self.adaptive {
            self.adaptive = optimize;
            self.keep_callbacks = if optimize {
                QUEUE_MIN_CALLBACKS
            } else {
                QUEUE_FIXED_CALLBACKS
            };
            self.clean = 0;
            self.clean_limit = self.clean_start;
            self.since_step_down = None;
        }
        self.callback = self.callback.max(frames.min(self.prepared));
        self.played = self.played.saturating_add(frames).min(self.warmup + 1);
        if self.adaptive {
            self.clean = self.clean.saturating_add(frames);
            self.since_step_down = self
                .since_step_down
                .map(|since| since.saturating_add(frames));
            if self.clean >= self.clean_limit {
                self.clean = 0;
                if self.keep_callbacks > QUEUE_MIN_CALLBACKS {
                    self.keep_callbacks -= 1;
                    self.since_step_down = Some(0);
                }
            }
        }
        self.lowest = self.lowest.min(available);
        self.seen += frames;
        if self.seen < self.window {
            return 0;
        }
        let keep = self.keep();
        let excess = self.lowest.saturating_sub(keep);
        self.level = self.lowest.min(keep);
        self.seen = 0;
        self.lowest = usize::MAX;
        excess
    }

    /// Records a callback that found too few captured frames. Returns whether it counts: an
    /// underrun in the warm-up does not.
    fn underrun(&mut self) -> bool {
        if self.played <= self.warmup {
            return false;
        }
        if self.adaptive {
            // The step down was too far for this system, so the next one waits longer.
            if self
                .since_step_down
                .take()
                .is_some_and(|since| since < self.clean_limit)
            {
                self.clean_limit = self.clean_limit.saturating_mul(2).min(self.clean_max);
            }
            self.keep_callbacks = (self.keep_callbacks + 1).min(QUEUE_MAX_CALLBACKS);
            self.clean = 0;
        }
        true
    }
}

/// Reads the captured frames for one playback callback from the queue.
///
/// Each shrink of the queue is a crossfade from the frames at the old read point to the frames at
/// the new read point, so that a shrink does not click. The crossfade copies the old frames into
/// a buffer that this reader allocated before the stream started. No call allocates or locks.
struct QueueReader {
    trim: QueueTrim,
    /// The channels of one captured frame.
    channels: usize,
    /// Whether a capture stream feeds the queue. With none, an empty queue is no underrun.
    capture: bool,
    /// The frames at the old read point of the latest shrink.
    fade: Box<[f32]>,
    fade_len: usize,
    fade_at: usize,
    /// Silent frames that the reader still plays before its first read, or `None` before the
    /// first callback. This is the cushion that a prefill of the ring gave before.
    prime: Option<usize>,
}

impl QueueReader {
    fn new(trim: QueueTrim, channels: usize, capture: bool) -> Self {
        let channels = channels.max(1);
        Self {
            trim,
            channels,
            capture,
            fade: vec![0.0; QUEUE_CROSSFADE_FRAMES * channels].into_boxed_slice(),
            fade_len: 0,
            fade_at: 0,
            prime: None,
        }
    }

    /// Fills `captured` with complete frames. Returns whether the queue ran empty in an underrun
    /// that counts.
    fn read(&mut self, ring: &InputRing, captured: &mut [f32], optimize: bool) -> bool {
        let channels = self.channels;
        let frames = captured.len() / channels;
        let first = if self.trim.host_chosen {
            frames
        } else {
            self.trim.prepared
        };
        let mut prime = *self.prime.get_or_insert(first);
        let available = ring.available_frames(channels);
        let excess = self.trim.excess(available, frames, optimize);
        if excess > 0 {
            self.begin_fade(ring, available);
            ring.skip_frames(excess, channels);
        }
        let mut empty = false;
        for frame in captured.chunks_exact_mut(channels) {
            if prime > 0 {
                prime -= 1;
                frame.fill(0.0);
                continue;
            }
            empty |= !ring.pop_frame(frame);
            if self.fade_at < self.fade_len {
                let gain = (self.fade_at + 1) as f32 / (self.fade_len + 1) as f32;
                let old = &self.fade[self.fade_at * channels..(self.fade_at + 1) * channels];
                for (sample, old) in frame.iter_mut().zip(old) {
                    *sample = *old * (1.0 - gain) + *sample * gain;
                }
                self.fade_at += 1;
            }
        }
        self.prime = Some(prime);
        empty && self.capture && self.trim.underrun()
    }

    /// Copies the frames at the read point before a shrink moves it.
    fn begin_fade(&mut self, ring: &InputRing, available: usize) {
        let channels = self.channels;
        let mut len = 0;
        for slot in self
            .fade
            .chunks_exact_mut(channels)
            .take(QUEUE_CROSSFADE_FRAMES.min(available))
        {
            if !ring.peek_frame(len, slot) {
                break;
            }
            len += 1;
        }
        self.fade_len = len;
        self.fade_at = 0;
    }
}

/// Single-producer, single-consumer ring of interleaved frames.
///
/// Each index moves by one complete frame, so the reader never sees a part of a frame and the
/// channels of a stereo capture cannot change places. The methods do not allocate or lock.
struct InputRing {
    slots: Box<[AtomicU32]>,
    read: AtomicUsize,
    write: AtomicUsize,
}

impl InputRing {
    fn new(capacity: usize) -> Self {
        let mut slots = Vec::with_capacity(capacity.max(2));
        slots.resize_with(capacity.max(2), || AtomicU32::new(0));
        Self {
            slots: slots.into_boxed_slice(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
        }
    }

    /// Publishes one complete frame, or nothing when the ring cannot hold all of it.
    fn push_frame(&self, frame: impl ExactSizeIterator<Item = f32>) -> bool {
        let capacity = self.slots.len();
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        // One slot stays empty, so that equal indices mean an empty ring.
        let free = (read + capacity - write - 1) % capacity;
        let samples = frame.len();
        if samples > free {
            return false;
        }
        for (offset, sample) in frame.enumerate() {
            self.slots[(write + offset) % capacity].store(sample.to_bits(), Ordering::Relaxed);
        }
        self.write
            .store((write + samples) % capacity, Ordering::Release);
        true
    }

    /// The complete frames that the reader can take now. Only the reader calls this.
    fn available_frames(&self, channels: usize) -> usize {
        let capacity = self.slots.len();
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        (write + capacity - read) % capacity / channels.max(1)
    }

    /// Drops the oldest complete frames. Only the reader calls this.
    fn skip_frames(&self, frames: usize, channels: usize) {
        if frames == 0 {
            return;
        }
        let capacity = self.slots.len();
        let read = self.read.load(Ordering::Relaxed);
        self.read
            .store((read + frames * channels) % capacity, Ordering::Release);
    }

    /// Reads one complete frame, and returns whether the ring had one. Without one, the frame is
    /// silent and the ring does not change.
    fn pop_frame(&self, frame: &mut [f32]) -> bool {
        let capacity = self.slots.len();
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        if (write + capacity - read) % capacity < frame.len() {
            frame.fill(0.0);
            return false;
        }
        for (offset, sample) in frame.iter_mut().enumerate() {
            let slot = &self.slots[(read + offset) % capacity];
            *sample = f32::from_bits(slot.load(Ordering::Relaxed));
        }
        self.read
            .store((read + frame.len()) % capacity, Ordering::Release);
        true
    }

    /// Copies the complete frame `offset` frames after the read point, and does not move the read
    /// point. Returns false when the ring has no such frame. Only the reader calls this.
    fn peek_frame(&self, offset: usize, frame: &mut [f32]) -> bool {
        let capacity = self.slots.len();
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        let start = offset * frame.len();
        if (write + capacity - read) % capacity < start + frame.len() {
            return false;
        }
        for (index, sample) in frame.iter_mut().enumerate() {
            let slot = &self.slots[(read + start + index) % capacity];
            *sample = f32::from_bits(slot.load(Ordering::Relaxed));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use cpal::{ErrorKind, SampleFormat, SupportedBufferSize, SupportedStreamConfigRange};

    use super::{
        AudioEvent, ConfigError, CpalBackend, InputRing, QUEUE_CLEAN_SECONDS,
        QUEUE_CROSSFADE_FRAMES, QUEUE_MAX_CLEAN_SECONDS, QueueReader, QueueTrim, choose_config,
        host_error, host_error_event, system_route, usable_ranges,
    };
    #[cfg(target_os = "linux")]
    use super::{PULSE_COOKIE_BYTES, ensure_pulse_cookie};
    use crate::audio::{AudioBackendKind, AudioError};

    /// One range for one rate, as ALSA gives it.
    fn range(channels: u16, rate: u32, format: SampleFormat) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            rate,
            rate,
            SupportedBufferSize::Range { min: 64, max: 2048 },
            format,
        )
    }

    /// A card that gives its smallest rate first, which hid 48 kHz from the first version.
    fn alsa_card() -> Vec<SupportedStreamConfigRange> {
        vec![
            range(2, 44_100, SampleFormat::I32),
            range(2, 48_000, SampleFormat::I32),
            range(2, 96_000, SampleFormat::I32),
            range(4, 44_100, SampleFormat::I32),
            range(4, 48_000, SampleFormat::I32),
        ]
    }

    #[test]
    fn a_rate_in_a_later_range_is_found() {
        let (config, format) =
            choose_config(&alsa_card(), 2, Some(48_000), None, None).expect("config");

        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.channels, 2);
        assert_eq!(format, SampleFormat::I32);
    }

    #[test]
    fn a_rate_that_no_range_holds_is_refused() {
        assert_eq!(
            choose_config(&alsa_card(), 4, Some(96_000), None, None).err(),
            Some(ConfigError::NoRate(96_000))
        );
    }

    #[test]
    fn with_no_request_the_rate_of_the_server_wins_and_then_48_khz() {
        let any_rate = [SupportedStreamConfigRange::new(
            2,
            1,
            384_000,
            SupportedBufferSize::Range { min: 1, max: 9000 },
            SampleFormat::F32,
        )];
        let server = choose_config(&any_rate, 2, None, Some(96_000), None).expect("server");
        assert_eq!(server.0.sample_rate, 96_000);

        let no_server = choose_config(&any_rate, 2, None, None, None).expect("no server");
        assert_eq!(no_server.0.sample_rate, 48_000);
    }

    #[test]
    fn the_buffer_keeps_the_default_inside_the_limits_of_the_device() {
        let (config, _) = choose_config(&alsa_card(), 2, None, None, None).expect("config");
        assert_eq!(config.buffer_size, cpal::BufferSize::Fixed(256));
        let (config, _) = choose_config(&alsa_card(), 2, None, None, Some(8)).expect("config");
        assert_eq!(config.buffer_size, cpal::BufferSize::Fixed(64));
    }

    #[test]
    fn the_best_format_wins_for_the_chosen_rate() {
        let ranges = [
            range(2, 48_000, SampleFormat::I16),
            range(2, 48_000, SampleFormat::F32),
        ];
        let (_, format) = choose_config(&ranges, 2, Some(48_000), None, None).expect("config");
        assert_eq!(format, SampleFormat::F32);
    }

    #[test]
    fn a_device_with_other_layouts_gives_the_nearest_that_fits() {
        let ranges = [
            range(1, 48_000, SampleFormat::F32),
            range(4, 48_000, SampleFormat::F32),
        ];
        assert_eq!(usable_ranges(&ranges, 2)[0].channels(), 1);
        assert_eq!(usable_ranges(&ranges[1..], 2)[0].channels(), 4);
        assert!(usable_ranges(&[], 2).is_empty());
    }

    #[test]
    fn a_busy_device_and_a_lost_device_have_their_own_messages_with_the_label() {
        let busy = host_error(&ErrorKind::DeviceBusy.into(), "Fake Interface");
        assert!(matches!(busy, AudioError::DeviceBusy(_)));
        assert!(busy.to_string().contains("Fake Interface"), "{busy}");
        assert!(busy.to_string().contains("another program"), "{busy}");

        let lost = host_error(&ErrorKind::DeviceNotAvailable.into(), "Fake Interface");
        assert!(matches!(lost, AudioError::DeviceGone(_)));
        assert!(lost.to_string().contains("not connected"), "{lost}");
    }

    #[test]
    fn a_dummy_kind_has_no_cpal_host() {
        let backend = CpalBackend::new(AudioBackendKind::Dummy);
        assert!(matches!(
            backend.new_host(),
            Err(AudioError::InvalidConfig(text)) if text.contains("dummy")
        ));
    }

    /// A Linux driver on another operating system is an error before any host starts.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_linux_driver_has_no_host_on_another_system() {
        for kind in [AudioBackendKind::Alsa, AudioBackendKind::Jack] {
            let backend = CpalBackend::new(kind);
            assert!(matches!(
                backend.new_host(),
                Err(AudioError::InvalidConfig(text)) if text.contains("only on Linux")
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_pulse_cookie_is_made_one_time_and_an_old_cookie_stays() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = tempfile::tempdir().expect("home");
        ensure_pulse_cookie(home.path()).expect("cookie");
        let path = home.path().join(".config/pulse/cookie");
        let first = std::fs::read(&path).expect("read");
        assert_eq!(first.len(), PULSE_COOKIE_BYTES);
        let mode = std::fs::metadata(&path).expect("mode").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "only the user can read a login cookie");

        ensure_pulse_cookie(home.path()).expect("second call");
        assert_eq!(std::fs::read(&path).expect("read"), first);

        let old_home = tempfile::tempdir().expect("home");
        std::fs::write(old_home.path().join(".pulse-cookie"), b"old").expect("old cookie");
        ensure_pulse_cookie(old_home.path()).expect("old cookie stays");
        assert!(!old_home.path().join(".config/pulse/cookie").exists());
    }

    #[test]
    fn a_system_route_with_no_server_says_so_and_runs_on_alsa() {
        assert_eq!(system_route(false).active, None);
        let route = system_route(true);
        assert_eq!(route.active, Some(AudioBackendKind::Alsa));
        assert!(route.notice.expect("notice").contains("ALSA"));
    }

    #[test]
    fn only_a_lost_stream_is_fatal_and_common_host_notices_are_not() {
        assert_eq!(host_error_event(ErrorKind::RealtimeDenied), None);
        assert_eq!(host_error_event(ErrorKind::DeviceChanged), None);
        assert_eq!(
            host_error_event(ErrorKind::Xrun),
            Some(AudioEvent::OutputUnderrun)
        );
        assert_eq!(
            host_error_event(ErrorKind::DeviceNotAvailable),
            Some(AudioEvent::DeviceDisconnected)
        );
        assert_eq!(
            host_error_event(ErrorKind::StreamInvalidated),
            Some(AudioEvent::DeviceDisconnected),
            "a sound server that goes away is a lost device"
        );
        assert_eq!(
            host_error_event(ErrorKind::BackendError),
            Some(AudioEvent::StreamFailed)
        );
    }

    #[test]
    fn frames_that_stay_in_the_queue_for_a_whole_window_go_and_a_short_peak_stays() {
        const CALLBACKS_IN_A_WINDOW: usize = 48_000 / 256 + 1;
        // OPTIMIZE is off, so that the queue keeps two callbacks.
        let mut queue = QueueTrim::new(256, 48_000, false);

        // A burst at the start of the window does not count.
        assert_eq!(queue.excess(2000, 256, false), 0);
        let mut last = 0;
        for _ in 1..CALLBACKS_IN_A_WINDOW {
            last = queue.excess(512, 256, false);
        }
        assert_eq!(last, 0, "the lowest point of this window is the limit");

        // A queue that stays long loses only what is above the limit.
        let mut dropped = 0;
        for _ in 0..CALLBACKS_IN_A_WINDOW {
            dropped += queue.excess(1000, 256, false);
        }
        assert_eq!(dropped, 1000 - 512);
        assert_eq!(queue.level, 512, "the frames that stay after the shrink");
    }

    const RATE: usize = 48_000;
    const CALLBACK: usize = 256;
    const CALLBACKS_IN_A_SECOND: usize = RATE / CALLBACK + 1;
    /// The peak and the frequency of the test note.
    const PEAK: f32 = 0.5;
    const HERTZ: f32 = 440.0;

    /// The test note at one captured frame.
    fn sine(index: usize) -> f32 {
        PEAK * (std::f32::consts::TAU * HERTZ * index as f32 / RATE as f32).sin()
    }

    fn largest_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_shrink_of_the_queue_is_a_crossfade_with_no_click() {
        // A queue that stays 54 frames above its limit loses them at the end of the first
        // window. 54 frames are half a period of the note, so a hard jump goes to the opposite
        // side of the wave.
        const EXCESS: usize = 54;
        let ring = InputRing::new(4096);
        let mut reader = QueueReader::new(QueueTrim::new(CALLBACK, RATE, false), 1, true);
        let mut written = 0;
        let mut push = |ring: &InputRing, frames: usize| {
            for _ in 0..frames {
                assert!(ring.push_frame(std::iter::once(sine(written))));
                written += 1;
            }
        };
        // The reader primes one callback, so that the queue holds two callbacks and the excess.
        push(&ring, CALLBACK + EXCESS);
        let mut output = Vec::new();
        let mut read_point = 0;
        let mut largest_gap = 0.0_f32;
        let mut shrinks = 0;
        // One window, and the callbacks that the crossfade needs after its end.
        for _ in 0..CALLBACKS_IN_A_SECOND + 4 {
            push(&ring, CALLBACK);
            let before = ring.available_frames(1);
            let mut captured = [f32::NAN; CALLBACK];
            assert!(!reader.read(&ring, &mut captured, false), "no underrun");
            let consumed = before - ring.available_frames(1);
            let read = if output.is_empty() { 0 } else { CALLBACK };
            if consumed > read {
                assert_eq!(consumed - read, EXCESS, "the shrink of the first window");
                shrinks += 1;
                // The two read points differ by up to the full height of the wave.
                for frame in 0..QUEUE_CROSSFADE_FRAMES {
                    let gap = (sine(read_point + EXCESS + frame) - sine(read_point + frame)).abs();
                    largest_gap = largest_gap.max(gap);
                }
            }
            read_point += consumed;
            output.extend_from_slice(&captured);
        }
        assert_eq!(shrinks, 1);
        assert!(largest_gap > 0.9, "a hard jump would click: {largest_gap}");

        // A sine moves at most `PEAK * TAU * HERTZ / RATE` in one frame, and a linear crossfade
        // over N frames adds at most the gap between the two read points over N + 1.
        let slope = PEAK * std::f32::consts::TAU * HERTZ / RATE as f32;
        let bound = slope + 2.0 * PEAK / (QUEUE_CROSSFADE_FRAMES + 1) as f32 + 1e-5;
        let step = largest_step(&output);
        assert!(step <= bound, "a step of {step} is larger than {bound}");
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn with_optimize_on_an_underrun_adds_a_callback_and_a_clean_period_gives_it_back() {
        let mut queue = QueueTrim::new(CALLBACK, RATE, false);
        // The fake clock is the count of played frames. One second ends the warm-up.
        for _ in 0..CALLBACKS_IN_A_SECOND {
            queue.excess(10_000, CALLBACK, true);
        }
        assert_eq!(queue.keep(), CALLBACK, "OPTIMIZE starts at one callback");

        assert!(queue.underrun());
        assert_eq!(queue.keep(), 2 * CALLBACK, "an underrun adds one callback");

        // 30 s is 5625 callbacks of 256 frames.
        let clean_callbacks = 30 * RATE / CALLBACK;
        for _ in 0..clean_callbacks - 1 {
            queue.excess(10_000, CALLBACK, true);
        }
        assert_eq!(queue.keep(), 2 * CALLBACK, "29.99 s is not a clean period");
        queue.excess(10_000, CALLBACK, true);
        assert_eq!(
            queue.keep(),
            CALLBACK,
            "30 s with no underrun gives it back"
        );
        for _ in 0..30 * CALLBACKS_IN_A_SECOND {
            queue.excess(10_000, CALLBACK, true);
        }
        assert_eq!(queue.keep(), CALLBACK, "one callback is the least");

        for _ in 0..5 {
            assert!(queue.underrun());
        }
        assert_eq!(queue.keep(), 3 * CALLBACK, "three callbacks are the most");
    }

    #[test]
    fn with_optimize_off_the_queue_keeps_two_callbacks() {
        let mut queue = QueueTrim::new(CALLBACK, RATE, false);
        for _ in 0..CALLBACKS_IN_A_SECOND {
            queue.excess(10_000, CALLBACK, false);
        }
        assert_eq!(queue.keep(), 2 * CALLBACK);
        assert!(queue.underrun(), "the underrun counts");
        assert_eq!(queue.keep(), 2 * CALLBACK, "and the queue stays");
        for _ in 0..31 * CALLBACKS_IN_A_SECOND {
            queue.excess(10_000, CALLBACK, false);
        }
        assert_eq!(queue.keep(), 2 * CALLBACK, "no clean period changes it");

        // OPTIMIZE on starts at one callback again, and off goes back to two.
        queue.excess(10_000, CALLBACK, true);
        assert_eq!(queue.keep(), CALLBACK);
        queue.excess(10_000, CALLBACK, false);
        assert_eq!(queue.keep(), 2 * CALLBACK);
    }

    /// Plays callbacks of [`CALLBACK`] frames for this many seconds with OPTIMIZE on. The fake
    /// clock is the count of played frames.
    fn play_seconds(queue: &mut QueueTrim, seconds: usize) {
        for _ in 0..(seconds * RATE).div_ceil(CALLBACK) {
            queue.excess(10_000, CALLBACK, true);
        }
    }

    /// A queue with OPTIMIZE on, after its warm-up.
    fn warm_queue() -> QueueTrim {
        let mut queue = QueueTrim::new(CALLBACK, RATE, false);
        play_seconds(&mut queue, 1);
        queue
    }

    #[test]
    fn an_underrun_soon_after_a_step_down_doubles_the_next_clean_period() {
        let mut queue = warm_queue();
        assert!(queue.underrun());
        assert_eq!(queue.keep(), 2 * CALLBACK);
        play_seconds(&mut queue, 30);
        assert_eq!(queue.keep(), CALLBACK, "30 s clean: the step down");

        play_seconds(&mut queue, 5);
        assert!(queue.underrun(), "1 callback is too few for this system");
        assert_eq!(queue.keep(), 2 * CALLBACK);
        play_seconds(&mut queue, 30);
        assert_eq!(queue.keep(), 2 * CALLBACK, "the next step down needs 60 s");
        play_seconds(&mut queue, 30);
        assert_eq!(queue.keep(), CALLBACK, "60 s clean in total");
    }

    #[test]
    fn a_repeated_cycle_doubles_the_clean_period_up_to_8_minutes() {
        let mut queue = warm_queue();
        assert!(queue.underrun());
        let mut periods = Vec::new();
        for _ in 0..8 {
            let period = queue.clean_limit / RATE;
            periods.push(period);
            play_seconds(&mut queue, period);
            assert_eq!(queue.keep(), CALLBACK, "a step down after {period} s");
            play_seconds(&mut queue, 5);
            assert!(queue.underrun());
            assert_eq!(queue.keep(), 2 * CALLBACK);
        }
        assert_eq!(periods, [30, 60, 120, 240, 480, 480, 480, 480]);
        assert_eq!(queue.clean_limit, QUEUE_MAX_CLEAN_SECONDS * RATE);
    }

    #[test]
    fn an_underrun_long_after_a_step_down_does_not_double_the_clean_period() {
        let mut queue = warm_queue();
        assert!(queue.underrun());
        play_seconds(&mut queue, 30);
        assert_eq!(queue.keep(), CALLBACK, "the step down");

        play_seconds(&mut queue, 31);
        assert!(
            queue.underrun(),
            "more than one clean period after the step down"
        );
        assert_eq!(queue.keep(), 2 * CALLBACK);
        assert_eq!(queue.clean_limit, QUEUE_CLEAN_SECONDS * RATE);
        play_seconds(&mut queue, 30);
        assert_eq!(queue.keep(), CALLBACK, "30 s is still enough");
    }

    #[test]
    fn a_change_of_optimize_sets_the_clean_period_back_to_30_s() {
        let mut queue = warm_queue();
        assert!(queue.underrun());
        play_seconds(&mut queue, 30);
        play_seconds(&mut queue, 5);
        assert!(queue.underrun());
        assert_eq!(queue.clean_limit, 60 * RATE, "doubled");

        queue.excess(10_000, CALLBACK, false);
        queue.excess(10_000, CALLBACK, true);
        assert_eq!(queue.clean_limit, QUEUE_CLEAN_SECONDS * RATE);
        assert!(queue.underrun());
        assert_eq!(queue.keep(), 2 * CALLBACK);
        play_seconds(&mut queue, 30);
        assert_eq!(queue.keep(), CALLBACK, "30 s again");
        assert_eq!(
            QueueTrim::new(CALLBACK, RATE, false).clean_limit,
            QUEUE_CLEAN_SECONDS * RATE,
            "a new queue starts at 30 s"
        );
    }

    #[test]
    fn an_underrun_in_the_warm_up_does_not_count() {
        let mut queue = QueueTrim::new(CALLBACK, RATE, false);
        queue.excess(0, CALLBACK, true);
        assert!(!queue.underrun(), "the streams start at different times");
        assert_eq!(queue.keep(), CALLBACK);
    }

    #[test]
    fn an_empty_queue_is_an_underrun_only_with_a_capture_stream() {
        let ring = InputRing::new(4096);
        let mut with_capture = QueueReader::new(QueueTrim::new(CALLBACK, RATE, false), 1, true);
        let mut no_capture = QueueReader::new(QueueTrim::new(CALLBACK, RATE, false), 1, false);
        let mut captured = [f32::NAN; CALLBACK];
        let mut counted = (0, 0);
        for _ in 0..CALLBACKS_IN_A_SECOND {
            counted.0 += usize::from(with_capture.read(&ring, &mut captured, true));
            counted.1 += usize::from(no_capture.read(&ring, &mut captured, true));
        }
        assert!(
            counted.0 > 0,
            "an empty queue after the warm-up is an underrun"
        );
        assert_eq!(with_capture.trim.keep(), 3 * CALLBACK);
        assert_eq!(counted.1, 0, "with no capture, silence is all there is");
        assert_eq!(captured, [0.0; CALLBACK]);
    }

    #[test]
    fn a_host_chosen_buffer_sizes_the_queue_from_the_real_callbacks() {
        // The chain was prepared for 1024 frames, and the host gives 256.
        let mut queue = QueueTrim::new(1024, RATE, true);
        queue.excess(10_000, CALLBACK, false);
        assert_eq!(queue.keep(), 2 * CALLBACK, "two callbacks of 256");
        queue.excess(10_000, CALLBACK, true);
        assert_eq!(queue.keep(), CALLBACK, "one callback of 256");

        // A fixed buffer keeps the prepared size.
        let mut fixed = QueueTrim::new(1024, RATE, false);
        fixed.excess(10_000, CALLBACK, false);
        assert_eq!(fixed.keep(), 2 * 1024);
    }

    #[test]
    fn the_first_callback_plays_one_real_callback_of_silence() {
        for (prepared, host_chosen) in [(1024, true), (CALLBACK, false)] {
            let ring = InputRing::new(4096);
            for index in 0..2 * CALLBACK {
                assert!(ring.push_frame(std::iter::once(sine(index))));
            }
            let mut reader = QueueReader::new(QueueTrim::new(prepared, RATE, host_chosen), 1, true);
            let mut first = [f32::NAN; CALLBACK];
            reader.read(&ring, &mut first, false);
            assert_eq!(first, [0.0; CALLBACK], "{prepared}");
            assert_eq!(
                ring.available_frames(1),
                2 * CALLBACK,
                "the silence reads no frame"
            );
            let mut second = [f32::NAN; CALLBACK];
            reader.read(&ring, &mut second, false);
            let expected: Vec<f32> = (0..CALLBACK).map(sine).collect();
            assert_eq!(second.as_slice(), expected, "{prepared}");
        }
    }

    #[test]
    fn a_peek_reads_a_frame_after_the_read_point_and_does_not_move_it() {
        let ring = InputRing::new(13);
        for index in 0..3 {
            assert!(push(&ring, stereo(index)));
        }
        let mut frame = [f32::NAN; 2];
        assert!(ring.peek_frame(2, &mut frame));
        assert_eq!(frame, stereo(2));
        assert!(!ring.peek_frame(3, &mut frame), "no fourth frame");
        assert_eq!(ring.available_frames(2), 3);
        assert_eq!(pop(&ring), stereo(0));
    }

    #[test]
    fn the_queue_drops_complete_frames_from_its_old_end() {
        let ring = InputRing::new(13);
        for index in 0..4 {
            assert!(push(&ring, stereo(index)));
        }
        assert_eq!(ring.available_frames(2), 4);

        ring.skip_frames(3, 2);

        assert_eq!(ring.available_frames(2), 1);
        assert_eq!(pop(&ring), stereo(3), "left stays left");
    }

    /// A stereo frame whose left sample is above zero and whose right sample is below zero.
    fn stereo(index: u16) -> [f32; 2] {
        [f32::from(index) + 1.0, -f32::from(index) - 1.0]
    }

    fn push(ring: &InputRing, frame: [f32; 2]) -> bool {
        ring.push_frame(frame.into_iter())
    }

    fn pop(ring: &InputRing) -> [f32; 2] {
        // The sentinel shows whether the ring wrote the whole frame.
        let mut frame = [f32::NAN; 2];
        ring.pop_frame(&mut frame);
        frame
    }

    #[test]
    fn ring_preserves_order_and_reports_full_and_empty_states() {
        let ring = InputRing::new(2);
        let mut frame = [1.0];
        ring.pop_frame(&mut frame);
        assert_eq!(frame, [0.0]);
        assert!(ring.push_frame([0.25].into_iter()));
        assert!(!ring.push_frame([0.5].into_iter()));
        ring.pop_frame(&mut frame);
        assert_eq!(frame, [0.25]);
        ring.pop_frame(&mut frame);
        assert_eq!(frame, [0.0]);
    }

    #[test]
    fn left_stays_left_after_an_underrun() {
        let ring = InputRing::new(9);
        assert!(push(&ring, stereo(0)));
        assert_eq!(pop(&ring), stereo(0));

        assert_eq!(pop(&ring), [0.0, 0.0], "an empty ring gives silence");
        assert_eq!(pop(&ring), [0.0, 0.0]);

        assert!(push(&ring, stereo(1)));
        assert_eq!(pop(&ring), stereo(1));
    }

    #[test]
    fn a_full_ring_refuses_the_whole_frame_and_left_stays_left() {
        // One frame and one sample fit, so a sample ring would take a half of the second frame.
        let ring = InputRing::new(4);
        assert!(push(&ring, stereo(0)));
        assert!(!push(&ring, stereo(1)));

        assert_eq!(pop(&ring), stereo(0));
        assert_eq!(pop(&ring), [0.0, 0.0], "no part of the refused frame stays");

        // These pushes also cross the end of the slots.
        for index in 2..6 {
            assert!(push(&ring, stereo(index)));
            assert_eq!(pop(&ring), stereo(index));
        }
    }

    #[test]
    fn a_frame_that_is_not_complete_stays_in_the_ring() {
        let ring = InputRing::new(9);
        let mut quad = [f32::NAN; 4];
        assert!(push(&ring, stereo(0)));
        ring.pop_frame(&mut quad);
        assert_eq!(quad, [0.0; 4]);
        assert_eq!(pop(&ring), stereo(0), "the silent frame took no samples");
    }

    #[test]
    fn left_stays_left_while_two_threads_use_the_ring() {
        const FRAMES: u16 = 50_000;
        let ring = std::sync::Arc::new(InputRing::new(9));
        let writer = {
            let ring = std::sync::Arc::clone(&ring);
            std::thread::spawn(move || {
                let mut index = 0;
                while index < FRAMES {
                    if push(&ring, stereo(index)) {
                        index += 1;
                    }
                }
            })
        };
        let mut expected = 0;
        while expected < FRAMES {
            let frame = pop(&ring);
            if frame != [0.0, 0.0] {
                assert_eq!(frame, stereo(expected));
                expected += 1;
            }
        }
        writer.join().expect("writer");
    }
}
