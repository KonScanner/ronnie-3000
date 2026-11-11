use std::{
    sync::Arc,
    thread::ThreadId,
    time::{Duration, Instant},
};

use ronie_core::InputMode;
use ronie_dsp::{
    LatencyReport, PlanarBuffer, PrepareSpec,
    lane::LaneSnapshot,
    prepare::PreparedChainChange,
    rt_chain::{ChainCommandSender, ChainControls, RtChain},
};

use super::{
    AudioError, AudioEvent, AudioTelemetry, DEFAULT_BUFFER_FRAMES, DeviceConfig,
    MAX_DEVICE_CHANNELS, NonFiniteReport,
    telemetry::{LANE_POSITIONS, block_slot},
};

/// Chain publications that can wait for the callback at one time.
const COMMAND_CAPACITY: usize = 8;

/// A meter holds its peak for this part of a second. The UI reads at 30 Hz, and a callback is
/// shorter than that, so that a meter with no hold shows only some of the callbacks.
const METER_WINDOWS_PER_SECOND: u32 = 30;

/// The load reads over this part of a second, so that the number in the UI stays readable.
const LOAD_WINDOWS_PER_SECOND: u32 = 4;

/// The frequency of the test tone.
pub const TEST_TONE_HERTZ: f64 = 440.0;

/// The peak of the test tone, which is -20 dBFS.
pub const TEST_TONE_PEAK: f32 = 0.1;

/// The length of the test tone, with its fades, in milliseconds.
pub const TEST_TONE_MILLISECONDS: u32 = 1_000;

/// The length of each fade of the test tone, in milliseconds. A fade prevents a click.
pub const TEST_TONE_FADE_MILLISECONDS: u32 = 10;

/// The length of the Hear Yourself fade, in milliseconds. A fade prevents a click.
pub const HEAR_YOURSELF_FADE_MILLISECONDS: u32 = 5;

/// The gain of the processed input on the output. It moves to its target in a short fade, so
/// that a change of Hear Yourself during a note does not click.
struct MonitorGain {
    gain: f32,
    /// The change of the gain in one frame.
    step: f32,
    /// The first callback starts at the target with no fade, so that a stream that starts with
    /// Hear Yourself off plays no input.
    started: bool,
}

impl MonitorGain {
    fn new(sample_rate: u32) -> Self {
        let frames = (sample_rate / 1_000 * HEAR_YOURSELF_FADE_MILLISECONDS).max(1);
        Self {
            gain: 1.0,
            step: 1.0 / frames as f32,
            started: false,
        }
    }

    /// Starts one callback. Returns the gain at its start and the target.
    fn begin(&mut self, enabled: bool) -> (f32, f32) {
        let target = if enabled { 1.0 } else { 0.0 };
        if !self.started {
            self.started = true;
            self.gain = target;
        }
        (self.gain, target)
    }

    /// The gain of one frame of a fade from `start`.
    fn at(&self, start: f32, target: f32, frame: usize) -> f32 {
        let moved = self.step * (frame + 1) as f32;
        if target > start {
            (start + moved).min(target)
        } else {
            (start - moved).max(target)
        }
    }

    /// Ends one callback of `frames` frames.
    fn end(&mut self, start: f32, target: f32, frames: usize) {
        if frames > 0 {
            self.gain = self.at(start, target, frames - 1);
        }
    }
}

/// One test tone. It adds to the output after the chain, so that the chain cannot hide it.
struct TestTone {
    sample_rate: u32,
    /// The next frame of the tone, or the length when no tone plays.
    position: u32,
    length: u32,
    fade: u32,
}

impl TestTone {
    fn new(sample_rate: u32) -> Self {
        let length = sample_rate / 1_000 * TEST_TONE_MILLISECONDS;
        Self {
            sample_rate,
            position: length,
            length,
            fade: (sample_rate / 1_000 * TEST_TONE_FADE_MILLISECONDS).max(1),
        }
    }

    const fn playing(&self) -> bool {
        self.position < self.length
    }

    const fn restart(&mut self) {
        self.position = 0;
    }

    fn next_sample(&mut self) -> f32 {
        let position = self.position;
        self.position += 1;
        let from_end = self.length - position - 1;
        let fade = (position.min(from_end) as f32 / self.fade as f32).min(1.0);
        let time = f64::from(position) / f64::from(self.sample_rate);
        let wave = (std::f64::consts::TAU * TEST_TONE_HERTZ * time).sin() as f32;
        wave * TEST_TONE_PEAK * fade
    }

    /// Adds the tone to the selected channels of one interleaved callback.
    fn mix_into(&mut self, output: &mut [f32], channels: usize, first: usize) {
        if channels == 0 {
            return;
        }
        for frame in output.chunks_exact_mut(channels) {
            if !self.playing() {
                return;
            }
            let sample = self.next_sample();
            frame[first] += sample;
            if channels > 1 {
                frame[first + 1] += sample;
            }
        }
    }
}

/// What the callback measures between two publications. It does not allocate after it starts.
struct CallbackMeters {
    sample_rate: u32,
    meter_frames_seen: u32,
    input_peaks: [f32; 2],
    output_peaks: [f32; 2],
    /// Held peak of each device channel.
    capture_peaks: [f32; MAX_DEVICE_CHANNELS],
    /// Held input and output level in dB for each block position.
    block_levels: Box<[[f32; 2]]>,
    load_frames_seen: u32,
    busy: Duration,
    load_published: bool,
}

impl CallbackMeters {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            meter_frames_seen: 0,
            input_peaks: [0.0; 2],
            output_peaks: [0.0; 2],
            capture_peaks: [0.0; MAX_DEVICE_CHANNELS],
            block_levels: vec![[f32::NEG_INFINITY; 2]; LANE_POSITIONS * 2].into_boxed_slice(),
            load_frames_seen: 0,
            busy: Duration::ZERO,
            load_published: false,
        }
    }
}

/// Callback-exclusive owner of the live chain and all host-conversion scratch buffers.
pub struct AudioWorker {
    chain: RtChain,
    commands: ChainCommandSender,
    scratch: Vec<f32>,
    max_frames: usize,
    callback_thread: Option<ThreadId>,
    telemetry: Arc<AudioTelemetry>,
    meters: CallbackMeters,
    tone: TestTone,
    monitor: MonitorGain,
}

impl AudioWorker {
    /// Prepares an empty chain and every allocation needed by the configured maximum callback.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidConfig`] for unsupported dimensions, or a backend error when
    /// the real-time chain cannot be prepared.
    pub fn new(config: &DeviceConfig) -> Result<Self, AudioError> {
        let spec = Self::stream_spec(config)?;
        let empty = || LaneSnapshot {
            blocks: Vec::new(),
            models: Vec::new(),
        };
        let prepared = PreparedChainChange::new(
            1,
            spec,
            empty(),
            empty(),
            false,
            LatencyReport {
                samples: 0,
                tail_seconds: 0.0,
            },
        )
        .map_err(|error| AudioError::Backend(error.to_string()))?;
        Self::with_chain(config, prepared, ChainControls::default())
    }

    /// Creates a worker that starts on a prepared chain with its controls already applied.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidConfig`] for unsupported dimensions or for a chain smaller
    /// than the configured maximum callback, or a backend error when the chain is rejected.
    pub fn with_chain(
        config: &DeviceConfig,
        prepared: PreparedChainChange,
        controls: ChainControls,
    ) -> Result<Self, AudioError> {
        let spec = Self::stream_spec(config)?;
        if prepared.spec.max_host_frames < spec.max_host_frames {
            return Err(AudioError::InvalidConfig(
                "the prepared chain is smaller than the stream callback".to_owned(),
            ));
        }
        let (mut chain, commands) = RtChain::prepare(prepared, COMMAND_CAPACITY)
            .map_err(|error| AudioError::Backend(error.to_string()))?;
        chain.set_controls(controls);
        let sample_rate = config.sample_rate.unwrap_or(48_000);
        let telemetry =
            AudioTelemetry::new(sample_rate, config.input_channels, config.output_channels)
                .shared();
        telemetry.publish_generation(chain.active_generation());
        let [first, second] = config.input_route;
        telemetry.set_input_route(
            first,
            if config.input_channels == 1 {
                first
            } else {
                second
            },
        );
        telemetry.set_output_first(config.output_first);
        Ok(Self {
            chain,
            commands,
            scratch: vec![0.0; spec.max_host_frames * 2],
            max_frames: spec.max_host_frames,
            callback_thread: None,
            telemetry,
            meters: CallbackMeters::new(sample_rate),
            tone: TestTone::new(sample_rate),
            monitor: MonitorGain::new(sample_rate),
        })
    }

    fn stream_spec(config: &DeviceConfig) -> Result<PrepareSpec, AudioError> {
        let sample_rate = config.sample_rate.unwrap_or(48_000);
        if sample_rate == 0
            || !matches!(config.input_channels, 1 | 2)
            || !matches!(config.output_channels, 1 | 2)
        {
            return Err(AudioError::InvalidConfig(
                "rate, buffer, and one or two I/O channels are required".to_owned(),
            ));
        }
        Ok(PrepareSpec {
            host_sample_rate: f64::from(sample_rate),
            max_host_frames: (config.buffer_frames.unwrap_or(DEFAULT_BUFFER_FRAMES) as usize)
                .max(1),
            // This spec checks the stream and prepares only an empty chain. The chain of the
            // session carries the oversampling that OPTIMIZE and the setting choose.
            oversampling: 1,
            input_mode: if config.input_channels == 1 {
                InputMode::Left
            } else {
                InputMode::Stereo
            },
            stereo_input: config.input_channels == 2,
            stereo_output: config.output_channels == 2,
        })
    }

    /// Returns the bounded control endpoint for replacing the prepared chain.
    #[must_use]
    pub fn command_sender(&self) -> ChainCommandSender {
        self.commands.clone()
    }

    /// Returns the lock-free capability, meter, and banner snapshot shared with the UI.
    #[must_use]
    pub fn telemetry(&self) -> Arc<AudioTelemetry> {
        Arc::clone(&self.telemetry)
    }

    /// Processes one interleaved host buffer without allocation.
    ///
    /// A buffer larger than the prepared callback is processed in prepared-size parts.
    /// `input_channels` is the layout of the device, which can hold more channels than the chain
    /// uses. The route in the telemetry selects the channels, and a change takes effect in the
    /// next call with no lock.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::WrongThread`] after callback-thread migration, or
    /// [`AudioError::InvalidCallback`] for a malformed buffer.
    pub fn process_interleaved(
        &mut self,
        input: &[f32],
        input_channels: usize,
        output: &mut [f32],
        output_channels: usize,
    ) -> Result<(), AudioError> {
        self.bind_callback_thread()?;
        if output_channels == 0 || !output.len().is_multiple_of(output_channels) {
            return Err(AudioError::InvalidCallback(
                "output must contain complete interleaved frames",
            ));
        }
        let frames = output.len() / output_channels;
        if input_channels > 0 && input.len() < frames * input_channels {
            return Err(AudioError::InvalidCallback(
                "input is shorter than the output callback",
            ));
        }
        if frames == 0 {
            return Ok(());
        }
        let started = Instant::now();
        let input = &input[..frames * input_channels];
        let (first, second) = routed_channels(self.telemetry.input_route(), input_channels);
        let input_peaks = channel_peaks(input, input_channels, first, second);
        let input_peak = input_peaks[0].max(input_peaks[1]);
        if self.telemetry.spend_tuner_lease(frames) {
            self.telemetry
                .publish_tuner_samples(input, input_channels, first, second);
        }
        let output_first = output_pair_start(self.telemetry.output_first(), output_channels);
        let mut start = 0;
        while start < frames {
            let count = (frames - start).min(self.max_frames);
            let end = start + count;
            self.deinterleave(
                &input[start * input_channels..end * input_channels],
                input_channels,
                count,
                first,
                second,
            );
            self.process_planar(count)?;
            self.interleave(
                &mut output[start * output_channels..end * output_channels],
                output_channels,
                count,
                output_first,
            );
            start = end;
        }
        // With Hear Yourself off, the chain still runs for its meters and spectrum, but the
        // output is silence. The test tone checks the output, so it still plays.
        let (start, target) = self.monitor.begin(self.telemetry.hear_yourself());
        if start != target {
            for (frame, samples) in output.chunks_exact_mut(output_channels).enumerate() {
                let gain = self.monitor.at(start, target, frame);
                samples.iter_mut().for_each(|sample| *sample *= gain);
            }
            self.monitor.end(start, target, frames);
        } else if target == 0.0 {
            output.fill(0.0);
        }
        if self.telemetry.take_tone_request() {
            self.tone.restart();
        }
        if self.tone.playing() {
            self.tone.mix_into(output, output_channels, output_first);
            self.telemetry.publish_tone_active(self.tone.playing());
        }
        self.telemetry.publish_meters(input_peak, peak(output));
        let output_peaks = channel_peaks(
            output,
            output_channels,
            output_first,
            output_first + usize::from(output_channels > 1),
        );
        self.publish_measurements(
            frames,
            input_peaks,
            output_peaks,
            input,
            input_channels,
            started,
        );
        Ok(())
    }

    /// Processes nullable planar host channels, tolerating missing and short arrays.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::WrongThread`] after callback-thread migration or a processing error.
    pub fn process_channels(
        &mut self,
        inputs: &[Option<&[f32]>],
        outputs: &mut [Option<&mut [f32]>],
    ) -> Result<(), AudioError> {
        self.bind_callback_thread()?;
        if !outputs.iter().any(Option::is_some) {
            return Ok(());
        }
        let short_input = inputs
            .iter()
            .flatten()
            .any(|input| input.len() < self.max_frames);
        let short_output = outputs
            .iter()
            .flatten()
            .any(|output| output.len() < self.max_frames);
        if short_input || short_output {
            for output in outputs.iter_mut().flatten() {
                output.fill(0.0);
            }
            return Ok(());
        }
        let frames = self.max_frames;
        let started = Instant::now();
        let input_peaks = planar_peaks(
            inputs.first().copied().flatten(),
            inputs.get(1).copied().flatten(),
        );
        let input_peak = inputs
            .iter()
            .flatten()
            .flat_map(|samples| samples.iter())
            .fold(0.0_f32, accumulate_peak);

        let left = inputs.first().and_then(Option::as_ref);
        let right = inputs.get(1).and_then(Option::as_ref).or(left);
        let tuner_reads = self.telemetry.spend_tuner_lease(frames);
        for frame in 0..frames {
            self.scratch[frame] = left.map_or(0.0, |samples| samples[frame]);
            self.scratch[frames + frame] = right.map_or(0.0, |samples| samples[frame]);
            if tuner_reads {
                self.telemetry.publish_tuner_samples(
                    &[self.scratch[frame], self.scratch[frames + frame]],
                    2,
                    0,
                    1,
                );
            }
        }
        self.process_planar(frames)?;
        let (start, target) = self.monitor.begin(self.telemetry.hear_yourself());
        let output_count = outputs.len();
        for (index, output) in outputs.iter_mut().enumerate() {
            let Some(samples) = output.as_deref_mut() else {
                continue;
            };
            for (frame, sample) in samples.iter_mut().enumerate().take(frames) {
                let gain = if start == target {
                    target
                } else {
                    self.monitor.at(start, target, frame)
                };
                *sample = match (output_count, index) {
                    _ if gain == 0.0 => 0.0,
                    (1, _) => 0.5 * (self.scratch[frame] + self.scratch[frames + frame]),
                    (_, 0) => self.scratch[frame],
                    _ => self.scratch[frames + frame],
                } * gain;
            }
        }
        self.monitor.end(start, target, frames);
        let output_peak = outputs
            .iter()
            .flatten()
            .flat_map(|samples| samples.iter())
            .fold(0.0_f32, accumulate_peak);
        self.telemetry.publish_meters(input_peak, output_peak);
        let output_peaks = planar_peaks(
            outputs.first().and_then(|output| output.as_deref()),
            outputs.get(1).and_then(|output| output.as_deref()),
        );
        self.publish_measurements(frames, input_peaks, output_peaks, &[], 0, started);
        Ok(())
    }

    /// Publishes the levels of one callback and its load, with no allocation and no lock.
    fn publish_measurements(
        &mut self,
        frames: usize,
        input_peaks: [f32; 2],
        output_peaks: [f32; 2],
        capture: &[f32],
        capture_channels: usize,
        started: Instant,
    ) {
        let frames = u32::try_from(frames).unwrap_or(u32::MAX);
        let meters = &mut self.meters;
        let new_window = meters.meter_frames_seen >= meters.sample_rate / METER_WINDOWS_PER_SECOND;
        if new_window {
            meters.meter_frames_seen = 0;
            meters.input_peaks = [0.0; 2];
            meters.output_peaks = [0.0; 2];
            meters.capture_peaks = [0.0; MAX_DEVICE_CHANNELS];
        }
        meters.meter_frames_seen = meters.meter_frames_seen.saturating_add(frames);
        for (held, peak) in meters.input_peaks.iter_mut().zip(input_peaks) {
            *held = held.max(peak);
        }
        for (held, peak) in meters.output_peaks.iter_mut().zip(output_peaks) {
            *held = held.max(peak);
        }
        self.telemetry
            .publish_levels(meters.input_peaks, meters.output_peaks);
        if capture_channels > 0 {
            let held = capture_channels.min(MAX_DEVICE_CHANNELS);
            for frame in capture.chunks_exact(capture_channels) {
                for (peak, sample) in meters.capture_peaks[..held].iter_mut().zip(frame) {
                    *peak = accumulate_peak(*peak, sample);
                }
            }
            self.telemetry
                .publish_capture_levels(&meters.capture_peaks[..held]);
        }

        let telemetry = &self.telemetry;
        let block_levels = &mut meters.block_levels;
        self.chain
            .visit_block_meters(|side, position, input, output| {
                let Some(held) = block_slot(side, position).map(|slot| &mut block_levels[slot])
                else {
                    return;
                };
                if new_window {
                    *held = [f32::NEG_INFINITY; 2];
                }
                held[0] = held[0].max(input);
                held[1] = held[1].max(output);
                telemetry.publish_block_level(side, position, held[0], held[1]);
            });

        meters.busy += started.elapsed();
        meters.load_frames_seen = meters.load_frames_seen.saturating_add(frames);
        if !meters.load_published
            || meters.load_frames_seen >= meters.sample_rate / LOAD_WINDOWS_PER_SECOND
        {
            let covered = f64::from(meters.load_frames_seen) / f64::from(meters.sample_rate);
            self.telemetry
                .publish_callback_load((meters.busy.as_secs_f64() / covered) as f32);
            // Arithmetic on the running chain, with no allocation, for the delay estimate.
            self.telemetry
                .publish_chain_latency(self.chain.host_report().latency_samples);
            meters.load_published = true;
            meters.load_frames_seen = 0;
            meters.busy = Duration::ZERO;
        }
    }

    pub(crate) const fn max_frames(&self) -> usize {
        self.max_frames
    }

    fn bind_callback_thread(&mut self) -> Result<(), AudioError> {
        let current = std::thread::current().id();
        match self.callback_thread {
            Some(expected) if expected != current => Err(AudioError::WrongThread),
            Some(_) => Ok(()),
            None => {
                self.callback_thread = Some(current);
                Ok(())
            }
        }
    }

    fn deinterleave(
        &mut self,
        input: &[f32],
        input_channels: usize,
        frames: usize,
        first: usize,
        second: usize,
    ) {
        for frame in 0..frames {
            let (left, right) = match input_channels {
                0 => (0.0, 0.0),
                _ => (
                    input[frame * input_channels + first],
                    input[frame * input_channels + second],
                ),
            };
            self.scratch[frame] = left;
            self.scratch[frames + frame] = right;
        }
    }

    fn process_planar(&mut self, frames: usize) -> Result<(), AudioError> {
        let mut buffer = PlanarBuffer::new(&mut self.scratch[..2 * frames], 2, frames)
            .map_err(AudioError::Chain)?;
        let result = self.chain.process(&mut buffer);
        // The generation goes first, so that a reader never sees a report of a newer chain.
        let generation = self.chain.active_generation();
        self.telemetry.publish_generation(generation);
        let failed = self.chain.take_failed_blocks();
        if !failed.is_empty() {
            self.telemetry.publish_failed_blocks(generation, failed);
        }
        if result.is_err() || self.chain.take_processing_failed() {
            self.telemetry.publish_event(AudioEvent::ProcessingFailed);
        }
        if self.chain.take_output_sanitized() {
            self.telemetry.publish_non_finite(NonFiniteReport {
                generation,
                block: self.chain.take_non_finite_block(),
            });
        }
        result.map_err(AudioError::Chain)
    }

    fn interleave(
        &mut self,
        output: &mut [f32],
        output_channels: usize,
        frames: usize,
        first: usize,
    ) {
        for frame in 0..frames {
            let left = self.scratch[frame];
            let right = self.scratch[frames + frame];
            if output_channels == 1 {
                output[frame] = 0.5 * (left + right);
            } else {
                let slots = &mut output[frame * output_channels..(frame + 1) * output_channels];
                slots.fill(0.0);
                slots[first] = left;
                slots[first + 1] = right;
            }
        }
    }
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, accumulate_peak)
}

/// The peak of two channels of interleaved samples. A mono route fills both.
fn channel_peaks(samples: &[f32], channels: usize, first: usize, second: usize) -> [f32; 2] {
    let mut peaks = [0.0_f32; 2];
    if channels == 0 {
        return peaks;
    }
    for frame in samples.chunks_exact(channels) {
        peaks[0] = accumulate_peak(peaks[0], &frame[first]);
        peaks[1] = accumulate_peak(peaks[1], &frame[second]);
    }
    peaks
}

/// The route for this layout. A channel past the end of the layout reads the first channel, so
/// that a late route for another device cannot read out of bounds.
fn routed_channels((first, second): (usize, usize), channels: usize) -> (usize, usize) {
    let inside = |channel: usize| if channel < channels { channel } else { 0 };
    (inside(first), inside(second))
}

/// The first channel of the output pair for this layout. A pair past the end starts at zero.
const fn output_pair_start(first: usize, channels: usize) -> usize {
    if first + 1 < channels { first } else { 0 }
}

/// The peak of each planar channel. A missing right channel takes the peak of the left.
fn planar_peaks(left: Option<&[f32]>, right: Option<&[f32]>) -> [f32; 2] {
    let left_peak = left.map_or(0.0, peak);
    [left_peak, right.map_or(left_peak, peak)]
}

/// `f32::max` ignores NaN, so a non-finite sample must read as infinity to stay visible.
fn accumulate_peak(peak: f32, sample: &f32) -> f32 {
    if sample.is_finite() {
        peak.max(sample.abs())
    } else {
        f32::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioBackendKind;

    const FRAMES: usize = 128;

    fn config() -> DeviceConfig {
        DeviceConfig {
            backend: AudioBackendKind::Dummy,
            sample_rate: Some(48_000),
            buffer_frames: Some(FRAMES as u32),
            input_channels: 1,
            output_channels: 2,
            ..DeviceConfig::default()
        }
    }

    /// Runs one planar callback, and returns the peak of the two outputs.
    fn planar_peak(worker: &mut AudioWorker) -> f32 {
        let input = [0.25_f32; FRAMES];
        let inputs = [Some(input.as_slice()), None];
        let mut left = [1.0_f32; FRAMES];
        let mut right = [1.0_f32; FRAMES];
        let mut outputs = [Some(left.as_mut_slice()), Some(right.as_mut_slice())];
        worker
            .process_channels(&inputs, &mut outputs)
            .expect("callback");
        left.iter().chain(&right).fold(0.0_f32, accumulate_peak)
    }

    /// The frames of one Hear Yourself fade at the rate of [`config`].
    const FADE_FRAMES: usize = 48 * HEAR_YOURSELF_FADE_MILLISECONDS as usize;

    /// The largest change from one sample to the next one.
    fn largest_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    /// A slow sine at the peak of the input of a loud guitar, which moves less than 0.004 in
    /// one frame. A step larger than that comes from the switch.
    fn note(frame: usize) -> f32 {
        let time = frame as f32 / 48_000.0;
        0.5 * (std::f32::consts::TAU * 50.0 * time).sin()
    }

    /// Runs interleaved callbacks of one note, and returns the left output of each frame.
    fn interleaved_note(worker: &mut AudioWorker, first: usize, callbacks: usize) -> Vec<f32> {
        let mut left = Vec::new();
        for callback in 0..callbacks {
            let start = first + callback * FRAMES;
            let input: Vec<f32> = (start..start + FRAMES).map(note).collect();
            let mut output = [0.0_f32; FRAMES * 2];
            worker
                .process_interleaved(&input, 1, &mut output, 2)
                .expect("callback");
            left.extend(output.iter().step_by(2));
        }
        left
    }

    /// Runs planar callbacks of one note, and returns the left output of each frame.
    fn planar_note(worker: &mut AudioWorker, first: usize, callbacks: usize) -> Vec<f32> {
        let mut left = Vec::new();
        for callback in 0..callbacks {
            let start = first + callback * FRAMES;
            let input: Vec<f32> = (start..start + FRAMES).map(note).collect();
            let inputs = [Some(input.as_slice()), None];
            let mut left_out = [0.0_f32; FRAMES];
            let mut right_out = [0.0_f32; FRAMES];
            let mut outputs = [
                Some(left_out.as_mut_slice()),
                Some(right_out.as_mut_slice()),
            ];
            worker
                .process_channels(&inputs, &mut outputs)
                .expect("callback");
            left.extend(left_out);
        }
        left
    }

    /// Turns Hear Yourself off and on during a note, and checks that each change is a fade.
    fn check_the_fade(play: fn(&mut AudioWorker, usize, usize) -> Vec<f32>) {
        let mut worker = AudioWorker::new(&config()).expect("worker");
        let callbacks = FADE_FRAMES.div_ceil(FRAMES) + 2;
        let before = play(&mut worker, 0, 4);
        assert!(
            before.iter().any(|sample| sample.abs() > 0.1),
            "the note plays"
        );
        let normal = largest_step(&before);

        worker.telemetry().set_hear_yourself(false);
        let mut off = vec![*before.last().expect("a sample")];
        off.extend(play(&mut worker, 4 * FRAMES, callbacks));
        assert!(
            largest_step(&off) <= normal + 0.001,
            "the output steps by {} after the switch, and by {normal} in the note",
            largest_step(&off)
        );
        assert!(
            off[1..=FADE_FRAMES / 2].iter().any(|sample| *sample != 0.0),
            "the output stops with no fade"
        );
        assert!(
            off[1 + FADE_FRAMES..].iter().all(|sample| *sample == 0.0),
            "the output is not silent after the fade"
        );

        worker.telemetry().set_hear_yourself(true);
        let mut on = vec![0.0];
        on.extend(play(&mut worker, (4 + callbacks) * FRAMES, callbacks));
        assert!(
            largest_step(&on) <= normal + 0.001,
            "the output steps by {} after the switch, and by {normal} in the note",
            largest_step(&on)
        );
        assert!(
            on[FADE_FRAMES + 1..]
                .iter()
                .any(|sample| sample.abs() > 0.1),
            "the note does not come back"
        );
    }

    #[test]
    fn hear_yourself_fades_the_interleaved_output() {
        check_the_fade(interleaved_note);
    }

    #[test]
    fn hear_yourself_fades_the_planar_output() {
        check_the_fade(planar_note);
    }

    #[test]
    fn a_stream_that_starts_with_hear_yourself_off_plays_no_input() {
        let mut worker = AudioWorker::new(&config()).expect("worker");
        worker.telemetry().set_hear_yourself(false);
        let output = interleaved_note(&mut worker, 0, 2);
        assert!(
            output.iter().all(|sample| *sample == 0.0),
            "the input plays"
        );
    }

    #[test]
    fn hear_yourself_off_silences_the_planar_output() {
        let mut worker = AudioWorker::new(&config()).expect("worker");
        assert!(planar_peak(&mut worker) > 0.1, "the input plays");

        worker.telemetry().set_hear_yourself(false);
        for _ in 0..FADE_FRAMES.div_ceil(FRAMES) {
            planar_peak(&mut worker);
        }
        assert_eq!(planar_peak(&mut worker), 0.0, "the output plays");
        let meters = worker.telemetry().input_levels_db();
        assert!(meters[0] > -20.0, "the input still reads: {meters:?}");
    }
}
