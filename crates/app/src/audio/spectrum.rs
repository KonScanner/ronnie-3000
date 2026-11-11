//! The spectrum of one block of the running chain, analyzed on the thread that reads.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use ronie_dsp::block::{SpectrumAnalyzer, SpectrumTap};

/// A gap this long between reads means that the page closed, so that the smoothed bins are old.
/// The UI reads at 30 Hz, and at 4 Hz while the window hides, and this gap is longer.
const CLOSED_AFTER: Duration = Duration::from_millis(400);

/// The analyzer skips an analysis under this age. The first read must not count as too young.
const CLOCK_START_MS: i64 = 1_000;

/// Reader of the EQ spectrum of one block at a time. Each read keeps the tap of that block open.
pub struct LiveSpectrum {
    clock: Instant,
    state: Option<(SpectrumAnalyzer, Instant)>,
}

impl LiveSpectrum {
    /// Creates a reader with no block.
    #[must_use]
    pub fn new() -> Self {
        Self {
            clock: Instant::now(),
            state: None,
        }
    }

    /// Reads the newest bins of one block, and keeps its tap open.
    pub fn read(&mut self, tap: &Arc<SpectrumTap>) -> [f32; 64] {
        tap.open();
        let now = Instant::now();
        let current = self.state.take().filter(|(analyzer, last_read)| {
            Arc::ptr_eq(analyzer.tap(), tap) && now.duration_since(*last_read) < CLOSED_AFTER
        });
        let (mut analyzer, _) =
            current.unwrap_or_else(|| (SpectrumAnalyzer::new(Arc::clone(tap)), now));
        let elapsed_ms = i64::try_from(self.clock.elapsed().as_millis()).unwrap_or(i64::MAX);
        let bins = analyzer.bins(elapsed_ms.saturating_add(CLOCK_START_MS));
        self.state = Some((analyzer, now));
        bins
    }
}

impl Default for LiveSpectrum {
    fn default() -> Self {
        Self::new()
    }
}
