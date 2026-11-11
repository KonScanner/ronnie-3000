use crate::{ChainError, PlanarBuffer, block::gain::LinearGain};

const FADE_MS: f64 = 30.0;
const FADE_MAX_MS: f64 = 120.0;
const FADE_MIN_MS: f64 = 6.0;
const FADE_NCC_HI: f64 = 0.95;
const FADE_NCC_LO: f64 = 0.6;
/// Largest share of the expected time to the next splice that a fade fills.
const FADE_SPLICE_SHARE: f64 = 0.5;
const ONSET_FADE_MS: f64 = 2.0;
/// One period of a 31 Hz note (bass low B), so a splice can align low notes.
const MAX_CORR_MS: f64 = 33.0;
const ONSET_SPAN_MS: f64 = 4.0;
// A note whose power is mostly below ONSET_LOW_HZ is a low note. On a low
// note in a window of ONSET_LOW_MIN_WINDOW_MS or more (the bass windows), the
// onset search spans ONSET_LOW_SPAN_SHARE of the window, so that it can reach
// one full period and find an aligned splice. That search is spread over the
// search lead, and its fade is ONSET_LOW_FADE_MS. If the best correlation is
// below ONSET_LOW_NCC_MIN, no aligned splice is in reach, and the shifter
// does not splice on that onset. All other onsets, for example a guitar pick
// attack in the 20 ms window, keep the tight 4 ms span and 2 ms fade.
const ONSET_LOW_HZ: f64 = 200.0;
const ONSET_LOW_MIN_WINDOW_MS: i32 = 40;
const ONSET_LOW_SHARE: f64 = 0.5;
const ONSET_LOW_SPAN_SHARE: f64 = 0.75;
const ONSET_LOW_FADE_MS: f64 = 6.0;
const ONSET_LOW_NCC_MIN: f64 = 0.9;
const LOW_POWER_MS: f64 = 10.0;
const REFRACTORY_MS: f64 = 40.0;
const DETECTOR_HPF_HZ: f64 = 600.0;
const DETECTOR_SMOOTH_MS: f64 = 2.0;
const HISTORY_CELLS: usize = 50;
const HISTORY_SKIP_CELLS: usize = 5;
const ONSET_OVER_MIN: f64 = 7.94;
const ONSET_OVER_MAX: f64 = 3.98;
const COARSE_STEP_RATE: f64 = 12_000.0;
const SEARCH_LEAD_MS: f64 = 4.0;
const UPSHIFT_LAND_SHARE: f64 = 0.5;
const SEARCH_KEEP_RATIO: f64 = 0.1;
const BLEND_SECONDS: f64 = 0.025;
const MIN_DELAY_MS: f64 = 2.0;
const TONALITY_MIN_HZ: f32 = 1000.0;
const TONALITY_OFF_HZ: f32 = 20_000.0;

/// Delay-buffer duration used by the pitch shifter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PitchWindow {
    /// Twenty milliseconds, the default and the lowest delay; intended for guitar.
    #[default]
    Ms20,
    /// Thirty milliseconds, a general-purpose setting.
    Ms30,
    /// Forty milliseconds.
    Ms40,
    /// Sixty milliseconds.
    Ms60,
}

impl PitchWindow {
    /// Resolves a clamped host choice index.
    #[must_use]
    pub fn from_index(index: i32) -> Self {
        match index.clamp(0, 3) {
            0 => Self::Ms20,
            1 => Self::Ms30,
            2 => Self::Ms40,
            _ => Self::Ms60,
        }
    }

    /// Window duration in milliseconds.
    #[must_use]
    pub const fn milliseconds(self) -> i32 {
        match self {
            Self::Ms20 => 20,
            Self::Ms30 => 30,
            Self::Ms40 => 40,
            Self::Ms60 => 60,
        }
    }

    /// Mean reported delay in milliseconds.
    #[must_use]
    pub fn latency_milliseconds(self) -> f64 {
        (MIN_DELAY_MS + f64::from(self.milliseconds())) * 0.5
    }
}

/// Pitch-shifter controls in real units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchParams {
    /// Continuous shift from -24 to +24 semitones.
    pub semitones: f32,
    /// Crossover above which dry audio replaces the shift; zero disables it.
    pub tonality_hz: f32,
    /// Delay-buffer duration.
    pub window: PitchWindow,
}

impl Default for PitchParams {
    fn default() -> Self {
        Self {
            semitones: 0.0,
            tonality_hz: 0.0,
            window: PitchWindow::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct Ring {
    buffer: Vec<f32>,
    mask: u32,
}

impl Ring {
    fn new(minimum_size: usize) -> Self {
        let size = minimum_size.max(1).next_power_of_two();
        Self {
            buffer: vec![0.0; size],
            mask: size as u32 - 1,
        }
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
    }

    fn write(&mut self, index: i64, sample: f32) {
        self.buffer[(index as u32 & self.mask) as usize] = sample;
    }

    fn at(&self, index: i64) -> f32 {
        self.buffer[(index as u32 & self.mask) as usize]
    }

    fn read(&self, position: f64) -> f32 {
        let index = position.floor() as i64;
        let fraction = (position - index as f64) as f32;
        let xm1 = self.at(index - 1);
        let x0 = self.at(index);
        let x1 = self.at(index + 1);
        let x2 = self.at(index + 2);
        let c = 0.5 * (x1 - xm1);
        let v = x0 - x1;
        let w = c + v;
        let a = w + v + 0.5 * (x2 - x0);
        let b = w + a;
        ((a * fraction - b) * fraction + c) * fraction + x0
    }
}

#[derive(Debug, Clone)]
struct MirroredRing {
    buffer: Vec<f32>,
    mask: u32,
    size: usize,
}

impl MirroredRing {
    fn new(minimum_size: usize) -> Self {
        let size = minimum_size.max(1).next_power_of_two();
        Self {
            buffer: vec![0.0; size * 2],
            mask: size as u32 - 1,
            size,
        }
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
    }

    fn write(&mut self, index: i64, sample: f32) {
        let slot = (index as u32 & self.mask) as usize;
        self.buffer[slot] = sample;
        self.buffer[slot + self.size] = sample;
    }

    fn window_ending_at(&self, index: i64, count: usize) -> &[f32] {
        let slot = (index as u32 & self.mask) as usize;
        &self.buffer[slot + self.size - count..slot + self.size]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LrKind {
    Lowpass,
    Highpass,
}

#[derive(Debug, Clone, Copy)]
struct Lr4 {
    kind: LrKind,
    sample_rate: f64,
    cutoff: f32,
    g: f32,
    r2: f32,
    h: f32,
    states: [[f32; 2]; 4],
}

impl Lr4 {
    fn new(sample_rate: f64, cutoff: f32, kind: LrKind) -> Self {
        let mut filter = Self {
            kind,
            sample_rate,
            cutoff,
            g: 0.0,
            r2: std::f32::consts::SQRT_2,
            h: 0.0,
            states: [[0.0; 2]; 4],
        };
        filter.update();
        filter
    }

    fn set_cutoff(&mut self, cutoff: f32) {
        self.cutoff = cutoff;
        self.update();
    }

    fn update(&mut self) {
        self.g = (std::f64::consts::PI * f64::from(self.cutoff) / self.sample_rate).tan() as f32;
        self.h = 1.0 / (1.0 + self.r2 * self.g + self.g * self.g);
    }

    fn process(&mut self, channel: usize, input: f32) -> f32 {
        let s1 = self.states[0][channel];
        let s2 = self.states[1][channel];
        let yh = (input - (self.r2 + self.g) * s1 - s2) * self.h;
        let yb = self.g * yh + s1;
        self.states[0][channel] = self.g * yh + yb;
        let yl = self.g * yb + s2;
        self.states[1][channel] = self.g * yb + yl;

        let s3 = self.states[2][channel];
        let s4 = self.states[3][channel];
        let source = if self.kind == LrKind::Lowpass { yl } else { yh };
        let yh2 = (source - (self.r2 + self.g) * s3 - s4) * self.h;
        let yb2 = self.g * yh2 + s3;
        self.states[2][channel] = self.g * yh2 + yb2;
        let yl2 = self.g * yb2 + s4;
        self.states[3][channel] = self.g * yb2 + yl2;
        if self.kind == LrKind::Lowpass {
            yl2
        } else {
            yh2
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Search {
    active: bool,
    ready: bool,
    onset: bool,
    lo: i32,
    hi: i32,
    next: i32,
    per_sample: i32,
    best_score: f64,
    best_ncc: f64,
    best_delay: i32,
    now0: i64,
    a0: i64,
    result_jump: i64,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            active: false,
            ready: false,
            onset: false,
            lo: 0,
            hi: 0,
            next: 0,
            per_sample: 1,
            best_score: -1.0e9,
            best_ncc: 0.0,
            best_delay: 0,
            now0: 0,
            a0: 0,
            result_jump: 0,
        }
    }
}

/// Allocation-free correlation-spliced delay-line pitch shifter.
pub struct PitchShifter {
    pub(crate) sample_rate: f64,
    params: PitchParams,
    ratio: f64,
    enabled: bool,
    running: bool,
    wet_mix: LinearGain,
    prime_left: i32,
    rings: [Ring; 2],
    control: MirroredRing,
    dry_rings: [Ring; 2],
    written: i64,
    d_min: i32,
    d_max: i32,
    corr_len: usize,
    fade_len: i32,
    fade_max_len: i32,
    fade_min_len: i32,
    fade_lo: i32,
    fade_hi: i32,
    onset_fade_len: i32,
    onset_span: i32,
    onset_low_fade_len: i32,
    onset_low_span: i32,
    refractory: i32,
    cell_len: i32,
    coarse_step: i32,
    search_lead: i32,
    search_lead_now: i32,
    r_a: f64,
    r_b: f64,
    fading: bool,
    fade: f64,
    fade_inc: f64,
    fade_ncc: f64,
    search: Search,
    reference: Vec<f32>,
    ref_xx: f64,
    hp_a: f64,
    hp_z1: f64,
    hp_z2: f64,
    smooth_a: f64,
    e_hf: f64,
    low_a: f64,
    low_z: f64,
    power_a: f64,
    low_power: f64,
    full_power: f64,
    min_hist: [f32; HISTORY_CELLS],
    max_hist: [f32; HISTORY_CELLS],
    min_acc: f64,
    max_acc: f64,
    last_onset: i64,
    onset_pending: Option<i64>,
    wet_lowpass: Lr4,
    dry_highpass: Lr4,
    tonality_mix: LinearGain,
}

impl PitchShifter {
    /// Allocates maximum rings and search storage and prepares a stopped engine.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64, max_block_samples: usize) -> Result<Self, ChainError> {
        let mut shifter = Self {
            sample_rate: 0.0,
            params: PitchParams::default(),
            ratio: 1.0,
            enabled: false,
            running: false,
            wet_mix: LinearGain::new(sample_rate, BLEND_SECONDS, 0.0),
            prime_left: 0,
            rings: [Ring::new(1), Ring::new(1)],
            control: MirroredRing::new(1),
            dry_rings: [Ring::new(1), Ring::new(1)],
            written: 0,
            d_min: 0,
            d_max: 0,
            corr_len: 0,
            fade_len: 0,
            fade_max_len: 0,
            fade_min_len: 0,
            fade_lo: 0,
            fade_hi: 0,
            onset_fade_len: 0,
            onset_span: 0,
            onset_low_fade_len: 0,
            onset_low_span: 0,
            refractory: 0,
            cell_len: 1,
            coarse_step: 1,
            search_lead: 1,
            search_lead_now: 1,
            r_a: 0.0,
            r_b: 0.0,
            fading: false,
            fade: 0.0,
            fade_inc: 0.0,
            fade_ncc: 1.0,
            search: Search::default(),
            reference: Vec::new(),
            ref_xx: 0.0,
            hp_a: 0.0,
            hp_z1: 0.0,
            hp_z2: 0.0,
            smooth_a: 0.0,
            e_hf: 0.0,
            low_a: 0.0,
            low_z: 0.0,
            power_a: 0.0,
            low_power: 0.0,
            full_power: 0.0,
            min_hist: [1.0e9; HISTORY_CELLS],
            max_hist: [0.0; HISTORY_CELLS],
            min_acc: 1.0e9,
            max_acc: 0.0,
            last_onset: -(1 << 30),
            onset_pending: None,
            wet_lowpass: Lr4::new(sample_rate, 2000.0, LrKind::Lowpass),
            dry_highpass: Lr4::new(sample_rate, 2000.0, LrKind::Highpass),
            tonality_mix: LinearGain::new(sample_rate, BLEND_SECONDS, 0.0),
        };
        shifter.prepare(sample_rate, max_block_samples)?;
        Ok(shifter)
    }

    /// Reprepares rates and maximum storage while retaining parameter choices.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn prepare(
        &mut self,
        sample_rate: f64,
        _max_block_samples: usize,
    ) -> Result<(), ChainError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(ChainError::InvalidLayout("pitch sample rate is invalid"));
        }
        self.sample_rate = sample_rate;
        self.fade_len = (sample_rate * FADE_MS * 0.001) as i32;
        self.fade_len = self.fade_len.max(8);
        self.fade_max_len = ((sample_rate * FADE_MAX_MS * 0.001) as i32).max(self.fade_len);
        self.fade_min_len = ((sample_rate * FADE_MIN_MS * 0.001) as i32).max(8);
        self.onset_fade_len = ((sample_rate * ONSET_FADE_MS * 0.001) as i32).max(8);
        self.onset_span = (sample_rate * ONSET_SPAN_MS * 0.001) as i32;
        self.onset_low_fade_len = ((sample_rate * ONSET_LOW_FADE_MS * 0.001) as i32).max(8);
        self.low_a = (-2.0 * std::f64::consts::PI * ONSET_LOW_HZ / sample_rate).exp();
        self.power_a = (-1.0 / (sample_rate * LOW_POWER_MS * 0.001)).exp();
        self.refractory = (sample_rate * REFRACTORY_MS * 0.001) as i32;
        self.search_lead = ((sample_rate * SEARCH_LEAD_MS * 0.001) as i32).max(1);
        self.cell_len = ((sample_rate * 0.001) as i32).max(1);
        self.coarse_step = (sample_rate / COARSE_STEP_RATE).round() as i32;
        self.coarse_step = self.coarse_step.max(1);
        self.hp_a = (-2.0 * std::f64::consts::PI * DETECTOR_HPF_HZ / sample_rate).exp();
        self.smooth_a = (-1.0 / (sample_rate * DETECTOR_SMOOTH_MS * 0.001)).exp();

        let max_corr = (sample_rate * MAX_CORR_MS * 0.001) as usize;
        let reach = Self::window_samples(PitchWindow::Ms60, sample_rate) as usize
            + max_corr
            + self.fade_max_len as usize
            + 2 * self.search_lead as usize
            + 64;
        self.rings = [Ring::new(reach), Ring::new(reach)];
        self.control = MirroredRing::new(reach);
        self.reference = vec![0.0; max_corr];
        self.dry_rings = [
            Ring::new(Self::min_delay_samples(sample_rate) as usize + 8),
            Ring::new(Self::min_delay_samples(sample_rate) as usize + 8),
        ];
        self.wet_lowpass = Lr4::new(sample_rate, self.wet_lowpass.cutoff, LrKind::Lowpass);
        self.dry_highpass = Lr4::new(sample_rate, self.dry_highpass.cutoff, LrKind::Highpass);
        let tonality = if self.params.tonality_hz > 0.0 {
            1.0
        } else {
            0.0
        };
        self.tonality_mix = LinearGain::new(sample_rate, BLEND_SECONDS, tonality);
        self.set_tonality(self.params.tonality_hz);
        self.wet_mix = LinearGain::new(sample_rate, BLEND_SECONDS, 0.0);
        self.wet_mix.set_current_and_target(0.0);
        self.running = self.enabled;
        self.apply_window();
        self.reset();
        Ok(())
    }

    /// Starts or stops the effect with a 25 ms dry/wet transition.
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            self.wet_mix.set_target(0.0);
        } else if self.running {
            self.wet_mix.set_target(1.0);
        } else {
            self.reset();
            self.running = true;
        }
    }

    /// Whether the engine still needs callbacks for its current power blend.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    /// Applies continuous pitch, window, and tonality controls.
    pub fn set_params(&mut self, params: PitchParams) {
        if params.window != self.params.window {
            self.params.window = params.window;
            self.apply_window();
            self.search = Search::default();
        }
        if params.semitones != self.params.semitones {
            self.params.semitones = params.semitones;
            let ratio = 2.0_f64.powf(f64::from(params.semitones) / 12.0);
            if direction(ratio) != direction(self.ratio)
                || (ratio - self.ratio).abs() > SEARCH_KEEP_RATIO
            {
                self.search = Search::default();
            }
            self.ratio = ratio;
            self.update_fade_range();
        }
        if params.tonality_hz != self.params.tonality_hz {
            self.params.tonality_hz = params.tonality_hz;
            self.set_tonality(params.tonality_hz);
        }
    }

    /// Processes one mono or stereo buffer without allocating.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for more than two channels.
    pub fn process(&mut self, buffer: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
        let channels = buffer.channels();
        if !matches!(channels, 1 | 2) {
            return Err(ChainError::InvalidLayout(
                "pitch supports one or two channels",
            ));
        }
        if self.sample_rate <= 0.0 || !self.running {
            return Ok(());
        }
        let ratio = self.ratio;
        let drift = (1.0 - ratio).abs();
        let lead_drift = drift * f64::from(self.search_lead_now + 4);
        let lead_drift_i = lead_drift.ceil() as i32;
        let fade_drift = (drift
            * f64::from(if ratio > 1.0 {
                self.fade_hi
            } else {
                self.fade_min_len
            }))
        .ceil() as i32;

        for frame in 0..buffer.frames() {
            let now = self.written;
            self.written += 1;
            let left = buffer.channel(0)?[frame];
            let right = if channels > 1 {
                buffer.channel(1)?[frame]
            } else {
                left
            };
            let dry = [left, right];
            self.rings[0].write(now, left);
            self.rings[1].write(now, right);
            let control = if channels > 1 {
                0.5 * (left + right)
            } else {
                left
            };
            self.control.write(now, control);

            let delay = now as f64 - self.r_a;
            let guard = self.low_guard();
            let onset = self.detect_onset(control);
            let land_lo = if ratio > 1.0 {
                (guard + fade_drift + lead_drift_i + 2).min(self.d_max)
            } else {
                self.d_min
            };
            let land_hi = if ratio < 1.0 {
                self.d_min.max(self.d_max - fade_drift - lead_drift_i - 2)
            } else {
                self.d_max
            };

            // An onset during a fade does not wait for the full fade. The rest
            // of the fade is shortened to at most the onset fade (the gain
            // stays continuous), and the onset re-splice runs when it ends.
            if self.fading
                && onset
                && self.onset_pending.is_none()
                && now - self.last_onset > i64::from(self.refractory)
            {
                self.onset_pending = Some(now);
                let rest = 1.0 - self.fade;
                if rest > self.fade_inc * f64::from(self.onset_fade_len) {
                    self.fade_inc = rest / f64::from(self.onset_fade_len);
                }
            }
            if !self.fading {
                let pending = self
                    .onset_pending
                    .take()
                    .is_some_and(|at| now - at <= i64::from(self.refractory));
                let onset = onset || pending;
                if self.search.onset {
                    if self.search.ready && self.search.best_ncc >= ONSET_LOW_NCC_MIN {
                        let jump = self.search.result_jump;
                        self.start_fade(jump, self.onset_low_fade_len);
                    } else if self.search.ready {
                        // No aligned splice is in reach; keep playing.
                        self.search = Search::default();
                    }
                } else if onset
                    && now - self.last_onset > i64::from(self.refractory)
                    && delay > f64::from(land_lo + self.onset_span)
                {
                    self.last_onset = now;
                    if self.onset_low_span > self.onset_span && self.low_note() {
                        let hi = (land_lo + self.onset_low_span)
                            .min(self.d_max)
                            .min(delay as i32 - self.onset_span);
                        self.begin_search(land_lo, hi, self.search_lead);
                        self.search.onset = true;
                    } else {
                        self.begin_search(land_lo, (land_lo + self.onset_span).min(self.d_max), 0);
                        let count = self.search.per_sample;
                        self.step_search(count);
                        let jump = self.search.result_jump;
                        self.start_fade(jump, self.onset_fade_len);
                    }
                    self.min_hist.fill(self.e_hf as f32);
                    self.max_hist.fill(self.e_hf as f32);
                    self.min_acc = self.e_hf;
                    self.max_acc = self.e_hf;
                } else if ratio < 1.0 {
                    if !self.search.active
                        && !self.search.ready
                        && delay >= f64::from(self.d_max) - lead_drift
                    {
                        let lo = (land_lo - lead_drift_i).max(4);
                        self.begin_search(
                            lo,
                            (land_hi - lead_drift_i).max(lo),
                            self.search_lead_now,
                        );
                    }
                    if self.search.ready && delay >= f64::from(self.d_max) {
                        let destination = delay - self.search.result_jump as f64;
                        let room = (f64::from(self.d_max) - destination - lead_drift - 2.0) / drift;
                        let jump = self.search.result_jump;
                        let samples = self.fade_for(self.search.best_ncc, room, jump);
                        self.start_fade(jump, samples);
                    }
                } else if ratio > 1.0 {
                    if !self.search.active
                        && !self.search.ready
                        && delay <= f64::from(guard) + lead_drift
                    {
                        self.begin_search(
                            land_lo + lead_drift_i,
                            land_hi + lead_drift_i,
                            self.search_lead_now,
                        );
                    }
                    if self.search.ready && delay <= f64::from(guard) {
                        let jump = self.search.result_jump;
                        let samples =
                            self.fade_for(self.search.best_ncc, f64::from(self.fade_hi), jump);
                        self.start_fade(jump, samples);
                    }
                }
            }
            if self.search.active {
                let count = self.search.per_sample;
                self.step_search(count);
            }

            let mut gain_a = 1.0_f32;
            let mut gain_b = 0.0_f32;
            if self.fading {
                gain_b = (0.5 - 0.5 * (std::f64::consts::PI * self.fade).cos()) as f32;
                gain_a = 1.0 - gain_b;
                let correlation = self.fade_ncc as f32;
                let norm =
                    (gain_a * gain_a + gain_b * gain_b + 2.0 * gain_a * gain_b * correlation)
                        .sqrt();
                gain_a /= norm;
                gain_b /= norm;
                self.fade += self.fade_inc;
            }
            if self.prime_left > 0 {
                self.prime_left -= 1;
                if self.prime_left == 0 && self.enabled {
                    self.wet_mix.set_target(1.0);
                }
            }
            let wet_mix = self.wet_mix.next();
            let tonality_mix = self.tonality_mix.next();
            for (channel, dry_sample) in dry.iter().enumerate().take(channels) {
                let mut wet = self.rings[channel].read(self.r_a);
                if self.fading {
                    wet = wet * gain_a + gain_b * self.rings[channel].read(self.r_b);
                }
                let high = self.dry_highpass.process(channel, *dry_sample);
                self.dry_rings[channel].write(now, high);
                let split = self.wet_lowpass.process(channel, wet)
                    + self.dry_rings[channel].at(now - i64::from(self.d_min));
                wet += tonality_mix * (split - wet);
                let output = dry_sample + wet_mix * (wet - dry_sample);
                buffer.channel_mut(channel)?[frame] = output;
            }
            if self.fading && self.fade >= 1.0 {
                self.r_a = self.r_b;
                self.fading = false;
            }
            self.r_a += ratio;
            self.r_b += ratio;
        }
        if !self.enabled && !self.wet_mix.is_smoothing() {
            self.running = false;
        }
        Ok(())
    }

    /// Mean delay of the current window in host samples.
    #[must_use]
    pub const fn latency_samples(&self) -> u64 {
        ((self.d_min + self.d_max) / 2) as u64
    }

    /// Mean delay of any window at a valid host rate.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn latency_samples_for(window: PitchWindow, sample_rate: f64) -> Result<u64, ChainError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(ChainError::InvalidLayout("pitch sample rate is invalid"));
        }
        Ok((Self::min_delay_samples(sample_rate) + Self::window_samples(window, sample_rate)) / 2)
    }

    /// Fixed onset floor in samples.
    #[must_use]
    pub fn min_delay_samples(sample_rate: f64) -> u64 {
        (sample_rate * MIN_DELAY_MS * 0.001) as u64
    }

    /// Window length in samples.
    #[must_use]
    pub fn window_samples(window: PitchWindow, sample_rate: f64) -> u64 {
        (sample_rate * f64::from(window.milliseconds()) * 0.001) as u64
    }

    fn apply_window(&mut self) {
        self.d_min = Self::min_delay_samples(self.sample_rate) as i32;
        self.d_max = Self::window_samples(self.params.window, self.sample_rate) as i32;
        self.corr_len =
            (self.d_max as usize).min((self.sample_rate * MAX_CORR_MS * 0.001) as usize);
        self.onset_low_span = if self.params.window.milliseconds() >= ONSET_LOW_MIN_WINDOW_MS {
            self.onset_span
                .max((f64::from(self.d_max) * ONSET_LOW_SPAN_SHARE) as i32)
        } else {
            self.onset_span
        };
        self.update_fade_range();
    }

    fn update_fade_range(&mut self) {
        let drift = (1.0 - self.ratio).abs();
        let mut high = f64::from(self.fade_max_len);
        let mut lead = f64::from(self.search_lead);
        let mut floor = self.fade_min_len;
        if self.ratio > 1.0 {
            high = f64::from(self.fade_min_len)
                .max(high.min(f64::from(self.d_max - self.d_min) / (6.0 * drift)));
            let cost = drift * (2.0 * high + lead + 4.0) + 6.0;
            let budget = f64::from(self.d_max) * (1.0 - UPSHIFT_LAND_SHARE);
            if cost > budget {
                let scale = budget / cost;
                high *= scale;
                lead *= scale;
                floor = 8;
            }
        }
        self.fade_hi = floor.max(high as i32);
        self.fade_lo = self.fade_len.min(self.fade_hi);
        self.search_lead_now = (lead as i32).max(1);
    }

    fn low_guard(&self) -> i32 {
        self.d_min
            .max(((self.ratio - 1.0).max(0.0) * f64::from(self.fade_hi)) as i32 + 4)
    }

    /// Fade length from the splice correlation, limited by `room` and by
    /// `FADE_SPLICE_SHARE` of the expected time to the next splice.
    fn fade_for(&self, ncc: f64, room: f64, jump: i64) -> i32 {
        let amount = ((FADE_NCC_HI - ncc) / (FADE_NCC_HI - FADE_NCC_LO)).clamp(0.0, 1.0);
        let length = f64::from(self.fade_lo) + amount * f64::from(self.fade_hi - self.fade_lo);
        let interval = jump.unsigned_abs() as f64 / (1.0 - self.ratio).abs().max(1.0e-6);
        (length.min(room).min(FADE_SPLICE_SHARE * interval) as i32).max(8)
    }

    fn set_tonality(&mut self, hz: f32) {
        let enabled = hz > 0.0;
        if enabled {
            let cutoff = hz.clamp(TONALITY_MIN_HZ, TONALITY_OFF_HZ);
            self.wet_lowpass.set_cutoff(cutoff);
            self.dry_highpass.set_cutoff(cutoff);
        }
        self.tonality_mix
            .set_target(if enabled { 1.0 } else { 0.0 });
    }

    fn reset(&mut self) {
        for ring in &mut self.rings {
            ring.clear();
        }
        self.control.clear();
        for ring in &mut self.dry_rings {
            ring.clear();
        }
        self.r_a = (self.written - i64::from(self.d_min)) as f64;
        self.r_b = self.r_a;
        self.prime_left = self.d_min + 8;
        self.fading = false;
        self.search = Search::default();
        self.hp_z1 = 0.0;
        self.hp_z2 = 0.0;
        self.e_hf = 0.0;
        self.low_z = 0.0;
        self.low_power = 0.0;
        self.full_power = 0.0;
        self.min_hist.fill(1.0e9);
        self.max_hist.fill(0.0);
        self.min_acc = 1.0e9;
        self.max_acc = 0.0;
        self.last_onset = -(1 << 30);
        self.onset_pending = None;
    }

    fn ncc_at(&self, ending: i64) -> f64 {
        let y = self.control.window_ending_at(ending, self.corr_len);
        let x = &self.reference[..self.corr_len];
        let mut xy = [0.0_f32; 8];
        let mut yy = [0.0_f32; 8];
        let mut index = 0;
        while index + 8 <= self.corr_len {
            for lane in 0..8 {
                xy[lane] += x[index + lane] * y[index + lane];
                yy[lane] += y[index + lane] * y[index + lane];
            }
            index += 8;
        }
        while index < self.corr_len {
            xy[0] += x[index] * y[index];
            yy[0] += y[index] * y[index];
            index += 1;
        }
        let mut sxy = 0.0;
        let mut syy = 0.0;
        for lane in 0..8 {
            sxy += f64::from(xy[lane]);
            syy += f64::from(yy[lane]);
        }
        sxy / ((self.ref_xx * syy).sqrt() + 1.0e-9)
    }

    fn begin_search(&mut self, lo: i32, hi: i32, lead: i32) {
        self.search.active = true;
        self.search.ready = false;
        self.search.onset = false;
        self.search.now0 = self.written - 1;
        self.search.a0 = self.r_a.floor() as i64;
        self.search.lo = lo;
        self.search.hi = hi.max(lo);
        self.search.next = lo;
        self.search.best_score = -1.0e9;
        self.search.best_delay = lo;
        let source = self.control.window_ending_at(self.search.a0, self.corr_len);
        self.reference[..self.corr_len].copy_from_slice(source);
        self.ref_xx = 0.0;
        for sample in source {
            self.ref_xx += f64::from(*sample) * f64::from(*sample);
        }
        let candidates = (self.search.hi - self.search.lo) / self.coarse_step + 1;
        self.search.per_sample = if lead > 0 {
            (candidates + lead - 1) / lead
        } else {
            candidates
        };
    }

    fn consider(&mut self, delay: i32) {
        let current_delay = (self.search.now0 - self.search.a0) as f64;
        let jump = (current_delay - f64::from(delay)).abs().max(1.0);
        let ncc = self.ncc_at(self.search.now0 - i64::from(delay));
        // The squared mismatch weights correlation more than jump length, so
        // a long jump at moderate correlation does not win.
        let mismatch = 1.0 - ncc;
        let score = -(mismatch * mismatch) / jump;
        if score > self.search.best_score {
            self.search.best_score = score;
            self.search.best_delay = delay;
            self.search.best_ncc = ncc;
        }
    }

    fn step_search(&mut self, mut count: i32) {
        while count > 0 && self.search.next <= self.search.hi {
            let next = self.search.next;
            self.consider(next);
            self.search.next += self.coarse_step;
            count -= 1;
        }
        if self.search.next > self.search.hi {
            if self.coarse_step > 1 {
                let center = self.search.best_delay;
                let start = self.search.lo.max(center - self.coarse_step + 1);
                let end = self.search.hi.min(center + self.coarse_step - 1);
                for delay in start..=end {
                    if delay != center {
                        self.consider(delay);
                    }
                }
            }
            self.search.result_jump =
                (self.search.now0 - i64::from(self.search.best_delay)) - self.search.a0;
            self.search.active = false;
            self.search.ready = true;
        }
    }

    fn start_fade(&mut self, jump: i64, fade_samples: i32) {
        self.search.ready = false;
        self.search.onset = false;
        let destination_delay = (self.written - 1) as f64 - (self.r_a + jump as f64);
        if destination_delay < 2.0 || destination_delay > self.rings[0].buffer.len() as f64 - 8.0 {
            return;
        }
        self.r_b = self.r_a + jump as f64;
        self.fading = true;
        self.fade = 0.0;
        self.fade_inc = 1.0 / f64::from(fade_samples);
        self.fade_ncc = self.search.best_ncc.clamp(0.0, 1.0);
    }

    /// Whether most of the recent power is below `ONSET_LOW_HZ`.
    fn low_note(&self) -> bool {
        self.low_power > ONSET_LOW_SHARE * self.full_power
    }

    fn detect_onset(&mut self, input: f32) -> bool {
        let now = self.written - 1;
        let input = f64::from(input);
        self.low_z = input + self.low_a * (self.low_z - input);
        self.low_power =
            self.power_a * self.low_power + (1.0 - self.power_a) * self.low_z * self.low_z;
        self.full_power = self.power_a * self.full_power + (1.0 - self.power_a) * input * input;
        let highpassed = input - self.hp_z1 + self.hp_a * self.hp_z2;
        self.hp_z1 = input;
        self.hp_z2 = highpassed;
        self.e_hf = self.smooth_a * self.e_hf + (1.0 - self.smooth_a) * highpassed * highpassed;
        let cell_now = now / i64::from(self.cell_len);
        if now % i64::from(self.cell_len) == 0 && now > 0 {
            let previous = (cell_now - 1) as usize % HISTORY_CELLS;
            self.min_hist[previous] = self.min_acc as f32;
            self.max_hist[previous] = self.max_acc as f32;
            self.min_acc = 1.0e9;
            self.max_acc = 0.0;
        }
        self.min_acc = self.min_acc.min(self.e_hf);
        self.max_acc = self.max_acc.max(self.e_hf);
        let mut recent_min = 1.0e9_f64;
        let mut recent_max = 0.0_f64;
        for back in HISTORY_SKIP_CELLS..HISTORY_CELLS {
            let index =
                (cell_now - back as i64 + 4 * HISTORY_CELLS as i64) as usize % HISTORY_CELLS;
            recent_min = recent_min.min(f64::from(self.min_hist[index]));
            recent_max = recent_max.max(f64::from(self.max_hist[index]));
        }
        self.e_hf > 1.0e-7
            && self.e_hf > recent_min * ONSET_OVER_MIN
            && self.e_hf > recent_max * ONSET_OVER_MAX
    }
}

fn direction(ratio: f64) -> i32 {
    if ratio > 1.0 {
        1
    } else if ratio < 1.0 {
        -1
    } else {
        0
    }
}
