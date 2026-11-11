//! The tuning picker of the pitch shift: the instrument choice, the knob caption, and the edits
//! of one picked tuning.
//!
//! The names come from [`ronie_core::tuning`], so the picker, the caption, and the tuner use one
//! catalog.

use ronie_core::{
    Instrument, Tuning,
    optimize::{OPTIMIZE_PREFERENCE, optimize_enabled, optimized_pitch},
    params::{ParameterId, ParameterValue},
    tuning::{note_name, sounding_shift},
};

use crate::backend::{GesturePhase, UiAction, UiBackend, UiError};

/// The machine setting that holds the instrument of the tuning picker.
pub const INSTRUMENT_PREFERENCE: &str = "pitchInstrument";

/// The text under the pitch knob when no tuning or shift applies.
pub const PITCH_LABEL: &str = "Pitch";

/// The instrument that the player picked on this machine, or a six-string guitar.
#[must_use]
pub fn instrument(backend: &dyn UiBackend) -> Instrument {
    backend
        .text_preference(INSTRUMENT_PREFERENCE)
        .and_then(|key| Instrument::from_key(&key))
        .unwrap_or_default()
}

/// Stores the instrument of the tuning picker as a machine setting.
///
/// While OPTIMIZE is on, the pitch settings then change to the policy of the new instrument,
/// as one undo step.
///
/// # Errors
/// Returns the error of the backend.
pub fn select_instrument(backend: &dyn UiBackend, instrument: Instrument) -> Result<(), UiError> {
    backend.dispatch(UiAction::SetTextPreference {
        key: INSTRUMENT_PREFERENCE.to_owned(),
        value: instrument.key().to_owned(),
    })?;
    apply_optimize(backend)
}

/// Whether OPTIMIZE is on. With no stored value, it is on.
#[must_use]
pub fn optimize(backend: &dyn UiBackend) -> bool {
    optimize_enabled(backend.preference(OPTIMIZE_PREFERENCE))
}

/// Stores the OPTIMIZE switch as a machine setting.
///
/// When OPTIMIZE comes on, the pitch settings change to the policy at once, as one undo step.
///
/// # Errors
/// Returns the error of the backend.
pub fn set_optimize(backend: &dyn UiBackend, enabled: bool) -> Result<(), UiError> {
    backend.dispatch(UiAction::SetPreference {
        key: OPTIMIZE_PREFERENCE.to_owned(),
        value: enabled,
    })?;
    apply_optimize(backend)
}

/// Changes the pitch window and the pitch power to the OPTIMIZE policy, as one undo step.
///
/// The owner of a control calls this after a change that the policy reads: a pick of an
/// instrument, the end of a gesture of the pitch knob, or a click of STEP. This does nothing
/// while OPTIMIZE is off, or when the settings agree with the policy.
///
/// # Errors
/// Returns the error of the backend. A refused write closes the gesture.
pub fn apply_optimize(backend: &dyn UiBackend) -> Result<(), UiError> {
    if !optimize(backend) {
        return Ok(());
    }
    let writes = optimized_writes(
        backend,
        instrument(backend),
        value(backend, "pitchSemitones"),
        value(backend, "pitchStep") > 0.5,
        value(backend, "pitchEnabled") > 0.5,
    );
    let Some(&(first, _)) = writes.first() else {
        return Ok(());
    };
    let id = ParameterId(first);
    backend.set_parameter(
        id,
        ParameterValue(value(backend, first)),
        GesturePhase::Begin,
    )?;
    let written = writes.into_iter().try_for_each(|(id, next)| {
        backend.set_parameter(ParameterId(id), ParameterValue(next), GesturePhase::Change)
    });
    let end = backend.set_parameter(id, ParameterValue(value(backend, first)), GesturePhase::End);
    written.and(end)
}

/// The writes that change the pitch settings to the OPTIMIZE policy.
///
/// `enabled` is the pitch power after the change. The policy can turn the pitch shift off for a
/// shift that sounds as no shift. It never turns the pitch shift on, because the player owns the
/// pitch power.
fn optimized_writes(
    backend: &dyn UiBackend,
    instrument: Instrument,
    semitones: f32,
    step: bool,
    enabled: bool,
) -> Vec<(&'static str, f32)> {
    let plan = optimized_pitch(instrument, semitones, step);
    let window = plan.window_index() as f32;
    let mut writes = Vec::new();
    if value(backend, "pitchWindow") != window {
        writes.push(("pitchWindow", window));
    }
    if enabled && !plan.enabled {
        writes.push(("pitchEnabled", 0.0));
    }
    writes
}

fn value(backend: &dyn UiBackend, id: &'static str) -> f32 {
    backend
        .parameter(ParameterId(id))
        .map_or(0.0, |value| value.0)
}

/// The shift that the player hears now, in semitones: 0 while the pitch shift is off.
///
/// While STEP is on, the engine rounds the shift to whole semitones, and so does this.
#[must_use]
pub fn shift(backend: &dyn UiBackend) -> f32 {
    if value(backend, "pitchEnabled") <= 0.5 {
        return 0.0;
    }
    let semitones = value(backend, "pitchSemitones");
    let shift = sounding_shift(semitones, value(backend, "pitchStep") > 0.5);
    if shift.is_finite() { shift } else { 0.0 }
}

/// The text under the pitch knob: the short name of a listed tuning, the value of another
/// shift, or "Pitch" while the pitch shift is off or does not shift.
#[must_use]
pub fn caption(backend: &dyn UiBackend) -> String {
    instrument(backend)
        .caption(shift(backend))
        .unwrap_or_else(|| PITCH_LABEL.to_owned())
}

/// The listed tuning that sounds now, or `None` for a custom shift.
#[must_use]
pub fn current_tuning(backend: &dyn UiBackend) -> Option<Tuning> {
    instrument(backend).tuning(shift(backend))
}

/// Sets the shift of a tuning, and turns STEP and the pitch shift on, as one undo step.
///
/// The tuning with no shift sets the shift to 0 and turns the pitch shift off, so that the
/// shift adds no delay. While OPTIMIZE is on, the pitch window changes to the policy of the
/// instrument in the same step. The edits go through the parameter path of the knobs, so undo,
/// presets, and MIDI see them. One gesture holds each write, and so one undo takes the whole
/// pick back, as a linked pan does.
///
/// # Errors
/// Returns the error of the backend. A refused write closes the gesture.
pub fn select_tuning(backend: &dyn UiBackend, tuning: Tuning) -> Result<(), UiError> {
    let semitones = ParameterId("pitchSemitones");
    let current = ParameterValue(value(backend, "pitchSemitones"));
    backend.set_parameter(semitones, current, GesturePhase::Begin)?;
    let shifts = tuning.shift() != 0;
    let mut writes = if shifts {
        vec![
            ("pitchSemitones", tuning.semitones()),
            ("pitchStep", 1.0),
            ("pitchEnabled", 1.0),
        ]
    } else {
        vec![("pitchSemitones", 0.0), ("pitchEnabled", 0.0)]
    };
    if optimize(backend) {
        let step = shifts || value(backend, "pitchStep") > 0.5;
        writes.extend(optimized_writes(
            backend,
            tuning.instrument(),
            tuning.semitones(),
            step,
            shifts,
        ));
    }
    let written = writes.into_iter().try_for_each(|(id, next)| {
        backend.set_parameter(ParameterId(id), ParameterValue(next), GesturePhase::Change)
    });
    let end = backend.set_parameter(
        semitones,
        ParameterValue(value(backend, "pitchSemitones")),
        GesturePhase::End,
    );
    written.and(end)
}

/// The note that a detected frequency sounds as after a shift, for example "D", or `None` for
/// no signal or no shift.
///
/// The tuner reads the signal before the shift, so its big note stays the real string. This is
/// the nearest note after the shift.
#[must_use]
pub fn sounds_as(frequency_hz: f64, shift: f32) -> Option<&'static str> {
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 || !shift.is_finite() || shift == 0.0 {
        return None;
    }
    let midi = 69.0 + 12.0 * (frequency_hz / 440.0).log2() + f64::from(shift);
    Some(note_name(midi.round() as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shift_moves_the_detected_note_to_the_nearest_note() {
        // The low E string of a guitar, 82.41 Hz.
        assert_eq!(sounds_as(82.41, -2.0), Some("D"));
        assert_eq!(sounds_as(82.41, -1.0), Some("Eb"));
        assert_eq!(sounds_as(110.0, 1.4), Some("Bb"));
        assert_eq!(sounds_as(82.41, 0.0), None);
        assert_eq!(sounds_as(0.0, -2.0), None);
    }
}
