//! The OPTIMIZE switch: the pitch settings with the lowest safe delay for the instrument and the
//! shift.
//!
//! The switch is a machine setting. With no stored value it is on. The UI and the audio engine
//! read it with [`optimize_enabled`], so that both use one default.
//!
//! [`optimized_pitch`] is the policy. It is pure data, so that one table sets the pitch window
//! of each instrument.

use crate::tuning::{Instrument, WHOLE_LIMIT, sounding_shift};

/// The machine setting that holds the OPTIMIZE switch.
pub const OPTIMIZE_PREFERENCE: &str = "t3k.optimizeSettings";

/// The choices of the `pitchWindow` parameter in milliseconds, in the order of their index.
pub const PITCH_WINDOWS_MS: [u16; 4] = [20, 30, 40, 60];

/// Whether OPTIMIZE is on for a stored value. With no stored value, it is on.
#[must_use]
pub const fn optimize_enabled(stored: Option<bool>) -> bool {
    match stored {
        Some(enabled) => enabled,
        None => true,
    }
}

/// The pitch window of each instrument, in milliseconds.
///
/// This table is the one place to tune the policy. A shorter window has less delay and more
/// warble on low notes. Low delay comes first, so a guitar uses the shortest window. The low
/// notes of a bass need a longer window.
const fn window_ms(instrument: Instrument) -> u16 {
    match instrument {
        Instrument::Guitar6 => 20,
        Instrument::Guitar7 => 20,
        Instrument::Bass4 => 40,
        Instrument::Bass5 => 40,
    }
}

/// The pitch settings that OPTIMIZE keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PitchPlan {
    /// The pitch window in milliseconds, one of [`PITCH_WINDOWS_MS`].
    pub window_ms: u16,
    /// Whether the pitch shift must stay on. A shift that sounds as no shift needs no pitch
    /// shift, and so no delay.
    pub enabled: bool,
}

impl PitchPlan {
    /// The index of the window in the choices of the `pitchWindow` parameter.
    #[must_use]
    pub fn window_index(self) -> usize {
        PITCH_WINDOWS_MS
            .iter()
            .position(|&ms| ms == self.window_ms)
            .unwrap_or(0)
    }
}

/// The pitch settings with the lowest safe delay for an instrument and a shift.
///
/// `semitones` is the value of the pitch knob, and `step` is the STEP switch. The shift that
/// sounds is the shift after the STEP rounding. A shift that the knob shows as "0 st" sounds as
/// no shift.
#[must_use]
pub fn optimized_pitch(instrument: Instrument, semitones: f32, step: bool) -> PitchPlan {
    let shift = sounding_shift(semitones, step);
    PitchPlan {
        window_ms: window_ms(instrument),
        enabled: shift.is_finite() && shift.abs() >= WHOLE_LIMIT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ParameterCatalog, ParameterId};

    #[test]
    fn optimize_is_on_with_no_stored_value() {
        assert!(optimize_enabled(None));
        assert!(optimize_enabled(Some(true)));
        assert!(!optimize_enabled(Some(false)));
    }

    #[test]
    fn a_guitar_uses_20_ms_and_a_bass_uses_40_ms() {
        for (instrument, window_ms, index) in [
            (Instrument::Guitar6, 20, 0),
            (Instrument::Guitar7, 20, 0),
            (Instrument::Bass4, 40, 2),
            (Instrument::Bass5, 40, 2),
        ] {
            let plan = optimized_pitch(instrument, -2.0, true);
            assert_eq!(
                plan,
                PitchPlan {
                    window_ms,
                    enabled: true
                },
                "{instrument:?}"
            );
            assert_eq!(plan.window_index(), index, "{instrument:?}");
        }
    }

    #[test]
    fn a_shift_that_sounds_as_no_shift_turns_the_pitch_shift_off() {
        for instrument in Instrument::ALL {
            assert!(
                !optimized_pitch(instrument, 0.0, true).enabled,
                "{instrument:?}"
            );
            assert!(
                !optimized_pitch(instrument, 0.0, false).enabled,
                "{instrument:?}"
            );
            // STEP rounds a small shift to no shift.
            assert!(
                !optimized_pitch(instrument, 0.4, true).enabled,
                "{instrument:?}"
            );
            assert!(
                !optimized_pitch(instrument, -0.04, false).enabled,
                "{instrument:?}"
            );
            assert!(
                !optimized_pitch(instrument, f32::NAN, false).enabled,
                "{instrument:?}"
            );
            // Without STEP, the small shift sounds.
            assert!(
                optimized_pitch(instrument, 0.4, false).enabled,
                "{instrument:?}"
            );
            assert!(
                optimized_pitch(instrument, 0.6, true).enabled,
                "{instrument:?}"
            );
            // The window does not depend on the shift.
            assert_eq!(
                optimized_pitch(instrument, 0.0, true).window_ms,
                optimized_pitch(instrument, -5.0, true).window_ms
            );
        }
    }

    #[test]
    fn each_window_of_the_policy_is_a_choice_of_the_pitch_window_parameter() {
        let spec = ParameterCatalog::v1()
            .iter()
            .find(|spec| spec.id == ParameterId("pitchWindow"))
            .expect("pitchWindow");
        let choices = PITCH_WINDOWS_MS.map(|ms| format!("{ms} ms"));
        assert_eq!(spec.choices, choices.each_ref().map(String::as_str));
        for instrument in Instrument::ALL {
            assert!(
                PITCH_WINDOWS_MS.contains(&window_ms(instrument)),
                "{instrument:?}"
            );
        }
    }
}
