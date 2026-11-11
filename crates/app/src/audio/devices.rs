use std::{
    ops::RangeInclusive,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{
    AudioBackendKind, AudioError, AudioTelemetry, AudioWorker, COMMON_SAMPLE_RATES,
    DEFAULT_BUFFER_FRAMES, DeviceConfig, SYSTEM_DEFAULT_DEVICE,
};
use crate::audio_settings::StandaloneAudioSettings;
use ronie_dsp::rt_chain::ChainCommandSender;

/// One device of a host, discovered with no stream and with no open of the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    /// Stable identity in the host. It can hold a serial number, so a log must not show it.
    pub id: String,
    /// The text that the user sees.
    pub label: String,
    /// Whether capture is available.
    pub input: bool,
    /// Whether playback is available.
    pub output: bool,
    /// Whether the host names this device as its default input.
    pub default_input: bool,
    /// Whether the host names this device as its default output.
    pub default_output: bool,
    /// Whether this device has a row of its own in a list.
    pub listed: bool,
}

/// What one direction of one device can do. Finding this can open the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCapabilities {
    /// The common sample rates that the device accepts.
    pub sample_rates: Vec<u32>,
    /// The rate that the device or its server uses now, when it has one.
    pub device_rate: Option<u32>,
    /// The real channel count.
    pub channels: usize,
    /// The smallest and the largest callback that the device gives, when it says.
    pub buffer_frames: Option<RangeInclusive<u32>>,
}

/// How a backend reaches the devices at this moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Route {
    /// The kind that really runs, when it is not the kind that was asked for.
    pub active: Option<AudioBackendKind>,
    /// A message for the user about a route that changed.
    pub notice: Option<String>,
}

/// The stream that a device will actually run for one request, known before it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedStream {
    /// Configuration to open. Its rate is final. Its buffer is `None` if the host chooses.
    pub config: DeviceConfig,
    /// Final device sample rate.
    pub sample_rate: u32,
    /// Largest callback to prepare the audio chain for.
    pub max_callback_frames: u32,
    /// Label of the actual capture device, or `None` when no capture device exists.
    pub input_device: Option<String>,
    /// Label of the actual playback device.
    pub output_device: Option<String>,
    /// The channels that the capture device delivers in each frame.
    pub capture_channels: usize,
    /// The channels that the playback device takes in each frame.
    pub playback_channels: usize,
}

impl NegotiatedStream {
    /// Configuration with the final rate and callback bound, used to prepare the worker.
    #[must_use]
    pub fn worker_config(&self) -> DeviceConfig {
        DeviceConfig {
            sample_rate: Some(self.sample_rate),
            buffer_frames: Some(self.max_callback_frames),
            ..self.config.clone()
        }
    }
}

/// Common stream interface implemented by dummy and operating-system backends.
pub trait AudioBackend {
    /// Finds how to reach the devices now. This can wait for a sound server to answer, and it
    /// comes before the other calls of a device job.
    fn connect(&mut self) -> Route {
        Route::default()
    }

    /// Discovers currently connected devices, with no open of a device.
    ///
    /// # Errors
    /// Returns a backend error when enumeration fails.
    fn enumerate(&self) -> Result<Vec<DeviceDescriptor>, AudioError>;

    /// Finds what one direction of one device can do. `None` asks for the default device.
    ///
    /// # Errors
    /// Returns a disconnected, busy, or backend error.
    fn capabilities(
        &self,
        device: Option<&str>,
        input: bool,
    ) -> Result<DeviceCapabilities, AudioError>;

    /// Resolves the final stream for one request without opening it.
    ///
    /// # Errors
    /// Returns an invalid-config, disconnected, or backend error.
    fn negotiate(&self, requested: &DeviceConfig) -> Result<NegotiatedStream, AudioError>;

    /// Opens a stream with exclusive callback-worker ownership.
    ///
    /// # Errors
    /// Returns an invalid-config, disconnected, or backend error.
    fn open(
        &mut self,
        config: &DeviceConfig,
        worker: AudioWorker,
    ) -> Result<AudioStream, AudioError>;
}

/// Open stream returned by any backend.
pub enum AudioStream {
    /// Deterministic in-memory stream.
    Dummy(Box<DummyStream>),
    /// Real CPAL input/output pair.
    Cpal(Box<CpalStream>),
}

impl AudioStream {
    /// Starts rendering.
    ///
    /// # Errors
    /// Returns a backend error when the stream cannot start.
    pub fn start(&mut self) -> Result<(), AudioError> {
        match self {
            Self::Dummy(stream) => stream.start(),
            Self::Cpal(stream) => stream.start(),
        }
    }

    /// Stops rendering. Dropping the stream releases its backend resources.
    ///
    /// # Errors
    /// Returns a backend error when the stream cannot stop cleanly.
    pub fn close(&self) -> Result<(), AudioError> {
        match self {
            Self::Dummy(stream) => stream.close(),
            Self::Cpal(stream) => stream.close(),
        }
    }

    /// Bounded control endpoint of the callback that owns the chain.
    #[must_use]
    pub fn commands(&self) -> &ChainCommandSender {
        match self {
            Self::Dummy(stream) => &stream.commands,
            Self::Cpal(stream) => &stream.commands,
        }
    }

    /// Lock-free telemetry that the callback publishes.
    #[must_use]
    pub fn telemetry(&self) -> &AudioTelemetry {
        match self {
            Self::Dummy(stream) => &stream.telemetry,
            Self::Cpal(stream) => &stream.telemetry,
        }
    }

    /// A handle to the telemetry, for a reader that must not hold the engine lock.
    #[must_use]
    pub fn shared_telemetry(&self) -> Arc<AudioTelemetry> {
        match self {
            Self::Dummy(stream) => Arc::clone(&stream.telemetry),
            Self::Cpal(stream) => Arc::clone(&stream.telemetry),
        }
    }

    /// The failure that ended this stream's audio, if any.
    #[must_use]
    pub fn failure(&self) -> Option<AudioError> {
        if let Self::Dummy(stream) = self
            && !stream.device.is_connected()
        {
            return Some(AudioError::Disconnected);
        }
        let telemetry = self.telemetry();
        if !telemetry.stream_failed() {
            return None;
        }
        Some(
            if telemetry.last_event() == Some(super::AudioEvent::DeviceDisconnected) {
                AudioError::Disconnected
            } else {
                AudioError::Backend("the audio stream stopped unexpectedly".to_owned())
            },
        )
    }

    /// Renders one deterministic dummy callback with the supplied interleaved input.
    ///
    /// # Errors
    /// Returns an invalid-config error for CPAL streams or malformed input, or a processing error.
    pub fn render_with_input(&mut self, input: &[f32]) -> Result<&[f32], AudioError> {
        match self {
            Self::Dummy(stream) => stream.render_with_input(input),
            Self::Cpal(_) => Err(AudioError::InvalidConfig(
                "deterministic input requires the dummy backend".to_owned(),
            )),
        }
    }

    /// Renders one deterministic dummy callback with all channels of the device, as a host gives
    /// them. The route of the stream selects from these channels.
    ///
    /// # Errors
    /// Returns an invalid-config error for CPAL streams or malformed input, or a processing error.
    pub fn render_capture(&mut self, capture: &[f32]) -> Result<&[f32], AudioError> {
        match self {
            Self::Dummy(stream) => stream.render_capture(capture),
            Self::Cpal(_) => Err(AudioError::InvalidConfig(
                "deterministic input requires the dummy backend".to_owned(),
            )),
        }
    }

    /// Requested stream configuration.
    #[must_use]
    pub fn config(&self) -> &DeviceConfig {
        match self {
            Self::Dummy(stream) => stream.config(),
            Self::Cpal(stream) => stream.config(),
        }
    }
}

/// Device capabilities of a deterministic dummy backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DummyLimits {
    /// Sample rates that the device accepts. An empty list accepts any nonzero rate.
    pub sample_rates: Vec<u32>,
    /// Rate that the device uses when the request has no rate.
    pub default_sample_rate: u32,
    /// Callback sizes that the device accepts. A request outside this range is clamped.
    pub buffer_frames: RangeInclusive<u32>,
    /// The channels that the capture device delivers in each frame.
    pub input_channels: usize,
    /// The channels that the playback device takes in each frame.
    pub output_channels: usize,
    /// The devices of this host. An empty list means one device for each direction.
    pub devices: Vec<DeviceDescriptor>,
}

impl Default for DummyLimits {
    fn default() -> Self {
        Self {
            sample_rates: Vec::new(),
            default_sample_rate: 48_000,
            buffer_frames: 1..=u32::MAX,
            input_channels: 2,
            output_channels: 2,
            devices: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct DummyDevice {
    connected: AtomicBool,
    start_fails: AtomicBool,
    limits: DummyLimits,
}

impl DummyDevice {
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

/// Deterministic audio backend with controllable connection state.
///
/// Clones control the same device, so a test can lose a device that an open stream uses.
#[derive(Debug, Clone)]
pub struct DummyBackend {
    device: Arc<DummyDevice>,
}

impl DummyBackend {
    /// Creates a connected backend.
    #[must_use]
    pub fn connected() -> Self {
        Self::with_limits(DummyLimits::default())
    }

    /// Creates a disconnected backend.
    #[must_use]
    pub fn disconnected() -> Self {
        let backend = Self::connected();
        backend.set_connected(false);
        backend
    }

    /// Creates a connected backend with explicit device capabilities.
    #[must_use]
    pub fn with_limits(limits: DummyLimits) -> Self {
        Self {
            device: Arc::new(DummyDevice {
                connected: AtomicBool::new(true),
                start_fails: AtomicBool::new(false),
                limits,
            }),
        }
    }

    /// Changes device availability for recovery tests, including for an open stream.
    pub fn set_connected(&self, connected: bool) {
        self.device.connected.store(connected, Ordering::Release);
    }

    /// Makes later stream starts fail for recovery tests.
    pub fn set_start_fails(&self, fails: bool) {
        self.device.start_fails.store(fails, Ordering::Release);
    }

    /// Opens a concrete dummy stream.
    ///
    /// # Errors
    /// Returns invalid-config or disconnected errors.
    pub fn open_dummy(&mut self, config: &DeviceConfig) -> Result<DummyStream, AudioError> {
        if !self.device.is_connected() {
            return Err(AudioError::Disconnected);
        }
        let worker = AudioWorker::new(config)?;
        Ok(DummyStream::new(
            config.clone(),
            worker,
            Arc::clone(&self.device),
        ))
    }
}

impl DummyBackend {
    /// Opens a dummy stream through the common backend contract.
    ///
    /// # Errors
    /// Returns invalid-config or disconnected errors.
    pub fn open(
        &mut self,
        config: &DeviceConfig,
        worker: AudioWorker,
    ) -> Result<AudioStream, AudioError> {
        <Self as AudioBackend>::open(self, config, worker)
    }
}

impl AudioBackend for DummyBackend {
    fn enumerate(&self) -> Result<Vec<DeviceDescriptor>, AudioError> {
        if !self.device.is_connected() {
            return Err(AudioError::Disconnected);
        }
        let limits = &self.device.limits;
        if !limits.devices.is_empty() {
            return Ok(limits.devices.clone());
        }
        Ok(vec![DeviceDescriptor {
            id: DUMMY_DEVICE_NAME.to_owned(),
            label: DUMMY_DEVICE_NAME.to_owned(),
            input: true,
            output: true,
            default_input: true,
            default_output: true,
            listed: true,
        }])
    }

    fn capabilities(
        &self,
        _device: Option<&str>,
        input: bool,
    ) -> Result<DeviceCapabilities, AudioError> {
        if !self.device.is_connected() {
            return Err(AudioError::Disconnected);
        }
        let limits = &self.device.limits;
        Ok(DeviceCapabilities {
            sample_rates: COMMON_SAMPLE_RATES
                .iter()
                .copied()
                .filter(|rate| limits.sample_rates.is_empty() || limits.sample_rates.contains(rate))
                .collect(),
            device_rate: Some(limits.default_sample_rate),
            channels: if input {
                limits.input_channels
            } else {
                limits.output_channels
            },
            buffer_frames: Some(limits.buffer_frames.clone()),
        })
    }

    fn negotiate(&self, requested: &DeviceConfig) -> Result<NegotiatedStream, AudioError> {
        validate_dummy_request(&self.device, requested)?;
        let limits = &self.device.limits;
        let sample_rate = requested.sample_rate.unwrap_or(limits.default_sample_rate);
        if !limits.sample_rates.is_empty() && !limits.sample_rates.contains(&sample_rate) {
            return Err(AudioError::InvalidConfig(format!(
                "sample rate {sample_rate} is not supported by the dummy device"
            )));
        }
        let frames = requested
            .buffer_frames
            .unwrap_or(DEFAULT_BUFFER_FRAMES)
            .clamp(*limits.buffer_frames.start(), *limits.buffer_frames.end());
        let mut config = requested.clone();
        fit_layout(&mut config, limits.input_channels, limits.output_channels);
        config.sample_rate = Some(sample_rate);
        config.buffer_frames = Some(frames);
        let label = |device: &Option<String>| {
            device
                .clone()
                .filter(|id| id != SYSTEM_DEFAULT_DEVICE)
                .unwrap_or_else(|| DUMMY_DEVICE_NAME.to_owned())
        };
        Ok(NegotiatedStream {
            sample_rate,
            max_callback_frames: frames,
            input_device: Some(label(&requested.input_device)),
            output_device: Some(label(&requested.output_device)),
            capture_channels: limits.input_channels,
            playback_channels: limits.output_channels,
            config,
        })
    }

    fn open(
        &mut self,
        config: &DeviceConfig,
        worker: AudioWorker,
    ) -> Result<AudioStream, AudioError> {
        validate_dummy_request(&self.device, config)?;
        Ok(AudioStream::Dummy(Box::new(DummyStream::new(
            config.clone(),
            worker,
            Arc::clone(&self.device),
        ))))
    }
}

const DUMMY_DEVICE_NAME: &str = "Dummy guitar and monitors";

/// Fits a request to the channels of the device: at most two channels reach the chain, and a route
/// outside the device starts again from the first channel.
pub(super) fn fit_layout(config: &mut DeviceConfig, capture: usize, playback: usize) {
    config.input_channels = config.input_channels.clamp(1, 2).min(capture.max(1));
    config.output_channels = config.output_channels.clamp(1, 2).min(playback.max(1));
    let [first, second] = config.input_route;
    let inside = |channel: usize| channel < capture.max(1);
    config.input_route = if config.input_channels == 2 {
        if inside(first) && inside(second) && first != second {
            [first, second]
        } else {
            [0, 1]
        }
    } else if inside(first) {
        [first, first]
    } else {
        [0, 0]
    };
    if config.output_first + 1 >= playback.max(1) {
        config.output_first = 0;
    }
}

fn validate_dummy_request(device: &DummyDevice, config: &DeviceConfig) -> Result<(), AudioError> {
    if !device.is_connected() {
        return Err(AudioError::Disconnected);
    }
    if config.backend != AudioBackendKind::Dummy {
        return Err(AudioError::InvalidConfig(
            "dummy backend requires the dummy configuration".to_owned(),
        ));
    }
    if config.buffer_frames == Some(0) || config.sample_rate == Some(0) {
        return Err(AudioError::InvalidConfig(
            "rate and buffer must be nonzero".to_owned(),
        ));
    }
    for (id, input) in [(&config.input_device, true), (&config.output_device, false)] {
        let known = id.as_deref().is_none_or(|id| {
            id == SYSTEM_DEFAULT_DEVICE
                || device.limits.devices.is_empty()
                || device
                    .limits
                    .devices
                    .iter()
                    .any(|row| row.id == id && if input { row.input } else { row.output })
        });
        if !known {
            return Err(AudioError::Disconnected);
        }
    }
    Ok(())
}

/// Preallocated dummy audio stream.
pub struct DummyStream {
    config: DeviceConfig,
    worker: AudioWorker,
    commands: ChainCommandSender,
    telemetry: Arc<AudioTelemetry>,
    device: Arc<DummyDevice>,
    input: Vec<f32>,
    output: Vec<f32>,
    /// The channels of the device in each frame of a capture.
    capture_channels: usize,
    started: bool,
    closed: bool,
}

impl DummyStream {
    fn new(config: DeviceConfig, worker: AudioWorker, device: Arc<DummyDevice>) -> Self {
        let frames = config.buffer_frames.unwrap_or(DEFAULT_BUFFER_FRAMES) as usize;
        let commands = worker.command_sender();
        let telemetry = worker.telemetry();
        Self {
            input: vec![0.0; frames * config.input_channels],
            output: vec![0.0; frames * config.output_channels.max(device.limits.output_channels)],
            capture_channels: device.limits.input_channels,
            config,
            worker,
            commands,
            telemetry,
            device,
            started: false,
            closed: false,
        }
    }

    /// Starts dummy rendering.
    ///
    /// # Errors
    /// Returns an error after the stream has been closed or when the device refuses to start.
    pub fn start(&mut self) -> Result<(), AudioError> {
        if self.closed {
            return Err(AudioError::InvalidConfig("stream is closed".to_owned()));
        }
        if self.device.start_fails.load(Ordering::Acquire) {
            return Err(AudioError::Backend(
                "the dummy device refused to start".to_owned(),
            ));
        }
        self.started = true;
        Ok(())
    }

    /// Renders one silent-input callback.
    ///
    /// # Errors
    /// Returns an error when the stream is stopped, closed, or processing fails.
    pub fn render(&mut self) -> Result<&[f32], AudioError> {
        if !self.device.is_connected() {
            return Err(AudioError::Disconnected);
        }
        if self.closed || !self.started {
            return Err(AudioError::InvalidConfig(
                "stream must be open and started".to_owned(),
            ));
        }
        let frames = self.input.len() / self.config.input_channels.max(1);
        let output_len = frames * self.config.output_channels;
        self.worker.process_interleaved(
            &self.input,
            self.config.input_channels,
            &mut self.output[..output_len],
            self.config.output_channels,
        )?;
        Ok(&self.output[..output_len])
    }

    /// Renders one callback with explicit interleaved input.
    ///
    /// # Errors
    /// Returns an invalid-config error for a channel-count mismatch or a processing error.
    pub fn render_with_input(&mut self, input: &[f32]) -> Result<&[f32], AudioError> {
        if input.len() != self.input.len() {
            return Err(AudioError::InvalidConfig(
                "dummy input length must match the stream layout".to_owned(),
            ));
        }
        self.input.copy_from_slice(input);
        self.render()
    }

    /// Renders one callback with all channels of the device, as a host gives them.
    ///
    /// # Errors
    /// Returns an invalid-config error for a malformed capture, or a processing error.
    pub fn render_capture(&mut self, capture: &[f32]) -> Result<&[f32], AudioError> {
        if !self.device.is_connected() {
            return Err(AudioError::Disconnected);
        }
        if self.closed || !self.started {
            return Err(AudioError::InvalidConfig(
                "stream must be open and started".to_owned(),
            ));
        }
        let playback = self.device.limits.output_channels;
        let frames = self.output.len() / playback;
        if capture.len() != frames * self.capture_channels {
            return Err(AudioError::InvalidConfig(
                "dummy capture length must match the device layout".to_owned(),
            ));
        }
        self.worker.process_interleaved(
            capture,
            self.capture_channels,
            &mut self.output,
            playback,
        )?;
        Ok(&self.output)
    }

    /// Stops this stream without waiting for an entire large I/O period.
    ///
    /// # Errors
    /// This dummy close operation is infallible.
    pub fn close(&self) -> Result<(), AudioError> {
        Ok(())
    }

    /// Requested stream configuration.
    #[must_use]
    pub const fn config(&self) -> &DeviceConfig {
        &self.config
    }
}

/// Backend selection, reconnection, and crash-sentinel recovery.
pub struct AudioDeviceManager<B> {
    backend: B,
    last_config: Option<DeviceConfig>,
    used_fallback: bool,
}

impl<B: AudioBackend> AudioDeviceManager<B> {
    /// Creates a manager around one selected backend.
    #[must_use]
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            last_config: None,
            used_fallback: false,
        }
    }

    /// Lists the backend kinds of this operating system.
    #[must_use]
    pub fn backends(&self) -> Vec<AudioBackendKind> {
        AudioBackendKind::available().to_vec()
    }

    /// Enumerates devices on the selected backend.
    ///
    /// # Errors
    /// Returns the backend enumeration error.
    pub fn enumerate(&self) -> Result<Vec<DeviceDescriptor>, AudioError> {
        self.backend.enumerate()
    }

    /// Opens a stream and remembers its configuration for reconnection.
    ///
    /// # Errors
    /// Returns an invalid-config, disconnected, or backend error.
    pub fn open(&mut self, config: &DeviceConfig) -> Result<AudioStream, AudioError> {
        let worker = AudioWorker::new(config)?;
        let stream = self.backend.open(config, worker)?;
        self.last_config = Some(config.clone());
        self.used_fallback = false;
        Ok(stream)
    }

    /// Reopens the last successful configuration.
    ///
    /// # Errors
    /// Returns an error when no stream was previously opened or reconnection fails.
    pub fn reconnect(&mut self) -> Result<AudioStream, AudioError> {
        let config = self.last_config.clone().ok_or_else(|| {
            AudioError::InvalidConfig("no previous audio configuration exists".to_owned())
        })?;
        self.open(&config)
    }

    /// Applies crash-sentinel fallback, marks open attempts, and clears only a clean open.
    ///
    /// # Errors
    /// Returns settings or stream errors. A failed open leaves the sentinel set.
    pub fn open_with_recovery(
        &mut self,
        requested: &DeviceConfig,
        settings: &StandaloneAudioSettings,
    ) -> Result<AudioStream, AudioError> {
        let mut selected = requested.clone();
        if settings.audio_open_sentinel() {
            selected.input_device = None;
            selected.output_device = None;
            self.used_fallback = true;
        }
        settings.mark_open_attempt()?;
        let worker = AudioWorker::new(&selected)?;
        let stream = self.backend.open(&selected, worker)?;
        settings.mark_open_clean()?;
        self.last_config = Some(selected);
        Ok(stream)
    }

    /// Whether the latest recovery used default devices.
    #[must_use]
    pub const fn used_fallback(&self) -> bool {
        self.used_fallback
    }
}

impl AudioDeviceManager<DummyBackend> {
    /// Changes dummy device availability.
    pub fn set_connected(&self, connected: bool) {
        self.backend.set_connected(connected);
    }
}

pub(super) use super::cpal_backend::CpalStream;
