use ronie_core::{EqBand, EqBandType, EqParams};

const BANDS: usize = 6;

/// Six band controls with no allocation or unvalidated parameter values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqControls {
    /// Whether EQ processing is enabled.
    pub enabled: bool,
    /// Whether EQ runs before the model rather than after it.
    pub pre: bool,
    /// Ordered channel-strip bands.
    pub bands: [EqBand; BANDS],
}

impl EqControls {
    fn reference_defaults() -> Self {
        Self {
            enabled: true,
            pre: false,
            bands: [
                EqBand {
                    kind: EqBandType::Lowshelf,
                    freq_hz: 100.0,
                    gain_db: 0.0,
                    q: 0.71,
                },
                EqBand {
                    kind: EqBandType::Bell,
                    freq_hz: 250.0,
                    gain_db: 0.0,
                    q: 1.0,
                },
                EqBand {
                    kind: EqBandType::Bell,
                    freq_hz: 650.0,
                    gain_db: 0.0,
                    q: 1.0,
                },
                EqBand {
                    kind: EqBandType::Bell,
                    freq_hz: 1600.0,
                    gain_db: 0.0,
                    q: 1.0,
                },
                EqBand {
                    kind: EqBandType::Bell,
                    freq_hz: 3500.0,
                    gain_db: 0.0,
                    q: 1.4,
                },
                EqBand {
                    kind: EqBandType::Highshelf,
                    freq_hz: 8000.0,
                    gain_db: 0.0,
                    q: 0.71,
                },
            ],
        }
    }
}

impl Default for EqControls {
    fn default() -> Self {
        Self::reference_defaults()
    }
}

impl From<&EqParams> for EqControls {
    fn from(params: &EqParams) -> Self {
        let mut controls = Self {
            enabled: params.enabled,
            pre: params.pre,
            ..Self::reference_defaults()
        };
        for (index, band) in params.bands.iter().take(BANDS).enumerate() {
            controls.bands[index] = *band;
        }
        controls
    }
}

impl From<EqParams> for EqControls {
    fn from(params: EqParams) -> Self {
        Self::from(&params)
    }
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl Biquad {
    fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }

    fn process(&mut self, input: f32, channel: usize) -> f32 {
        let output = self.b0 * input + self.z1[channel];
        self.z1[channel] = self.b1 * input - self.a1 * output + self.z2[channel];
        self.z2[channel] = self.b2 * input - self.a2 * output;
        output
    }
}

/// Prepared six-band EQ with independent stereo filter histories.
pub struct EqEngine {
    sample_rate: f64,
    bands: [EqBand; BANDS],
    filters: [Biquad; BANDS],
    active: [bool; BANDS],
    any_active: bool,
    enabled: bool,
    pre: bool,
}

impl EqEngine {
    /// Prepares coefficients for a fixed chain sample rate.
    #[must_use]
    pub fn new(sample_rate: f64, controls: &EqControls) -> Self {
        let mut engine = Self {
            sample_rate: if sample_rate > 0.0 {
                sample_rate
            } else {
                48_000.0
            },
            bands: [EqBand::default(); BANDS],
            filters: [Biquad::default(); BANDS],
            active: [false; BANDS],
            any_active: false,
            enabled: true,
            pre: false,
        };
        engine.update_controls(controls);
        engine
    }

    /// Updates changed coefficients outside the sample loop and resets stale history.
    pub fn update_controls(&mut self, controls: &EqControls) {
        let was_active = self.any_active;
        if self.pre != controls.pre || (!self.enabled && controls.enabled) {
            self.reset();
        }
        for index in 0..BANDS {
            let band = clamp_band(index, controls.bands[index]);
            if self.bands[index] != band {
                self.bands[index] = band;
                self.update_band(index);
                self.filters[index].reset();
            }
        }
        self.enabled = controls.enabled;
        self.pre = controls.pre;
        self.update_activity();
        if !was_active && self.any_active {
            self.reset();
        }
    }

    /// Whether any enabled band has a non-flat transfer function.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.enabled && self.any_active
    }

    /// Returns `[b0, b1, b2, a1, a2]` for one band, or `None` out of range.
    #[must_use]
    pub fn coefficients(&self, index: usize) -> Option<[f32; 5]> {
        let filter = self.filters.get(index)?;
        Some([filter.b0, filter.b1, filter.b2, filter.a1, filter.a2])
    }

    /// Processes one planar buffer in place without allocating.
    pub fn process(&mut self, data: &mut [f32], channels: usize) {
        debug_assert!(matches!(channels, 1 | 2));
        debug_assert_eq!(data.len() % channels, 0);
        if !self.is_active() {
            return;
        }
        let frames = data.len() / channels;
        for band in 0..BANDS {
            if !self.active[band] {
                continue;
            }
            for channel in 0..channels {
                let filter = &mut self.filters[band];
                for frame in 0..frames {
                    let index = channel * frames + frame;
                    data[index] = filter.process(data[index], channel);
                }
            }
        }
    }

    fn reset(&mut self) {
        for filter in &mut self.filters {
            filter.reset();
        }
    }

    fn update_activity(&mut self) {
        self.any_active = self.active.into_iter().any(|active| active);
    }

    fn update_band(&mut self, index: usize) {
        let band = self.bands[index];
        self.active[index] = match band.kind {
            EqBandType::Bell | EqBandType::Lowshelf | EqBandType::Highshelf => {
                band.gain_db.abs() >= 0.05
            }
            EqBandType::Lowcut | EqBandType::Highcut => true,
        };
        let frequency = band
            .freq_hz
            .clamp(20.0, 20_000.0_f64.min(self.sample_rate * 0.49));
        let a = 10.0_f64.powf(band.gain_db / 40.0);
        let omega = 2.0 * std::f64::consts::PI * frequency / self.sample_rate;
        let sn = omega.sin();
        let cs = omega.cos();
        let alpha = sn / (2.0 * band.q);
        let sqrt_a = a.sqrt();
        let (b0, b1, b2, a0, a1, a2) = match band.kind {
            EqBandType::Lowcut => (
                (1.0 + cs) * 0.5,
                -(1.0 + cs),
                (1.0 + cs) * 0.5,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            EqBandType::Highcut => (
                (1.0 - cs) * 0.5,
                1.0 - cs,
                (1.0 - cs) * 0.5,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            EqBandType::Bell => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            EqBandType::Lowshelf => (
                a * ((a + 1.0) - (a - 1.0) * cs + 2.0 * sqrt_a * alpha),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - 2.0 * sqrt_a * alpha),
                (a + 1.0) + (a - 1.0) * cs + 2.0 * sqrt_a * alpha,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - 2.0 * sqrt_a * alpha,
            ),
            EqBandType::Highshelf => (
                a * ((a + 1.0) + (a - 1.0) * cs + 2.0 * sqrt_a * alpha),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - 2.0 * sqrt_a * alpha),
                (a + 1.0) - (a - 1.0) * cs + 2.0 * sqrt_a * alpha,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - 2.0 * sqrt_a * alpha,
            ),
        };
        let norm = 1.0 / a0;
        self.filters[index].b0 = (b0 * norm) as f32;
        self.filters[index].b1 = (b1 * norm) as f32;
        self.filters[index].b2 = (b2 * norm) as f32;
        self.filters[index].a1 = (a1 * norm) as f32;
        self.filters[index].a2 = (a2 * norm) as f32;
    }
}

fn clamp_band(index: usize, mut band: EqBand) -> EqBand {
    band.freq_hz = band.freq_hz.clamp(20.0, 20_000.0);
    band.gain_db = band.gain_db.clamp(-15.0, 15.0);
    band.q = band.q.clamp(0.1, 10.0);
    band.kind = match (index, band.kind) {
        (0, EqBandType::Lowcut | EqBandType::Lowshelf) => band.kind,
        (0, _) => EqBandType::Lowshelf,
        (5, EqBandType::Highcut | EqBandType::Highshelf) => band.kind,
        (5, _) => EqBandType::Highshelf,
        _ => EqBandType::Bell,
    };
    band
}
