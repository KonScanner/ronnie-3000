//! The tuner of the running stream: raw input from the callback, and analysis on the reader.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use ronie_dsp::analysis::{TunerDetector, TunerReading};

use super::AudioTelemetry;

/// A gap this long between reads means that the page closed, so that what the detector holds is
/// old. The UI reads at 30 Hz, and at 4 Hz while the window hides, and this gap is longer.
const CLOSED_AFTER: Duration = Duration::from_millis(400);

/// With no new sample for this long, the stream has stopped, and the window must not read as a note.
const STOPPED_AFTER: Duration = Duration::from_millis(250);

/// The detector skips an analysis under this age. The first read must not count as too young.
const CLOCK_START_MS: i64 = 1_000;

/// What a read reports when nothing can be measured.
const SILENT_READING: TunerReading = TunerReading {
    frequency_hz: 0.0,
    confidence: 0.0,
    level_db: -60.0,
};

/// Reader of the raw input of one stream at a time. Each read keeps the tap of the callback open.
///
/// The analysis runs on the thread that reads, never on the callback.
pub struct LiveTuner {
    clock: Instant,
    state: Option<TunerState>,
    scratch: Vec<f32>,
}

/// What belongs to one uninterrupted time on the tuner page.
struct TunerState {
    stream: Arc<AudioTelemetry>,
    detector: TunerDetector,
    read_position: usize,
    last_read: Instant,
    last_sample: Instant,
}

impl LiveTuner {
    /// Creates a reader with no stream.
    #[must_use]
    pub fn new() -> Self {
        Self {
            clock: Instant::now(),
            state: None,
            scratch: Vec::new(),
        }
    }

    /// Reads the newest raw input of `stream`, and keeps its tap open.
    pub fn read(&mut self, stream: &Arc<AudioTelemetry>) -> TunerReading {
        stream.open_tuner_tap();
        let now = Instant::now();
        let current = self.state.take().filter(|state| {
            Arc::ptr_eq(&state.stream, stream) && now.duration_since(state.last_read) < CLOSED_AFTER
        });
        let Some(mut state) = current.or_else(|| TunerState::start(stream, now)) else {
            return SILENT_READING;
        };
        state.last_read = now;
        state.read_position = stream.read_tuner_samples(state.read_position, &mut self.scratch);
        if !self.scratch.is_empty() {
            state.detector.push_samples(&self.scratch);
            state.last_sample = now;
        }
        let stopped = now.duration_since(state.last_sample) >= STOPPED_AFTER;
        let elapsed_ms = i64::try_from(self.clock.elapsed().as_millis()).unwrap_or(i64::MAX);
        let reading = state
            .detector
            .get_reading(elapsed_ms.saturating_add(CLOCK_START_MS));
        self.state = Some(state);
        if stopped { SILENT_READING } else { reading }
    }
}

impl Default for LiveTuner {
    fn default() -> Self {
        Self::new()
    }
}

impl TunerState {
    /// A detector with an empty window. Only the input from now on counts.
    fn start(stream: &Arc<AudioTelemetry>, now: Instant) -> Option<Self> {
        let mut detector = TunerDetector::new(f64::from(stream.sample_rate())).ok()?;
        detector.set_enabled(true);
        Some(Self {
            stream: Arc::clone(stream),
            detector,
            read_position: stream.tuner_samples_written(),
            last_read: now,
            last_sample: now,
        })
    }
}
