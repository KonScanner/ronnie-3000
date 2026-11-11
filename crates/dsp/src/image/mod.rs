/// Corrective stereo-chain delay.
mod align;
/// Shared stereo-image primitives.
mod deck;
/// Mono-to-stereo doubling.
mod spread;

pub use align::{Align, AlignParams};
pub use deck::deck_crossover_hz;
pub use spread::{Spread, SpreadParams};

use crate::{ChainError, PlanarBuffer, block::gain::LinearGain};

/// Immutable controls for the post-chain image stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageControls {
    /// Whether two chains are active instead of one mono chain.
    pub stereo_chains: bool,
    /// Whether the output rig can reproduce stereo.
    pub stereo_rig: bool,
    /// Whether dual mono already supplies two independent voices.
    pub dual_mono: bool,
    /// Spread power and deck controls.
    pub spread_enabled: bool,
    /// Spread controls retained even when the rig disables processing.
    pub spread: SpreadParams,
    /// Align power and deck controls.
    pub align_enabled: bool,
    /// Align controls.
    pub align: AlignParams,
    /// Opposing chain balance from zero to one; 0.5 is centered.
    pub balance: f32,
    /// Left chain constant-power pan position.
    pub pan_left: f32,
    /// Right chain constant-power pan position.
    pub pan_right: f32,
    /// Solo the left chain.
    pub solo_left: bool,
    /// Solo the right chain.
    pub solo_right: bool,
    /// Invert the left chain.
    pub invert_left: bool,
    /// Invert the right chain.
    pub invert_right: bool,
    /// Capture pre-matrix chain outputs for automatic balance measurement.
    pub auto_balance_listening: bool,
}

impl Default for ImageControls {
    fn default() -> Self {
        Self {
            stereo_chains: true,
            stereo_rig: true,
            dual_mono: false,
            spread_enabled: false,
            spread: SpreadParams::default(),
            align_enabled: false,
            align: AlignParams::default(),
            balance: 0.5,
            pan_left: 0.0,
            pan_right: 1.0,
            solo_left: false,
            solo_right: false,
            invert_left: false,
            invert_right: false,
            auto_balance_listening: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ImageGains {
    left_to_left: f32,
    left_to_right: f32,
    right_to_left: f32,
    right_to_right: f32,
}

/// Spread/Align selection and the post-chain output matrix.
pub struct ImageStage {
    spread: Spread,
    align: Align,
    gains: [LinearGain; 4],
}

impl ImageStage {
    /// Prepares both image engines and their gain smoothers.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a nonfinite or nonpositive rate.
    pub fn new(sample_rate: f64, max_block_size: usize) -> Result<Self, ChainError> {
        Ok(Self {
            spread: Spread::new(sample_rate, max_block_size)?,
            align: Align::new(sample_rate, max_block_size)?,
            gains: [
                LinearGain::new(sample_rate, 0.020, 1.0),
                LinearGain::new(sample_rate, 0.020, 0.0),
                LinearGain::new(sample_rate, 0.020, 0.0),
                LinearGain::new(sample_rate, 0.020, 1.0),
            ],
        })
    }

    /// Whether Spread is active or finishing its power transition.
    #[must_use]
    pub const fn spread_is_running(&self) -> bool {
        self.spread.is_running()
    }

    /// Whether Align is active or finishing its identity transition.
    #[must_use]
    pub const fn align_is_running(&self) -> bool {
        self.align.is_running()
    }

    /// Stops both image engines and repairs any non-finite gain without allocating.
    pub(crate) fn reset(&mut self) {
        self.spread.force_idle();
        self.align.force_idle();
        for gain in &mut self.gains {
            if !gain.current().is_finite() {
                gain.set_current_and_target(0.0);
            }
        }
    }

    /// Applies image processing, the pre-matrix tap, and gain matrix without allocating.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error unless the buffer has two channels.
    pub fn process(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        controls: ImageControls,
        mut on_auto_balance: impl FnMut(&[f32], &[f32]),
    ) -> Result<(), ChainError> {
        if buffer.channels() != 2 {
            return Err(ChainError::InvalidLayout(
                "image processing requires two channels",
            ));
        }
        let spread_active = controls.spread_enabled && controls.stereo_rig && !controls.dual_mono;
        let mono_fold = controls.stereo_chains && !controls.stereo_rig;
        if controls.stereo_chains {
            self.spread.force_idle();
            self.align
                .set_target(controls.align, controls.align_enabled);
            if self.align.is_running() {
                self.align.process(buffer)?;
            }
        } else {
            self.align.force_idle();
            self.spread.set_target(controls.spread, spread_active);
            if self.spread.is_running() {
                self.spread.process(buffer)?;
            }
        }
        if controls.auto_balance_listening {
            on_auto_balance(buffer.channel(0)?, buffer.channel(1)?);
        }

        if controls.stereo_rig || mono_fold {
            let apply_balance = controls.stereo_chains || spread_active || controls.dual_mono;
            let gains = image_matrix_gains(
                controls.stereo_chains && !mono_fold,
                mono_fold,
                if apply_balance { controls.balance } else { 0.5 },
                controls,
            );
            self.gains[0].set_target(gains.left_to_left);
            self.gains[1].set_target(gains.left_to_right);
            self.gains[2].set_target(gains.right_to_left);
            self.gains[3].set_target(gains.right_to_right);
            let smoothing = self.gains.iter().any(LinearGain::is_smoothing);
            let identity = (gains.left_to_left - 1.0).abs() < 1.0e-4
                && (gains.right_to_right - 1.0).abs() < 1.0e-4
                && gains.left_to_right.abs() < 1.0e-4
                && gains.right_to_left.abs() < 1.0e-4;
            if smoothing || !identity {
                for frame in 0..buffer.frames() {
                    let ll = self.gains[0].next();
                    let lr = self.gains[1].next();
                    let rl = self.gains[2].next();
                    let rr = self.gains[3].next();
                    let left = buffer.channel(0)?[frame];
                    let right = buffer.channel(1)?[frame];
                    buffer.channel_mut(0)?[frame] = left * ll + right * rl;
                    buffer.channel_mut(1)?[frame] = left * lr + right * rr;
                }
            }
        }
        Ok(())
    }
}

fn image_matrix_gains(
    pan_active: bool,
    fold_to_mono: bool,
    balance: f32,
    controls: ImageControls,
) -> ImageGains {
    let mut balance_left = if controls.solo_right && !controls.solo_left {
        0.0
    } else {
        balance_chain_gain(balance, 0)
    };
    let mut balance_right = if controls.solo_left && !controls.solo_right {
        0.0
    } else {
        balance_chain_gain(balance, 1)
    };
    if controls.invert_left {
        balance_left = -balance_left;
    }
    if controls.invert_right {
        balance_right = -balance_right;
    }
    if fold_to_mono {
        return ImageGains {
            left_to_left: 0.5 * balance_left,
            left_to_right: 0.5 * balance_left,
            right_to_left: 0.5 * balance_right,
            right_to_right: 0.5 * balance_right,
        };
    }
    let (left_to_left, left_to_right) = if pan_active {
        constant_power_pan(controls.pan_left)
    } else {
        (1.0, 0.0)
    };
    let (right_to_left, right_to_right) = if pan_active {
        constant_power_pan(controls.pan_right)
    } else {
        (0.0, 1.0)
    };
    ImageGains {
        left_to_left: balance_left * left_to_left,
        left_to_right: balance_left * left_to_right,
        right_to_left: balance_right * right_to_left,
        right_to_right: balance_right * right_to_right,
    }
}

/// Linear left and right chain gains for one normalized balance position.
#[must_use]
pub fn chain_balance_gains(balance: f32) -> (f32, f32) {
    (
        balance_chain_gain(balance, 0),
        balance_chain_gain(balance, 1),
    )
}

fn balance_chain_gain(balance: f32, chain: usize) -> f32 {
    let trim_db = (balance - 0.5) * 24.0 * if chain == 0 { -1.0 } else { 1.0 };
    10.0_f32.powf(trim_db / 20.0)
}

fn constant_power_pan(pan: f32) -> (f32, f32) {
    let angle = pan.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}
