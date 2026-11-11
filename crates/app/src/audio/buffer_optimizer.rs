//! The device buffer that OPTIMIZE asks for: the smallest that plays with no underruns.
//!
//! This is a pure state machine, so that a test needs no device and no real time. The engine
//! asks it for a size when it opens a stream, and tells it about each underrun. A step only goes
//! up, and the last of [`STEP_FRAMES`] is the cap, so that the engine cannot restart the stream in a
//! loop. Only a new probe can give a smaller size again.

use std::{
    ops::RangeInclusive,
    time::{Duration, Instant},
};

/// The sizes that a probe tries, smallest first. The first that the device takes wins.
pub(crate) const PROBE_FRAMES: [u32; 3] = [64, 128, 256];

/// The sizes that a step up goes through. The last one, 512, is the cap.
pub(crate) const STEP_FRAMES: [u32; 4] = [64, 128, 256, 512];

/// This many underruns in [`UNDERRUN_WINDOW`] make the buffer one step larger.
pub(crate) const UNDERRUN_LIMIT: usize = 3;

/// The time in which [`UNDERRUN_LIMIT`] underruns must come.
pub(crate) const UNDERRUN_WINDOW: Duration = Duration::from_secs(10);

/// Underruns in this time after a stream starts do not count, because a start can glitch.
pub(crate) const COOL_DOWN: Duration = Duration::from_secs(5);

/// The size that the optimizer chose for the running stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BufferChoice {
    /// The identity of the device pair that the size is for.
    pub(crate) device: String,
    /// The size that the stream really runs at.
    pub(crate) frames: u32,
    /// The sizes that both devices take, when they say.
    pub(crate) range: Option<RangeInclusive<u32>>,
}

/// The size of the first stream on a device. A stored result wins, except in a new probe.
#[must_use]
pub(crate) fn chosen_frames(
    stored: Option<u32>,
    probe: bool,
    range: Option<&RangeInclusive<u32>>,
) -> u32 {
    match stored {
        Some(frames) if !probe => frames,
        _ => probe_frames(range),
    }
}

/// The smallest probe size that the device takes. A device that takes none of them gets its
/// size nearest to them.
#[must_use]
pub(crate) fn probe_frames(range: Option<&RangeInclusive<u32>>) -> u32 {
    let Some(range) = range else {
        return PROBE_FRAMES[0];
    };
    PROBE_FRAMES
        .into_iter()
        .find(|frames| range.contains(frames))
        .unwrap_or_else(|| PROBE_FRAMES[0].clamp(*range.start(), *range.end()))
}

/// The next larger step that the device takes, or `None` at the cap.
#[must_use]
pub(crate) fn next_frames(current: u32, range: Option<&RangeInclusive<u32>>) -> Option<u32> {
    STEP_FRAMES
        .into_iter()
        .find(|frames| *frames > current && range.is_none_or(|range| range.contains(frames)))
}

/// What the optimizer knows about the running stream.
#[derive(Debug, Default)]
pub(crate) struct BufferOptimizer {
    running: Option<BufferChoice>,
    /// The times of the latest underruns that count, oldest first.
    underruns: Vec<Instant>,
    /// Underruns before this time do not count.
    quiet_until: Option<Instant>,
    /// Whether an underrun can ask for a restart. A restart that waits ends this until the next
    /// stream starts.
    armed: bool,
}

impl BufferOptimizer {
    /// Records the stream that started with a size of the optimizer.
    pub(crate) fn installed(&mut self, choice: BufferChoice, now: Instant) {
        self.running = Some(choice);
        self.underruns.clear();
        self.quiet_until = Some(now + COOL_DOWN);
        self.armed = true;
    }

    /// Records a stream with no size of the optimizer, for example with OPTIMIZE off.
    pub(crate) fn stop(&mut self) {
        self.running = None;
        self.underruns.clear();
        self.quiet_until = None;
        self.armed = false;
    }

    /// The choice of the running stream.
    pub(crate) const fn running(&self) -> Option<&BufferChoice> {
        self.running.as_ref()
    }

    /// Records one underrun. Returns the larger size to restart the stream at, when the
    /// underruns pass the limit and a larger size exists.
    ///
    /// After it returns a size, no underrun counts until the next [`Self::installed`], so that
    /// one burst of underruns gives one restart.
    pub(crate) fn underrun(&mut self, now: Instant) -> Option<u32> {
        let running = self.running.as_ref().filter(|_| self.armed)?;
        if self.quiet_until.is_some_and(|until| now < until) {
            return None;
        }
        self.underruns
            .retain(|time| now.saturating_duration_since(*time) < UNDERRUN_WINDOW);
        self.underruns.push(now);
        if self.underruns.len() < UNDERRUN_LIMIT {
            return None;
        }
        self.underruns.clear();
        let next = next_frames(running.frames, running.range.as_ref())?;
        self.armed = false;
        Some(next)
    }

    /// Ends the cool-down of the running stream, for a test with no real time.
    #[cfg(test)]
    pub(crate) fn end_cool_down(&mut self) {
        self.quiet_until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(frames: u32, range: Option<RangeInclusive<u32>>) -> BufferChoice {
        BufferChoice {
            device: "System|in|out".to_owned(),
            frames,
            range,
        }
    }

    #[test]
    fn a_probe_asks_for_the_smallest_size_that_the_device_takes() {
        assert_eq!(probe_frames(None), 64);
        assert_eq!(probe_frames(Some(&(32..=4096))), 64);
        assert_eq!(probe_frames(Some(&(100..=4096))), 128);
        assert_eq!(probe_frames(Some(&(200..=4096))), 256);
        assert_eq!(
            probe_frames(Some(&(300..=4096))),
            300,
            "the smallest size it takes"
        );
        assert_eq!(probe_frames(Some(&(16..=48))), 48, "the size nearest to 64");
    }

    #[test]
    fn a_stored_result_is_used_with_no_probe_and_a_new_probe_starts_again() {
        let range = 32..=4096;
        assert_eq!(
            chosen_frames(None, false, Some(&range)),
            64,
            "no result: probe"
        );
        assert_eq!(chosen_frames(Some(256), false, Some(&range)), 256);
        assert_eq!(
            chosen_frames(Some(256), true, Some(&range)),
            64,
            "a new probe"
        );
    }

    #[test]
    fn a_step_goes_up_one_size_at_a_time_and_stops_at_512() {
        assert_eq!(next_frames(64, None), Some(128));
        assert_eq!(next_frames(128, None), Some(256));
        assert_eq!(next_frames(256, None), Some(512));
        assert_eq!(next_frames(512, None), None, "the cap");
        assert_eq!(next_frames(100, None), Some(128));
        assert_eq!(next_frames(64, Some(&(64..=200))), Some(128));
        assert_eq!(
            next_frames(128, Some(&(64..=200))),
            None,
            "the device takes no more"
        );
    }

    #[test]
    fn three_underruns_in_ten_seconds_step_up_once() {
        let start = Instant::now();
        let mut optimizer = BufferOptimizer::default();
        assert_eq!(optimizer.underrun(start), None, "no choice runs");
        optimizer.installed(choice(64, None), start);

        // The cool-down after the start.
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        for _ in 0..UNDERRUN_LIMIT {
            assert_eq!(optimizer.underrun(at(1)), None, "the start can glitch");
        }
        assert_eq!(optimizer.underrun(at(6)), None);
        assert_eq!(optimizer.underrun(at(7)), None);
        assert_eq!(optimizer.underrun(at(8)), Some(128));
        // The restart takes time, and more underruns do not ask again.
        for second in 9..40 {
            assert_eq!(optimizer.underrun(at(second)), None);
        }
    }

    #[test]
    fn underruns_spread_over_more_than_ten_seconds_do_not_step_up() {
        let start = Instant::now();
        let mut optimizer = BufferOptimizer::default();
        optimizer.installed(choice(128, None), start);
        optimizer.end_cool_down();
        for second in [0, 6, 12, 18, 24, 30] {
            assert_eq!(
                optimizer.underrun(start + Duration::from_secs(second)),
                None,
                "{second}"
            );
        }
    }

    #[test]
    fn the_underruns_of_the_largest_size_restart_nothing() {
        let start = Instant::now();
        let mut optimizer = BufferOptimizer::default();
        optimizer.installed(choice(512, None), start);
        optimizer.end_cool_down();
        for _ in 0..10 * UNDERRUN_LIMIT {
            assert_eq!(optimizer.underrun(start), None);
        }

        optimizer.installed(choice(128, Some(64..=128)), start);
        optimizer.end_cool_down();
        for _ in 0..10 * UNDERRUN_LIMIT {
            assert_eq!(optimizer.underrun(start), None, "the device takes no more");
        }
    }

    #[test]
    fn a_stopped_optimizer_counts_no_underrun() {
        let start = Instant::now();
        let mut optimizer = BufferOptimizer::default();
        optimizer.installed(choice(64, None), start);
        optimizer.stop();
        assert_eq!(optimizer.running(), None);
        for _ in 0..10 {
            assert_eq!(optimizer.underrun(start + COOL_DOWN), None);
        }
    }
}
