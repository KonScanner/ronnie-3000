use ronie_core::{DspError, NamDocument};
use ronie_dsp::build_model;
use std::path::PathBuf;
use thiserror::Error;

use crate::fixtures::FixtureError;

/// Input streams shared by both differential adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifferentialStream {
    /// All zero samples.
    Silence,
    /// One initial impulse.
    Impulse,
    /// 220 Hz and 1,230 Hz sine mixture.
    Sines,
    /// Seeded xorshift noise.
    Noise,
    /// Isolated positive and negative transients.
    Transients,
    /// NaN and signed infinities.
    NonFinite,
    /// Positive and negative subnormal values.
    Denormals,
}

impl DifferentialStream {
    /// Stable command-line stream name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Silence => "silence",
            Self::Impulse => "impulse",
            Self::Sines => "sines",
            Self::Noise => "noise",
            Self::Transients => "transients",
            Self::NonFinite => "nonfinite",
            Self::Denormals => "denormals",
        }
    }
}

/// Finite-sample comparison policy for one case.
#[derive(Debug, Clone, Copy)]
pub enum FiniteEquivalence {
    /// Exact `f64` bit patterns, including signed zero.
    ExactBits,
    /// Maximum absolute error, at most `5e-5`.
    MaxAbs(f64),
}

/// Nonfinite-sample comparison policy for one case.
#[derive(Debug, Clone, Copy)]
pub enum NonFiniteEquivalence {
    /// Exact `f64` bit patterns.
    ExactBits,
    /// Any NaN matches any NaN; infinities must have the same sign.
    SameClass,
}

/// One differential render and comparison case.
#[derive(Debug, Clone)]
pub struct DifferentialCase {
    /// Human-readable case name.
    pub name: String,
    /// Document rendered by the Rust engine.
    pub document: NamDocument,
    /// File rendered by the private adapter, when applicable.
    pub model_file: Option<PathBuf>,
    /// Generated input stream.
    pub stream: DifferentialStream,
    /// Callback size in frames.
    pub block_size: usize,
    /// Total frames to render.
    pub frame_count: usize,
    /// Deterministic stream seed.
    pub seed: u64,
    /// Finite comparison policy.
    pub finite: FiniteEquivalence,
    /// Why reordered arithmetic permits a finite tolerance for this case.
    pub tolerance_reason: Option<&'static str>,
    /// Nonfinite comparison policy.
    pub nonfinite: NonFiniteEquivalence,
}

impl DifferentialCase {
    /// Generates the input shared by both adapters.
    #[must_use]
    pub fn input(&self) -> Vec<f64> {
        let mut state = self.seed;
        (0..self.frame_count)
            .map(|index| match self.stream {
                DifferentialStream::Silence => 0.0,
                DifferentialStream::Impulse => {
                    if index == 0 {
                        1.0
                    } else {
                        0.0
                    }
                }
                DifferentialStream::Sines => {
                    let phase = std::f64::consts::TAU * index as f64 / 48_000.0;
                    0.5 * ((220.0 * phase).sin() + (1_230.0 * phase).sin())
                }
                DifferentialStream::Noise => {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (state as f64 / u64::MAX as f64) * 2.0 - 1.0
                }
                DifferentialStream::Transients => {
                    if index == self.frame_count / 4 || index == self.frame_count / 2 {
                        1.0
                    } else if index == self.frame_count * 3 / 4 {
                        -1.0
                    } else {
                        0.0
                    }
                }
                DifferentialStream::NonFinite => match index % 4 {
                    0 => f64::NAN,
                    1 => f64::INFINITY,
                    2 => f64::NEG_INFINITY,
                    _ => 0.0,
                },
                DifferentialStream::Denormals => match index % 3 {
                    0 => f64::from_bits(1),
                    1 => -f64::from_bits(1),
                    _ => 0.0,
                },
            })
            .collect()
    }
}

/// Differential comparison evidence for one case.
#[derive(Debug)]
pub struct DifferentialReport {
    /// Case name.
    pub case_name: String,
    /// Callback size.
    pub block_size: usize,
    /// Stream kind.
    pub stream: DifferentialStream,
    /// Finite policy.
    pub finite: FiniteEquivalence,
    /// Reason for allowing reordered arithmetic in this case.
    pub tolerance_reason: Option<&'static str>,
    /// Nonfinite policy.
    pub nonfinite: NonFiniteEquivalence,
    /// Compared sample count, including length mismatches.
    pub compared_samples: usize,
    /// Number of mismatched samples.
    pub mismatch_count: usize,
    /// Maximum absolute error over finite pairs only.
    pub max_abs_error: Option<f64>,
    /// First mismatching sample index.
    pub first_mismatch: Option<usize>,
}

/// Development oracle errors.
#[derive(Debug, Error)]
pub enum OracleError {
    /// Case parameters or model shape are invalid.
    #[error("invalid differential case: {0}")]
    InvalidCase(&'static str),
    /// Rust DSP construction or processing failed.
    #[error("Rust DSP failed: {0}")]
    Dsp(#[from] DspError),
    /// Private fixture configuration failed.
    #[error("Reference fixture configuration failed: {0}")]
    Fixture(#[from] FixtureError),
    /// Private Reference paths are not configured.
    #[error("private Reference paths are not configured")]
    NotConfigured,
    /// The private adapter requires a model file.
    #[error("private Reference adapter requires a model file")]
    MissingModelFile,
    /// File or process I/O failed.
    #[error("oracle I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// Reference executable returned failure.
    #[error("Reference executable failed: {0}")]
    ProcessFailed(String),
    /// Reference output length is invalid.
    #[error("invalid Reference output length")]
    InvalidOutputLength,
    /// Full-chain Reference output is malformed.
    #[error("invalid full-chain Reference output: {0}")]
    InvalidChainOutput(&'static str),
    /// Full-chain Reference executable returned failure without exposing stderr.
    #[error("full-chain Reference executable failed: {0}")]
    ChainProcessFailed(std::process::ExitStatus),
    /// The requested state operation is not implemented by the private runner.
    #[error("unsupported Reference state command")]
    UnsupportedStateCommand,
    /// Reference state output exceeded the configured bound.
    #[error("Reference state output exceeded the {limit}-byte limit")]
    StateOutputTooLarge {
        /// Maximum permitted output size.
        limit: usize,
    },
    /// Reference state executable returned failure without exposing its stderr.
    #[error("Reference state executable failed: {0}")]
    StateProcessFailed(std::process::ExitStatus),
    /// Outputs differ under the case's comparison policies.
    #[error("differential mismatch: {report:?}")]
    Divergence {
        /// Complete mismatch report.
        report: Box<DifferentialReport>,
    },
}

/// Second-implementation adapter used by [`run_differential`].
pub trait ModelOracle {
    /// Renders a case with an independent implementation.
    ///
    /// # Errors
    ///
    /// Returns model, configuration, or process errors.
    fn render(&mut self, case: &DifferentialCase) -> Result<Vec<f64>, OracleError>;
}

/// Runs the Rust engine and an independent adapter on the same input.
///
/// # Errors
///
/// Returns configuration or processing errors, and [`OracleError::Divergence`]
/// with the complete report when any sample mismatches.
pub fn run_differential(
    case: &DifferentialCase,
    oracle: &mut dyn ModelOracle,
) -> Result<DifferentialReport, OracleError> {
    if case.frame_count == 0 || case.block_size == 0 {
        return Err(OracleError::InvalidCase(
            "frames and block size must be nonzero",
        ));
    }
    if let FiniteEquivalence::MaxAbs(limit) = case.finite
        && (!limit.is_finite() || !(0.0..=5e-5).contains(&limit))
    {
        return Err(OracleError::InvalidCase(
            "finite tolerance must be between 0 and 5e-5",
        ));
    }

    let input = case.input();
    let mut engine = build_model(&case.document)?;
    engine.prepare(
        48_000.0,
        std::num::NonZeroUsize::new(case.block_size)
            .ok_or(OracleError::InvalidCase("zero block"))?,
        false,
    )?;
    let mut actual = vec![0.0; input.len()];
    for (start, chunk) in input.chunks(case.block_size).enumerate() {
        let begin = start * case.block_size;
        let end = begin + chunk.len();
        engine.process(chunk, &mut actual[begin..end])?;
    }
    let expected = oracle.render(case)?;
    let report = compare_samples(&actual, &expected, case);
    if report.mismatch_count > 0 {
        return Err(OracleError::Divergence {
            report: Box::new(report),
        });
    }
    Ok(report)
}

/// Compares two rendered streams using the case's explicit policies.
#[must_use]
pub fn compare_samples(
    actual: &[f64],
    expected: &[f64],
    case: &DifferentialCase,
) -> DifferentialReport {
    let mut report = DifferentialReport {
        case_name: case.name.clone(),
        block_size: case.block_size,
        stream: case.stream,
        finite: case.finite,
        tolerance_reason: case.tolerance_reason,
        nonfinite: case.nonfinite,
        compared_samples: actual.len().max(expected.len()),
        mismatch_count: actual.len().abs_diff(expected.len()),
        max_abs_error: None,
        first_mismatch: None,
    };

    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let mismatch = if actual.is_finite() && expected.is_finite() {
            let difference = (actual - expected).abs();
            report.max_abs_error = Some(
                report
                    .max_abs_error
                    .map_or(difference, |current| current.max(difference)),
            );
            match case.finite {
                FiniteEquivalence::ExactBits => actual.to_bits() != expected.to_bits(),
                FiniteEquivalence::MaxAbs(limit) => difference > limit,
            }
        } else {
            match case.nonfinite {
                NonFiniteEquivalence::ExactBits => actual.to_bits() != expected.to_bits(),
                NonFiniteEquivalence::SameClass => {
                    if actual.is_nan() && expected.is_nan() {
                        false
                    } else if actual.is_infinite() && expected.is_infinite() {
                        actual.is_sign_positive() != expected.is_sign_positive()
                    } else {
                        true
                    }
                }
            }
        };

        if mismatch {
            report.mismatch_count += 1;
            report.first_mismatch.get_or_insert(index);
        }
    }
    report
}
