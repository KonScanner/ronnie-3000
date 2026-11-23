//! Named tunings for the whole-signal pitch shift.
//!
//! The input pitch shift moves the whole signal, so one shift moves each string by the same
//! number of semitones. A [`Tuning`] is one such shift with the name of the standard tuning that
//! it sounds as on one [`Instrument`], for example "D Standard" for two semitones down on a
//! six-string guitar.
//!
//! Each text here is pure data, so that the faceplate caption, the picker, the string strip, and
//! the tuner use one rule.

// TODO: Drop tunings and open tunings are not in this catalog. They shift one string, or each
// string by a different amount, so they need one signal for each string: a hexaphonic pickup,
// and more than the one or two input channels that the app accepts now. A mono-signal approach
// that shifts only the notes that only the low string can play (frets 0 to 4) is an experiment,
// and it has artifacts.

/// The names of the twelve notes from C, with the accidentals that the tuning names use.
pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B",
];

/// The largest downward shift of a listed tuning, in semitones.
pub const LOWEST_SHIFT: i8 = -5;

/// How near to a whole semitone a shift must be to read as that semitone.
///
/// The knob text uses the same limit, so a shift that the knob shows as "-2 st" is also the
/// listed tuning of two semitones down.
pub(crate) const WHOLE_LIMIT: f32 = 0.05;

/// An instrument with a fixed set of open strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Instrument {
    /// A six-string guitar: E A D G B e.
    #[default]
    Guitar6,
    /// A seven-string guitar: B E A D G B e.
    Guitar7,
    /// A four-string bass: E A D G.
    Bass4,
    /// A five-string bass: B E A D G.
    Bass5,
}

impl Instrument {
    /// Each instrument, in the order of the picker.
    pub const ALL: [Self; 4] = [Self::Guitar6, Self::Guitar7, Self::Bass4, Self::Bass5];

    /// The full name, for example "Guitar 6-string".
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Guitar6 => "Guitar 6-string",
            Self::Guitar7 => "Guitar 7-string",
            Self::Bass4 => "Bass 4-string",
            Self::Bass5 => "Bass 5-string",
        }
    }

    /// The short name, for example "Guitar 6".
    #[must_use]
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Guitar6 => "Guitar 6",
            Self::Guitar7 => "Guitar 7",
            Self::Bass4 => "Bass 4",
            Self::Bass5 => "Bass 5",
        }
    }

    /// The stable key that a settings file stores.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Guitar6 => "guitar6",
            Self::Guitar7 => "guitar7",
            Self::Bass4 => "bass4",
            Self::Bass5 => "bass5",
        }
    }

    /// The instrument of a stored key, or `None` for a key that no instrument has.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|instrument| instrument.key() == key.trim())
    }

    /// The MIDI notes of the open strings, from the lowest string to the highest.
    #[must_use]
    pub const fn open_strings(self) -> &'static [u8] {
        match self {
            Self::Guitar6 => &[40, 45, 50, 55, 59, 64],
            Self::Guitar7 => &[35, 40, 45, 50, 55, 59, 64],
            Self::Bass4 => &[28, 33, 38, 43],
            Self::Bass5 => &[23, 28, 33, 38, 43],
        }
    }

    /// Whether the instrument is a guitar. A guitar writes its highest string in lower case.
    #[must_use]
    pub const fn is_guitar(self) -> bool {
        matches!(self, Self::Guitar6 | Self::Guitar7)
    }

    /// The listed tunings, from no shift down to [`LOWEST_SHIFT`].
    pub fn tunings(self) -> impl Iterator<Item = Tuning> {
        (LOWEST_SHIFT..=0).rev().map(move |shift| Tuning {
            instrument: self,
            shift,
        })
    }

    /// The listed tuning of a shift in semitones, or `None` for a custom shift.
    ///
    /// A custom shift is not a whole semitone, or it is outside the list.
    #[must_use]
    pub fn tuning(self, semitones: f32) -> Option<Tuning> {
        let whole = whole_semitones(semitones)?;
        self.tunings()
            .find(|tuning| i32::from(tuning.shift) == whole)
    }

    /// The text under the pitch knob for a shift, or `None` for no shift.
    ///
    /// A listed tuning gives its short name, for example "D Std". Another shift gives its value
    /// in the text of the knob, for example "-7 st" or "+1.5 st".
    #[must_use]
    pub fn caption(self, semitones: f32) -> Option<String> {
        if !semitones.is_finite() || f64::from(semitones).abs() < f64::from(WHOLE_LIMIT) {
            return None;
        }
        Some(self.tuning(semitones).map_or_else(
            || format_semitones(f64::from(semitones)),
            Tuning::short_name,
        ))
    }

    /// The open notes and the shifted notes of each string, from the lowest string.
    ///
    /// A shift that is not a whole semitone uses the nearest whole semitone.
    #[must_use]
    pub fn strings(self, semitones: f32) -> Vec<StringShift> {
        let shift = if semitones.is_finite() {
            semitones.round() as i32
        } else {
            0
        };
        let count = self.open_strings().len();
        self.open_strings()
            .iter()
            .enumerate()
            .map(|(index, &open)| {
                let lower = self.is_guitar() && index + 1 == count;
                let name = |midi: i32| {
                    let name = note_name(midi);
                    if lower {
                        name.to_lowercase()
                    } else {
                        name.to_owned()
                    }
                };
                StringShift {
                    open: name(i32::from(open)),
                    shifted: name(i32::from(open) + shift),
                }
            })
            .collect()
    }

    /// The string strip for a shift, for example "E→D  A→G  D→C  G→F  B→A  e→d".
    ///
    /// With no shift, the strip names the open strings only, for example "E  A  D  G  B  e".
    /// A shift that is not a whole semitone starts with "≈", because the strip then names the
    /// nearest notes.
    #[must_use]
    pub fn string_strip(self, semitones: f32) -> String {
        let strings = self.strings(semitones);
        let no_shift = strings.iter().all(|string| string.open == string.shifted);
        let strip = strings
            .iter()
            .map(|string| {
                if no_shift {
                    string.open.clone()
                } else {
                    format!("{}→{}", string.open, string.shifted)
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        if whole_semitones(semitones).is_none() && semitones.is_finite() {
            format!("≈ {strip}")
        } else {
            strip
        }
    }
}

/// One listed tuning: a whole-semitone shift on one instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tuning {
    instrument: Instrument,
    shift: i8,
}

impl Tuning {
    /// The instrument of this tuning.
    #[must_use]
    pub const fn instrument(self) -> Instrument {
        self.instrument
    }

    /// The shift in whole semitones, from 0 down to [`LOWEST_SHIFT`].
    #[must_use]
    pub const fn shift(self) -> i8 {
        self.shift
    }

    /// The shift as a value of the pitch parameter.
    #[must_use]
    pub fn semitones(self) -> f32 {
        f32::from(self.shift)
    }

    /// The note of the shifted lowest string, for example "D".
    fn root(self) -> &'static str {
        let lowest = self.instrument.open_strings().first().copied().unwrap_or(0);
        note_name(i32::from(lowest) + i32::from(self.shift))
    }

    /// The name, for example "D Standard".
    #[must_use]
    pub fn name(self) -> String {
        format!("{} Standard", self.root())
    }

    /// The short name for the knob caption, for example "D Std".
    #[must_use]
    pub fn short_name(self) -> String {
        format!("{} Std", self.root())
    }

    /// The text of the picker, for example "D Standard (-2)". No shift has no number.
    #[must_use]
    pub fn label(self) -> String {
        if self.shift == 0 {
            self.name()
        } else {
            format!("{} ({})", self.name(), self.shift)
        }
    }
}

/// The open note and the shifted note of one string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringShift {
    /// The name of the open note, for example "E".
    pub open: String,
    /// The name of the note after the shift, for example "D".
    pub shifted: String,
}

/// The name of a MIDI note, with no octave.
#[must_use]
pub fn note_name(midi: i32) -> &'static str {
    NOTE_NAMES[midi.rem_euclid(12) as usize]
}

/// The shift that sounds: the engine rounds the shift to whole semitones while STEP is on.
#[must_use]
pub fn sounding_shift(semitones: f32, step: bool) -> f32 {
    if step { semitones.round() } else { semitones }
}

/// The shift in whole semitones, or `None` when it is not near a whole semitone.
fn whole_semitones(semitones: f32) -> Option<i32> {
    if !semitones.is_finite() {
        return None;
    }
    let whole = semitones.round();
    ((semitones - whole).abs() < WHOLE_LIMIT).then_some(whole as i32)
}

/// The text of a shift in semitones, for example "0 st", "-7 st", or "+1.5 st".
///
/// The pitch knob shows its value with this text.
#[must_use]
pub fn format_semitones(semitones: f64) -> String {
    if semitones.abs() < f64::from(WHOLE_LIMIT) {
        "0 st".to_owned()
    } else if (semitones - semitones.round()).abs() < f64::from(WHOLE_LIMIT) {
        format!("{:+} st", semitones.round() as i32)
    } else {
        format!("{semitones:+.1} st")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(instrument: Instrument) -> Vec<String> {
        instrument.tunings().map(Tuning::name).collect()
    }

    #[test]
    fn each_instrument_has_its_open_strings_from_the_lowest() {
        assert_eq!(
            Instrument::Guitar6.open_strings(),
            &[40, 45, 50, 55, 59, 64]
        );
        assert_eq!(
            Instrument::Guitar7.open_strings(),
            &[35, 40, 45, 50, 55, 59, 64]
        );
        assert_eq!(Instrument::Bass4.open_strings(), &[28, 33, 38, 43]);
        assert_eq!(Instrument::Bass5.open_strings(), &[23, 28, 33, 38, 43]);
    }

    #[test]
    fn each_instrument_lists_six_tunings_named_by_the_shifted_lowest_string() {
        let e = ["E", "Eb", "D", "C#", "C", "B"].map(|root| format!("{root} Standard"));
        let b = ["B", "Bb", "A", "G#", "G", "F#"].map(|root| format!("{root} Standard"));
        assert_eq!(names(Instrument::Guitar6), e);
        assert_eq!(names(Instrument::Bass4), e);
        assert_eq!(names(Instrument::Guitar7), b);
        assert_eq!(names(Instrument::Bass5), b);
        for instrument in Instrument::ALL {
            let shifts = instrument.tunings().map(Tuning::shift).collect::<Vec<_>>();
            assert_eq!(shifts, [0, -1, -2, -3, -4, -5], "{instrument:?}");
        }
    }

    #[test]
    fn a_tuning_has_a_short_name_and_a_picker_label() {
        let d = Instrument::Guitar6.tuning(-2.0).expect("D Standard");
        assert_eq!(d.name(), "D Standard");
        assert_eq!(d.short_name(), "D Std");
        assert_eq!(d.label(), "D Standard (-2)");
        assert_eq!(d.semitones(), -2.0);
        assert_eq!(d.instrument(), Instrument::Guitar6);
        let e = Instrument::Guitar6.tuning(0.0).expect("E Standard");
        assert_eq!(e.label(), "E Standard");
        let f_sharp = Instrument::Bass5.tuning(-5.0).expect("F# Standard");
        assert_eq!(f_sharp.short_name(), "F# Std");
        assert_eq!(f_sharp.label(), "F# Standard (-5)");
    }

    #[test]
    fn a_shift_outside_the_list_or_between_semitones_is_custom() {
        for semitones in [-7.0, 1.0, -1.5, -5.5, 24.0, f32::NAN] {
            assert_eq!(Instrument::Guitar6.tuning(semitones), None, "{semitones}");
        }
        assert_eq!(
            Instrument::Guitar6.tuning(-2.02).map(Tuning::shift),
            Some(-2),
            "the knob shows -2.02 as -2 st"
        );
    }

    #[test]
    fn the_caption_names_a_listed_tuning_and_shows_the_value_of_another_shift() {
        assert_eq!(Instrument::Guitar6.caption(-2.0).as_deref(), Some("D Std"));
        assert_eq!(Instrument::Guitar7.caption(-2.0).as_deref(), Some("A Std"));
        assert_eq!(Instrument::Bass4.caption(-1.0).as_deref(), Some("Eb Std"));
        assert_eq!(Instrument::Bass5.caption(-3.0).as_deref(), Some("G# Std"));
        assert_eq!(Instrument::Guitar6.caption(-7.0).as_deref(), Some("-7 st"));
        assert_eq!(Instrument::Guitar6.caption(1.5).as_deref(), Some("+1.5 st"));
        assert_eq!(Instrument::Guitar6.caption(0.0), None);
        assert_eq!(Instrument::Guitar6.caption(0.01), None);
    }

    #[test]
    fn the_string_strip_shifts_each_string_and_writes_the_top_guitar_string_in_lower_case() {
        assert_eq!(
            Instrument::Guitar6.string_strip(-2.0),
            "E→D  A→G  D→C  G→F  B→A  e→d"
        );
        assert_eq!(
            Instrument::Guitar7.string_strip(-1.0),
            "B→Bb  E→Eb  A→G#  D→C#  G→F#  B→Bb  e→eb"
        );
        assert_eq!(Instrument::Bass4.string_strip(-2.0), "E→D  A→G  D→C  G→F");
        assert_eq!(
            Instrument::Bass5.string_strip(-2.0),
            "B→A  E→D  A→G  D→C  G→F"
        );
        assert_eq!(Instrument::Guitar6.string_strip(0.0), "E  A  D  G  B  e");
        assert_eq!(Instrument::Bass4.string_strip(-1.5), "≈ E→D  A→G  D→C  G→F");
    }

    #[test]
    fn each_string_names_its_open_note_and_its_shifted_note() {
        let strings = Instrument::Guitar6.strings(-7.0);
        assert_eq!(strings.len(), 6);
        assert_eq!(
            strings[0],
            StringShift {
                open: "E".to_owned(),
                shifted: "A".to_owned(),
            }
        );
        assert_eq!(strings[5].open, "e");
        assert_eq!(strings[5].shifted, "a");
    }

    #[test]
    fn an_instrument_keeps_its_key() {
        for instrument in Instrument::ALL {
            assert_eq!(Instrument::from_key(instrument.key()), Some(instrument));
        }
        assert_eq!(Instrument::from_key("banjo"), None);
        assert_eq!(Instrument::default(), Instrument::Guitar6);
        assert_eq!(Instrument::Bass5.short_name(), "Bass 5");
        assert_eq!(Instrument::Guitar7.name(), "Guitar 7-string");
    }

    #[test]
    fn note_names_wrap_at_the_octave() {
        assert_eq!(note_name(60), "C");
        assert_eq!(note_name(63), "Eb");
        assert_eq!(note_name(-1), "B");
    }

    #[test]
    fn the_sounding_shift_is_whole_while_step_is_on() {
        assert_eq!(sounding_shift(-2.4, true), -2.0);
        assert_eq!(sounding_shift(-2.4, false), -2.4);
    }

    #[test]
    fn the_semitone_text_is_the_text_of_the_pitch_knob() {
        assert_eq!(format_semitones(0.0), "0 st");
        assert_eq!(format_semitones(-0.04), "0 st");
        assert_eq!(format_semitones(3.0), "+3 st");
        assert_eq!(format_semitones(-7.0), "-7 st");
        assert_eq!(format_semitones(1.5), "+1.5 st");
        assert_eq!(format_semitones(-1.46), "-1.5 st");
    }
}
