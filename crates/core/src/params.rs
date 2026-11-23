//! Stable host parameter identities, ranges, mappings, and persistence scopes.

/// Stable string identity used by hosts, automation, MIDI, and persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParameterId(pub &'static str);

/// Real-unit parameter value in the range declared by its specification.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ParameterValue(pub f32);

/// Conversion between a normalized host value and its real-unit meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapping {
    /// Linear interpolation over the declared range.
    Linear,
    /// Logarithmic interpolation over a positive real-unit range.
    Logarithmic,
}

/// Audio-side ramp attached to a parameter change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Smoothing {
    /// Apply the value immediately.
    Immediate,
    /// Glide to the new value over this many milliseconds.
    Milliseconds(u16),
}

/// Persistence and machine-default ownership for one parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterScope {
    /// Stored with the host session and the standalone session.
    pub session: bool,
    /// Stored with a user preset.
    pub preset: bool,
    /// Seeds new instances from machine-wide settings.
    pub machine_default: bool,
}

/// Complete host-facing contract for one parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterSpec {
    /// Stable ID.
    pub id: ParameterId,
    /// Stable AU version hint.
    pub version_hint: u16,
    /// Inclusive minimum real-unit value.
    pub min: f32,
    /// Inclusive maximum real-unit value.
    pub max: f32,
    /// Default real-unit value.
    pub default: f32,
    /// Optional real-unit quantization interval.
    pub interval: Option<f32>,
    /// Real-unit mapping.
    pub mapping: Mapping,
    /// Audio-side change ramp.
    pub smoothing: Smoothing,
    /// Whether host automation is supported.
    pub automatable: bool,
    /// Persistence ownership.
    pub scope: ParameterScope,
    /// Ordered choices for a choice parameter, empty for continuous and Boolean parameters.
    pub choices: &'static [&'static str],
}

impl ParameterSpec {
    /// Whether the logarithmic rule applies. That rule needs a positive minimum, because it gives
    /// NaN for a range from zero, so such a range uses the linear rule.
    fn logarithmic(&self) -> bool {
        self.mapping == Mapping::Logarithmic && self.min > 0.0 && self.max > self.min
    }

    /// The real-unit value at a normalized `position` from 0 to 1.
    ///
    /// A position outside 0 to 1 is clamped, and NaN is 0. The mapping is logarithmic only for a
    /// logarithmic parameter with a positive minimum, and linear for every other parameter.
    #[must_use]
    pub fn value_at(&self, position: f32) -> f32 {
        let position = if position.is_nan() {
            0.0
        } else {
            position.clamp(0.0, 1.0)
        };
        if self.logarithmic() {
            self.min * (self.max / self.min).powf(position)
        } else {
            self.min + position * (self.max - self.min)
        }
    }

    /// The normalized position from 0 to 1 of a real-unit `value`. This is the inverse of
    /// [`Self::value_at`].
    ///
    /// A value outside the range gives the nearest end, and a value with no position gives 0.
    #[must_use]
    pub fn position_of(&self, value: f32) -> f32 {
        let position = if self.logarithmic() {
            (value / self.min).ln() / (self.max / self.min).ln()
        } else if self.max > self.min {
            (value - self.min) / (self.max - self.min)
        } else {
            0.0
        };
        if position.is_finite() {
            position.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// Versioned parameter catalog.
pub struct ParameterCatalog;

impl ParameterCatalog {
    /// Returns all 43 Reference v1 parameters in stable AU-hint order.
    #[must_use]
    pub const fn v1() -> &'static [ParameterSpec] {
        PARAMETERS_V1
    }
}

const fn scope(preset: bool, machine_default: bool) -> ParameterScope {
    ParameterScope {
        session: true,
        preset,
        machine_default,
    }
}

const fn norm(
    id: &'static str,
    version_hint: u16,
    default: f32,
    smoothing: Smoothing,
    preset: bool,
) -> ParameterSpec {
    ParameterSpec {
        id: ParameterId(id),
        version_hint,
        min: 0.0,
        max: 1.0,
        default,
        interval: Some(0.0001),
        mapping: Mapping::Linear,
        smoothing,
        automatable: true,
        scope: scope(preset, false),
        choices: &[],
    }
}

const fn boolean(
    id: &'static str,
    version_hint: u16,
    default: bool,
    smoothing: Smoothing,
    preset: bool,
) -> ParameterSpec {
    ParameterSpec {
        id: ParameterId(id),
        version_hint,
        min: 0.0,
        max: 1.0,
        default: if default { 1.0 } else { 0.0 },
        interval: Some(1.0),
        mapping: Mapping::Linear,
        smoothing,
        automatable: true,
        scope: scope(preset, false),
        choices: &[],
    }
}

const fn machine_boolean(id: &'static str, version_hint: u16, default: bool) -> ParameterSpec {
    ParameterSpec {
        automatable: false,
        scope: scope(false, true),
        ..boolean(id, version_hint, default, Smoothing::Immediate, false)
    }
}

const fn float(
    id: &'static str,
    version_hint: u16,
    min: f32,
    max: f32,
    default: f32,
    mapping: Mapping,
    preset: bool,
) -> ParameterSpec {
    ParameterSpec {
        id: ParameterId(id),
        version_hint,
        min,
        max,
        default,
        interval: None,
        mapping,
        smoothing: Smoothing::Immediate,
        automatable: true,
        scope: scope(preset, false),
        choices: &[],
    }
}

const fn machine_float(
    id: &'static str,
    version_hint: u16,
    min: f32,
    max: f32,
    default: f32,
) -> ParameterSpec {
    ParameterSpec {
        scope: scope(false, true),
        ..float(id, version_hint, min, max, default, Mapping::Linear, false)
    }
}

const fn choice(
    id: &'static str,
    version_hint: u16,
    choices: &'static [&'static str],
    default: f32,
    automatable: bool,
    preset: bool,
    machine_default: bool,
) -> ParameterSpec {
    ParameterSpec {
        id: ParameterId(id),
        version_hint,
        min: 0.0,
        max: choices.len() as f32 - 1.0,
        default,
        interval: Some(1.0),
        mapping: Mapping::Linear,
        smoothing: Smoothing::Immediate,
        automatable,
        scope: scope(preset, machine_default),
        choices,
    }
}

const OS_CHOICES: &[&str] = &["2x", "4x", "8x"];
const WINDOW_CHOICES: &[&str] = &["20 ms", "30 ms", "40 ms", "60 ms"];

const PARAMETERS_V1: &[ParameterSpec] = &[
    norm("inputLevel", 1, 0.5, Smoothing::Immediate, true),
    float("toneBass", 2, 0.0, 10.0, 5.0, Mapping::Linear, true),
    float("toneMid", 3, 0.0, 10.0, 5.0, Mapping::Linear, true),
    float("toneTreble", 4, 0.0, 10.0, 5.0, Mapping::Linear, true),
    norm("outputLevel", 5, 0.5, Smoothing::Milliseconds(20), true),
    float(
        "gateThreshold",
        6,
        -100.0,
        0.0,
        -80.0,
        Mapping::Linear,
        true,
    ),
    float(
        "targetLoudness",
        7,
        -60.0,
        0.0,
        -18.0,
        Mapping::Linear,
        false,
    ),
    ParameterSpec {
        scope: scope(false, true),
        ..boolean("calibrateInput", 8, false, Smoothing::Immediate, false)
    },
    machine_float("inputCalibrationLevel", 9, -60.0, 60.0, 12.0),
    boolean("gateEnabled", 10, true, Smoothing::Immediate, true),
    boolean("toneEqEnabled", 11, true, Smoothing::Immediate, true),
    norm("outputBalance", 12, 0.5, Smoothing::Milliseconds(20), true),
    boolean(
        "spreadEnabled",
        13,
        false,
        Smoothing::Milliseconds(25),
        true,
    ),
    norm(
        "spreadOffset",
        14,
        0.8125,
        Smoothing::Milliseconds(100),
        true,
    ),
    norm("spreadWobble", 15, 0.25, Smoothing::Milliseconds(25), true),
    boolean(
        "spreadWobbleEnabled",
        16,
        true,
        Smoothing::Milliseconds(25),
        true,
    ),
    ParameterSpec {
        mapping: Mapping::Logarithmic,
        ..norm("spreadCrossover", 17, 0.5, Smoothing::Immediate, true)
    },
    boolean(
        "spreadCrossoverEnabled",
        18,
        true,
        Smoothing::Milliseconds(25),
        true,
    ),
    boolean(
        "spreadDiffuseEnabled",
        19,
        true,
        Smoothing::Milliseconds(25),
        true,
    ),
    boolean("alignEnabled", 20, false, Smoothing::Milliseconds(25), true),
    norm("alignOffset", 21, 0.5, Smoothing::Milliseconds(40), true),
    norm("alignWobble", 22, 0.25, Smoothing::Milliseconds(25), true),
    boolean(
        "alignWobbleEnabled",
        23,
        false,
        Smoothing::Milliseconds(25),
        true,
    ),
    ParameterSpec {
        mapping: Mapping::Logarithmic,
        ..norm("alignCrossover", 24, 0.5, Smoothing::Immediate, true)
    },
    boolean(
        "alignCrossoverEnabled",
        25,
        false,
        Smoothing::Milliseconds(25),
        true,
    ),
    boolean(
        "alignDiffuseEnabled",
        26,
        false,
        Smoothing::Milliseconds(25),
        true,
    ),
    norm("chainPanLeft", 27, 0.0, Smoothing::Milliseconds(20), true),
    norm("chainPanRight", 28, 1.0, Smoothing::Milliseconds(20), true),
    boolean(
        "chainPanLinked",
        29,
        true,
        Smoothing::Milliseconds(20),
        true,
    ),
    boolean(
        "chainSoloLeft",
        30,
        false,
        Smoothing::Milliseconds(20),
        false,
    ),
    boolean(
        "chainSoloRight",
        31,
        false,
        Smoothing::Milliseconds(20),
        false,
    ),
    boolean(
        "chainInvertLeft",
        32,
        false,
        Smoothing::Milliseconds(20),
        true,
    ),
    boolean(
        "chainInvertRight",
        33,
        false,
        Smoothing::Milliseconds(20),
        true,
    ),
    machine_boolean("osEnabled", 34, false),
    choice("osFactor", 35, OS_CHOICES, 0.0, false, false, true),
    float(
        "gateRelease",
        36,
        5.0,
        500.0,
        50.0,
        Mapping::Logarithmic,
        true,
    ),
    float("gateHold", 37, 0.0, 200.0, 20.0, Mapping::Linear, true),
    float("gateRange", 38, 20.0, 80.0, 80.0, Mapping::Linear, true),
    boolean("pitchEnabled", 39, false, Smoothing::Immediate, true),
    float(
        "pitchSemitones",
        40,
        -24.0,
        24.0,
        0.0,
        Mapping::Linear,
        true,
    ),
    boolean("pitchStep", 41, true, Smoothing::Immediate, true),
    float(
        "pitchTonality",
        42,
        1000.0,
        20000.0,
        20000.0,
        Mapping::Logarithmic,
        true,
    ),
    choice("pitchWindow", 43, WINDOW_CHOICES, 0.0, false, true, false),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str) -> &'static ParameterSpec {
        ParameterCatalog::v1()
            .iter()
            .find(|spec| spec.id.0 == id)
            .expect("catalog parameter")
    }

    #[test]
    fn a_logarithmic_range_with_a_positive_minimum_maps_logarithmically() {
        let release = spec("gateRelease");
        assert_eq!(release.value_at(0.0), 5.0);
        assert!((release.value_at(0.5) - 50.0).abs() < 0.01);
        assert!((release.value_at(1.0) - 500.0).abs() < 0.01);
        assert!((release.position_of(50.0) - 0.5).abs() < 0.0001);
    }

    #[test]
    fn a_logarithmic_range_from_zero_maps_linearly() {
        let crossover = spec("spreadCrossover");
        assert_eq!(crossover.mapping, Mapping::Logarithmic);
        assert_eq!(crossover.value_at(0.25), 0.25);
        assert_eq!(crossover.position_of(0.25), 0.25);
    }

    #[test]
    fn a_position_or_a_value_outside_the_range_is_clamped() {
        let bass = spec("toneBass");
        assert_eq!(bass.value_at(-1.0), 0.0);
        assert_eq!(bass.value_at(2.0), 10.0);
        assert_eq!(bass.value_at(f32::NAN), 0.0);
        assert_eq!(bass.position_of(20.0), 1.0);
        assert_eq!(bass.position_of(f32::NAN), 0.0);
        assert_eq!(spec("gateRelease").position_of(0.0), 0.0);
    }

    #[test]
    fn each_position_maps_back_to_itself() {
        for spec in ParameterCatalog::v1() {
            for position in [0.0, 0.3, 0.7, 1.0] {
                let back = spec.position_of(spec.value_at(position));
                assert!((back - position).abs() < 0.001, "{}", spec.id.0);
            }
        }
    }
}
