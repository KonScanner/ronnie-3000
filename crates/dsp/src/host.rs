use crate::{LatencyReport, PrepareSpec};

/// Host bus and callback capabilities used by one prepared chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostCapabilities {
    /// Host sample rate in hertz.
    pub sample_rate: f64,
    /// Maximum promised callback frames.
    pub max_host_frames: usize,
    /// Whether two input channels are available.
    pub stereo_input: bool,
    /// Whether two output channels are available.
    pub stereo_output: bool,
}

impl HostCapabilities {
    /// Captures host capabilities from an immutable preparation snapshot.
    #[must_use]
    pub const fn from_spec(spec: PrepareSpec) -> Self {
        Self {
            sample_rate: spec.host_sample_rate,
            max_host_frames: spec.max_host_frames,
            stereo_input: spec.stereo_input,
            stereo_output: spec.stereo_output,
        }
    }
}

/// Host-visible bus arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusLayout {
    /// One input and output channel.
    Mono,
    /// Two input and output channels.
    Stereo,
}

/// Host-visible bus, delay, and tail report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostReport {
    /// Current output bus arrangement.
    pub bus_layout: BusLayout,
    /// Total delay in host-rate samples.
    pub latency_samples: u64,
    /// Total output tail in seconds.
    pub tail_seconds: f64,
}

impl HostReport {
    pub(crate) fn from_parts(capabilities: HostCapabilities, latency: LatencyReport) -> Self {
        Self {
            bus_layout: if capabilities.stereo_output {
                BusLayout::Stereo
            } else {
                BusLayout::Mono
            },
            latency_samples: latency.samples,
            tail_seconds: latency.tail_seconds,
        }
    }
}
