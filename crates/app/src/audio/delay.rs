//! The estimate of the delay from the instrument to the ears.

/// The parts of the delay from the input to the output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DelayParts {
    /// The stream sample rate.
    pub(crate) sample_rate: u32,
    /// The frames of one input buffer, or zero with no capture device.
    pub(crate) input_frames: usize,
    /// The frames of one output buffer.
    pub(crate) output_frames: usize,
    /// Whether a sound server plays the output. A PulseAudio server holds about two buffers.
    pub(crate) server_playback: bool,
    /// The frames that stay in the queue between the capture and the playback.
    pub(crate) queue_frames: usize,
    /// The delay of the chain, for example of the pitch shift.
    pub(crate) chain_frames: u64,
}

/// The estimated delay in milliseconds: one input buffer, one output buffer (two on a sound
/// server), the queue, and the chain.
#[must_use]
pub(crate) fn round_trip_ms(parts: DelayParts) -> f64 {
    if parts.sample_rate == 0 {
        return 0.0;
    }
    let playback = parts.output_frames * if parts.server_playback { 2 } else { 1 };
    let frames = parts.input_frames as f64
        + playback as f64
        + parts.queue_frames as f64
        + parts.chain_frames as f64;
    frames * 1_000.0 / f64::from(parts.sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> DelayParts {
        DelayParts {
            sample_rate: 48_000,
            input_frames: 128,
            output_frames: 128,
            server_playback: false,
            queue_frames: 128,
            chain_frames: 0,
        }
    }

    #[test]
    fn the_estimate_adds_the_buffers_the_queue_and_the_chain() {
        // Three buffers of 128 frames at 48 kHz are 8 ms.
        assert!((round_trip_ms(parts()) - 8.0).abs() < 1e-9);

        let server = DelayParts {
            server_playback: true,
            ..parts()
        };
        assert!(
            (round_trip_ms(server) - 32.0 / 3.0).abs() < 1e-9,
            "a server plays two output buffers"
        );

        let pitch = DelayParts {
            chain_frames: 960,
            ..parts()
        };
        assert!((round_trip_ms(pitch) - 28.0).abs() < 1e-9, "20 ms of pitch");

        let no_input = DelayParts {
            input_frames: 0,
            queue_frames: 0,
            ..parts()
        };
        assert!((round_trip_ms(no_input) - 8.0 / 3.0).abs() < 1e-9);

        let no_rate = DelayParts {
            sample_rate: 0,
            ..parts()
        };
        assert_eq!(round_trip_ms(no_rate), 0.0);
    }
}
