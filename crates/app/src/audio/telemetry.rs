use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

use ronie_core::ChainSide;
use ronie_dsp::rt_chain::FailedBlocks;

use super::MAX_DEVICE_CHANNELS;

/// Audio events suitable for status banners and recovery decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEvent {
    /// The operating system reported a disconnected device.
    DeviceDisconnected,
    /// The stream failed for another reason and cannot continue to produce audio.
    StreamFailed,
    /// The processing chain rejected or disabled a block.
    ProcessingFailed,
    /// The chain output contained a non-finite sample that was replaced with silence.
    NonFiniteOutput,
    /// Playback did not receive enough capture samples.
    OutputUnderrun,
    /// Capture exceeded the bounded input queue.
    InputOverrun,
}

impl AudioEvent {
    const WORST_FIRST: [Self; 6] = [
        Self::DeviceDisconnected,
        Self::StreamFailed,
        Self::ProcessingFailed,
        Self::NonFiniteOutput,
        Self::OutputUnderrun,
        Self::InputOverrun,
    ];

    const fn bit(self) -> usize {
        match self {
            Self::DeviceDisconnected => 1,
            Self::StreamFailed => 1 << 1,
            Self::ProcessingFailed => 1 << 2,
            Self::NonFiniteOutput => 1 << 3,
            Self::OutputUnderrun => 1 << 4,
            Self::InputOverrun => 1 << 5,
        }
    }
}

/// Events that end a stream. They stay set until the stream closes.
const FATAL_EVENTS: usize = AudioEvent::DeviceDisconnected.bit() | AudioEvent::StreamFailed.bit();

/// The value of the block slot when no block is on record.
const NO_BLOCK: usize = 0;

/// The positions in one lane that have a level. A lane holds at most 64 blocks.
pub(crate) const LANE_POSITIONS: usize = 64;

/// The level that a meter shows for silence, in dB.
const FLOOR_DB: f32 = -60.0;

/// Samples in the ring of raw input for the tuner. This is more than the longest window of the
/// detector, at the highest rate that it decimates.
const TUNER_TAP_SAMPLES: usize = 1 << 15;

/// A tuner reader keeps the tap open for this part of a second after each read. The tap needs no
/// message when the page closes, and a hidden window costs the callback no copy.
const TUNER_LEASES_PER_SECOND: u32 = 2;

/// What the callback knows about a non-finite output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonFiniteReport {
    /// Generation of the chain that made the samples.
    pub generation: u64,
    /// Side and position of the block that made them, when one block did.
    pub block: Option<(ChainSide, usize)>,
}

/// The blocks of one chain that failed in a callback. They run as bypass until that chain ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailedBlocksReport {
    /// Generation of the chain that holds the blocks.
    pub generation: u64,
    /// Position of each failed block.
    pub blocks: FailedBlocks,
}

/// A set of audio events taken from the telemetry.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AudioEvents(usize);

impl AudioEvents {
    /// Whether this set contains one event.
    #[must_use]
    pub const fn contains(self, event: AudioEvent) -> bool {
        self.0 & event.bit() != 0
    }

    /// The events of this set and of another set.
    #[must_use]
    pub(crate) const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Lock-free capability, meter, and banner state published by the callback, and the few requests
/// that the control thread gives to the callback.
pub struct AudioTelemetry {
    sample_rate: AtomicU32,
    input_channels: AtomicUsize,
    output_channels: AtomicUsize,
    input_peak: AtomicU32,
    output_peak: AtomicU32,
    events: AtomicUsize,
    non_finite_block: AtomicUsize,
    non_finite_generation: AtomicU64,
    failed_left: AtomicU64,
    failed_right: AtomicU64,
    failed_generation: AtomicU64,
    active_generation: AtomicU64,
    /// Linear peak of each input channel in the current meter window.
    input_levels: [AtomicU32; 2],
    /// Linear peak of each output channel in the current meter window.
    output_levels: [AtomicU32; 2],
    /// Callback time over the audio time that it covers.
    callback_load: AtomicU32,
    /// Input and output level in dB for each position of each lane.
    block_levels: Box<[[AtomicU32; 2]]>,
    /// Raw input for the tuner. Only the callback writes, and only with a lease.
    tuner_tap: Box<[AtomicU32]>,
    /// Total samples that the callback wrote to the tap.
    tuner_written: AtomicUsize,
    /// Frames that the callback can still copy.
    tuner_lease: AtomicU32,
    /// The two device channels that feed the chain, as one value, so that a pair changes at once.
    input_route: AtomicU32,
    /// First device channel of the output pair.
    output_first: AtomicUsize,
    /// Whether the output plays the processed input. When false, the callback writes silence.
    hear_yourself: AtomicBool,
    /// A request from the control thread for one test tone.
    tone_request: AtomicBool,
    /// Whether the callback mixes a test tone now.
    tone_active: AtomicBool,
    /// The channels that the device delivers in each frame.
    capture_channels: AtomicUsize,
    /// Linear peak of each device channel in the current meter window.
    capture_levels: Box<[AtomicU32]>,
    /// Whether the host reported its latest error for the capture stream.
    capture_error: AtomicBool,
    /// Whether OPTIMIZE is on. The callback reads it to size the input queue.
    optimize: AtomicBool,
    /// The frames that stayed in the input queue for the latest whole window.
    queue_frames: AtomicUsize,
    /// The frames of one callback as the host really gives them.
    callback_frames: AtomicUsize,
    /// The delay of the running chain in samples.
    chain_latency: AtomicU64,
    /// Callbacks that found too few captured frames in the input queue.
    input_underruns: AtomicU32,
}

impl AudioTelemetry {
    pub(crate) fn new(sample_rate: u32, input_channels: usize, output_channels: usize) -> Self {
        Self {
            sample_rate: AtomicU32::new(sample_rate),
            input_channels: AtomicUsize::new(input_channels),
            output_channels: AtomicUsize::new(output_channels),
            input_peak: AtomicU32::new(0),
            output_peak: AtomicU32::new(0),
            events: AtomicUsize::new(0),
            non_finite_block: AtomicUsize::new(NO_BLOCK),
            non_finite_generation: AtomicU64::new(0),
            failed_left: AtomicU64::new(0),
            failed_right: AtomicU64::new(0),
            failed_generation: AtomicU64::new(0),
            active_generation: AtomicU64::new(0),
            input_levels: [AtomicU32::new(0), AtomicU32::new(0)],
            output_levels: [AtomicU32::new(0), AtomicU32::new(0)],
            callback_load: AtomicU32::new(0.0_f32.to_bits()),
            block_levels: (0..LANE_POSITIONS * 2)
                .map(|_| {
                    [
                        AtomicU32::new(FLOOR_DB.to_bits()),
                        AtomicU32::new(FLOOR_DB.to_bits()),
                    ]
                })
                .collect(),
            tuner_tap: (0..TUNER_TAP_SAMPLES).map(|_| AtomicU32::new(0)).collect(),
            tuner_written: AtomicUsize::new(0),
            tuner_lease: AtomicU32::new(0),
            input_route: AtomicU32::new(pack_route(0, 1)),
            output_first: AtomicUsize::new(0),
            hear_yourself: AtomicBool::new(true),
            tone_request: AtomicBool::new(false),
            tone_active: AtomicBool::new(false),
            capture_channels: AtomicUsize::new(0),
            capture_levels: (0..MAX_DEVICE_CHANNELS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            capture_error: AtomicBool::new(false),
            optimize: AtomicBool::new(true),
            queue_frames: AtomicUsize::new(0),
            callback_frames: AtomicUsize::new(0),
            chain_latency: AtomicU64::new(0),
            input_underruns: AtomicU32::new(0),
        }
    }

    /// Turns OPTIMIZE on or off for the callback. The next callback reads it with no lock.
    pub fn set_optimize(&self, enabled: bool) {
        self.optimize.store(enabled, Ordering::Release);
    }

    /// Whether OPTIMIZE is on for the callback.
    #[must_use]
    pub fn optimize(&self) -> bool {
        self.optimize.load(Ordering::Acquire)
    }

    /// Publishes the frames that stay in the input queue and the real callback size.
    pub(crate) fn publish_queue(&self, queue_frames: usize, callback_frames: usize) {
        self.queue_frames.store(queue_frames, Ordering::Release);
        self.callback_frames
            .store(callback_frames, Ordering::Release);
    }

    /// The frames that stayed in the input queue for the latest whole window.
    #[must_use]
    pub fn queue_frames(&self) -> usize {
        self.queue_frames.load(Ordering::Acquire)
    }

    /// The frames of one callback as the host gives them, or zero before a queue reports it.
    #[must_use]
    pub fn callback_frames(&self) -> usize {
        self.callback_frames.load(Ordering::Acquire)
    }

    /// Publishes the delay of the running chain in samples.
    pub(crate) fn publish_chain_latency(&self, samples: u64) {
        self.chain_latency.store(samples, Ordering::Release);
    }

    /// The delay of the running chain in samples.
    #[must_use]
    pub fn chain_latency(&self) -> u64 {
        self.chain_latency.load(Ordering::Acquire)
    }

    /// Counts one callback that found too few captured frames.
    pub(crate) fn note_input_underrun(&self) {
        self.input_underruns.fetch_add(1, Ordering::AcqRel);
    }

    /// All callbacks that found too few captured frames. The count wraps.
    #[must_use]
    pub fn input_underruns(&self) -> u32 {
        self.input_underruns.load(Ordering::Acquire)
    }

    pub(crate) fn note_capture_error(&self) {
        self.capture_error.store(true, Ordering::Release);
    }

    /// Whether the capture device gave the error that ended the stream.
    #[must_use]
    pub fn capture_failed(&self) -> bool {
        self.capture_error.load(Ordering::Acquire)
    }

    /// Selects the device channels that feed the chain. The next callback reads the new pair.
    pub fn set_input_route(&self, first: usize, second: usize) {
        self.input_route
            .store(pack_route(first, second), Ordering::Release);
    }

    /// The device channels that feed the chain.
    #[must_use]
    pub fn input_route(&self) -> (usize, usize) {
        let packed = self.input_route.load(Ordering::Acquire);
        ((packed >> 16) as usize, (packed & 0xFFFF) as usize)
    }

    /// Selects the first device channel of the output pair.
    pub fn set_output_first(&self, first: usize) {
        self.output_first.store(first, Ordering::Release);
    }

    /// First device channel of the output pair.
    #[must_use]
    pub fn output_first(&self) -> usize {
        self.output_first.load(Ordering::Acquire)
    }

    /// Selects whether the output plays the processed input. The callback reads the choice in its
    /// next call, with no lock. The input meters and the tuner read the input in both cases.
    pub fn set_hear_yourself(&self, enabled: bool) {
        self.hear_yourself.store(enabled, Ordering::Release);
    }

    /// Whether the output plays the processed input.
    #[must_use]
    pub fn hear_yourself(&self) -> bool {
        self.hear_yourself.load(Ordering::Acquire)
    }

    /// Asks the callback to play one test tone from its start.
    pub fn request_test_tone(&self) {
        self.tone_request.store(true, Ordering::Release);
    }

    /// Whether the callback mixes a test tone now.
    #[must_use]
    pub fn test_tone_active(&self) -> bool {
        self.tone_active.load(Ordering::Acquire)
    }

    pub(crate) fn take_tone_request(&self) -> bool {
        // A load first, because each callback asks and a request is rare.
        self.tone_request.load(Ordering::Relaxed) && self.tone_request.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn publish_tone_active(&self, active: bool) {
        self.tone_active.store(active, Ordering::Release);
    }

    /// The level of each channel of the device in dB, over the latest meter window.
    #[must_use]
    pub fn capture_levels_db(&self) -> Vec<f32> {
        let channels = self
            .capture_channels
            .load(Ordering::Acquire)
            .min(MAX_DEVICE_CHANNELS);
        self.capture_levels[..channels]
            .iter()
            .map(|slot| level_db(f32::from_bits(slot.load(Ordering::Acquire))))
            .collect()
    }

    /// Publishes the linear peak of each device channel in the current meter window.
    pub(crate) fn publish_capture_levels(&self, peaks: &[f32]) {
        for (slot, peak) in self.capture_levels.iter().zip(peaks) {
            slot.store(peak.to_bits(), Ordering::Release);
        }
        self.capture_channels
            .store(peaks.len().min(MAX_DEVICE_CHANNELS), Ordering::Release);
    }

    /// Current stream sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Acquire)
    }

    /// Current capture channel count.
    #[must_use]
    pub fn input_channels(&self) -> usize {
        self.input_channels.load(Ordering::Acquire)
    }

    /// Current playback channel count.
    #[must_use]
    pub fn output_channels(&self) -> usize {
        self.output_channels.load(Ordering::Acquire)
    }

    /// Latest absolute input peak. A non-finite sample reads as infinity.
    #[must_use]
    pub fn input_peak(&self) -> f32 {
        f32::from_bits(self.input_peak.load(Ordering::Acquire))
    }

    /// Latest absolute output peak. A non-finite sample reads as infinity.
    #[must_use]
    pub fn output_peak(&self) -> f32 {
        f32::from_bits(self.output_peak.load(Ordering::Acquire))
    }

    /// Level of each input channel in dB, over the latest meter window.
    #[must_use]
    pub fn input_levels_db(&self) -> [f32; 2] {
        read_levels_db(&self.input_levels)
    }

    /// Level of each output channel in dB, over the latest meter window.
    #[must_use]
    pub fn output_levels_db(&self) -> [f32; 2] {
        read_levels_db(&self.output_levels)
    }

    /// Latest callback time over the audio time that it covers. One means no time remains.
    #[must_use]
    pub fn callback_load(&self) -> f32 {
        f32::from_bits(self.callback_load.load(Ordering::Acquire))
    }

    /// Input and output level in dB of one block position, or `None` for a position past the end.
    #[must_use]
    pub fn block_level_db(&self, side: ChainSide, position: usize) -> Option<(f32, f32)> {
        let [input, output] = self.block_levels.get(block_slot(side, position)?)?;
        Some((
            f32::from_bits(input.load(Ordering::Acquire)),
            f32::from_bits(output.load(Ordering::Acquire)),
        ))
    }

    /// Asks the callback to copy raw input for a short time. Each read of the tuner renews this.
    pub fn open_tuner_tap(&self) {
        self.tuner_lease.store(
            self.sample_rate() / TUNER_LEASES_PER_SECOND,
            Ordering::Release,
        );
    }

    /// Whether the callback still copies raw input.
    #[must_use]
    pub fn tuner_tap_open(&self) -> bool {
        self.tuner_lease.load(Ordering::Acquire) > 0
    }

    /// Total raw input samples that the callback has written. This count does not wrap the ring.
    #[must_use]
    pub fn tuner_samples_written(&self) -> usize {
        self.tuner_written.load(Ordering::Acquire)
    }

    /// Replaces `samples` with the raw input after position `from`, and returns the next position.
    ///
    /// A reader that is too far behind gets the newest half of the ring. The callback can write
    /// over the older half during this read.
    pub fn read_tuner_samples(&self, from: usize, samples: &mut Vec<f32>) -> usize {
        samples.clear();
        let written = self.tuner_samples_written();
        let start = from.max(written.saturating_sub(TUNER_TAP_SAMPLES / 2));
        samples.extend((start..written).map(|position| {
            let slot = &self.tuner_tap[position & (TUNER_TAP_SAMPLES - 1)];
            f32::from_bits(slot.load(Ordering::Relaxed))
        }));
        written
    }

    /// Generation of the chain that the latest callback used.
    #[must_use]
    pub fn active_generation(&self) -> u64 {
        self.active_generation.load(Ordering::Acquire)
    }

    /// Worst event not yet taken, or `None` during healthy processing.
    #[must_use]
    pub fn last_event(&self) -> Option<AudioEvent> {
        let events = AudioEvents(self.events.load(Ordering::Acquire));
        AudioEvent::WORST_FIRST
            .into_iter()
            .find(|event| events.contains(*event))
    }

    /// Returns the events since the last call and clears all except the fatal events.
    pub fn take_events(&self) -> AudioEvents {
        AudioEvents(self.events.fetch_and(FATAL_EVENTS, Ordering::AcqRel))
    }

    /// Whether the stream reported a failure that ends its audio.
    #[must_use]
    pub fn stream_failed(&self) -> bool {
        self.events.load(Ordering::Acquire) & FATAL_EVENTS != 0
    }

    pub(crate) fn publish_meters(&self, input: f32, output: f32) {
        self.input_peak.store(input.to_bits(), Ordering::Release);
        self.output_peak.store(output.to_bits(), Ordering::Release);
    }

    /// Publishes the linear peak of each channel in the current meter window.
    pub(crate) fn publish_levels(&self, input: [f32; 2], output: [f32; 2]) {
        for (slot, peak) in self.input_levels.iter().zip(input) {
            slot.store(peak.to_bits(), Ordering::Release);
        }
        for (slot, peak) in self.output_levels.iter().zip(output) {
            slot.store(peak.to_bits(), Ordering::Release);
        }
    }

    pub(crate) fn publish_callback_load(&self, load: f32) {
        self.callback_load.store(load.to_bits(), Ordering::Release);
    }

    pub(crate) fn publish_block_level(
        &self,
        side: ChainSide,
        position: usize,
        input_db: f32,
        output_db: f32,
    ) {
        if let Some([input, output]) =
            block_slot(side, position).and_then(|slot| self.block_levels.get(slot))
        {
            input.store(input_db.to_bits(), Ordering::Release);
            output.store(output_db.to_bits(), Ordering::Release);
        }
    }

    /// Spends the lease for one callback, and returns whether the callback must copy its input.
    ///
    /// A read that renews the lease at this moment can lose. The next read renews it again.
    pub(crate) fn spend_tuner_lease(&self, frames: usize) -> bool {
        let lease = self.tuner_lease.load(Ordering::Acquire);
        if lease == 0 {
            return false;
        }
        let frames = u32::try_from(frames).unwrap_or(u32::MAX);
        self.tuner_lease
            .store(lease.saturating_sub(frames), Ordering::Release);
        true
    }

    /// Copies a mono mix of the two routed channels of one callback into the tap.
    pub(crate) fn publish_tuner_samples(
        &self,
        input: &[f32],
        channels: usize,
        first: usize,
        second: usize,
    ) {
        if channels == 0 {
            return;
        }
        let mut position = self.tuner_written.load(Ordering::Relaxed);
        for frame in input.chunks_exact(channels) {
            let mixed = 0.5 * (frame[first] + frame[second]);
            let sample = if mixed.is_finite() { mixed } else { 0.0 };
            self.tuner_tap[position & (TUNER_TAP_SAMPLES - 1)]
                .store(sample.to_bits(), Ordering::Relaxed);
            position = position.wrapping_add(1);
        }
        self.tuner_written.store(position, Ordering::Release);
    }

    pub(crate) fn publish_event(&self, event: AudioEvent) {
        self.events.fetch_or(event.bit(), Ordering::AcqRel);
    }

    /// Publishes a non-finite output with no allocation and no lock. The newest report wins, and
    /// the event comes last, so that a reader of the event also sees the report.
    pub(crate) fn publish_non_finite(&self, report: NonFiniteReport) {
        let block = report.block.map_or(NO_BLOCK, |(side, position)| {
            let side_bit = match side {
                ChainSide::Left => 0,
                ChainSide::Right => 1,
            };
            position.saturating_add(1) << 1 | side_bit
        });
        self.non_finite_block.store(block, Ordering::Release);
        self.non_finite_generation
            .store(report.generation, Ordering::Release);
        self.publish_event(AudioEvent::NonFiniteOutput);
    }

    /// The latest non-finite report. Read this after [`Self::take_events`] gives the event.
    #[must_use]
    pub fn non_finite_report(&self) -> NonFiniteReport {
        let block = self.non_finite_block.load(Ordering::Acquire);
        let side = if block & 1 == 0 {
            ChainSide::Left
        } else {
            ChainSide::Right
        };
        NonFiniteReport {
            generation: self.non_finite_generation.load(Ordering::Acquire),
            block: (block != NO_BLOCK).then_some((side, (block >> 1).saturating_sub(1))),
        }
    }

    /// Adds failed blocks of one chain with no allocation and no lock.
    ///
    /// A failed block stays bypassed and does not fail again, so that a report must not lose it.
    /// Only the callback writes these values. The bits of an older chain go before the new
    /// generation shows, and the event comes last, so that a reader of the event sees the report.
    pub(crate) fn publish_failed_blocks(&self, generation: u64, blocks: FailedBlocks) {
        if self.failed_generation.load(Ordering::Acquire) != generation {
            self.failed_left.store(0, Ordering::Release);
            self.failed_right.store(0, Ordering::Release);
            self.failed_generation.store(generation, Ordering::Release);
        }
        self.failed_left.fetch_or(blocks.left, Ordering::AcqRel);
        self.failed_right.fetch_or(blocks.right, Ordering::AcqRel);
        self.publish_event(AudioEvent::ProcessingFailed);
    }

    /// The failed blocks of the newest chain that had one. Read [`Self::active_generation`] after
    /// this report, because a report of an older chain says nothing about the running chain.
    #[must_use]
    pub fn failed_blocks(&self) -> FailedBlocksReport {
        loop {
            let generation = self.failed_generation.load(Ordering::Acquire);
            let blocks = FailedBlocks {
                left: self.failed_left.load(Ordering::Acquire),
                right: self.failed_right.load(Ordering::Acquire),
            };
            // A callback can change the chain during these reads, which is rare and short.
            if self.failed_generation.load(Ordering::Acquire) == generation {
                return FailedBlocksReport { generation, blocks };
            }
        }
    }

    pub(crate) fn publish_generation(&self, generation: u64) {
        self.active_generation.store(generation, Ordering::Release);
    }

    pub(crate) fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

const fn pack_route(first: usize, second: usize) -> u32 {
    ((first as u32 & 0xFFFF) << 16) | (second as u32 & 0xFFFF)
}

/// The slot of one block position. Positions at or past the limit have no slot.
pub(crate) fn block_slot(side: ChainSide, position: usize) -> Option<usize> {
    let first = match side {
        ChainSide::Left => 0,
        ChainSide::Right => LANE_POSITIONS,
    };
    (position < LANE_POSITIONS).then_some(first + position)
}

fn read_levels_db(slots: &[AtomicU32; 2]) -> [f32; 2] {
    [
        level_db(f32::from_bits(slots[0].load(Ordering::Acquire))),
        level_db(f32::from_bits(slots[1].load(Ordering::Acquire))),
    ]
}

/// A meter level for a linear peak. A non-finite sample reads as full scale, because a meter has
/// no place for infinity.
fn level_db(peak: f32) -> f32 {
    if !peak.is_finite() {
        0.0
    } else if peak > 0.0 {
        (20.0 * peak.log10()).max(FLOOR_DB)
    } else {
        FLOOR_DB
    }
}
