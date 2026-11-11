//! Independent differential and comparison tests.

use ronie_core::{NamDocument, parse_nam};
use ronie_dsp as _;
use ronie_oracle as _;
use ronie_oracle::{
    DifferentialCase, DifferentialStream, FiniteEquivalence, NonFiniteEquivalence,
    PublicMathOracle, ReferenceOracle, compare_samples, reference_paths, run_differential,
};
use serde_json as _;

fn case(
    document: NamDocument,
    stream: DifferentialStream,
    block_size: usize,
    frame_count: usize,
) -> DifferentialCase {
    DifferentialCase {
        name: format!("{stream:?}-{block_size}-{frame_count}"),
        document,
        model_file: None,
        stream,
        block_size,
        frame_count,
        seed: 0x5eed,
        finite: FiniteEquivalence::MaxAbs(5e-5),
        tolerance_reason: Some(
            "independent loop nesting and code generation may reorder or contract f32 arithmetic",
        ),
        nonfinite: if stream == DifferentialStream::NonFinite {
            NonFiniteEquivalence::SameClass
        } else {
            NonFiniteEquivalence::ExactBits
        },
    }
}

#[test]
fn public_math_matches_all_streams_tiers_and_block_sizes() {
    for channels in [3, 8] {
        for stream in [
            DifferentialStream::Silence,
            DifferentialStream::Impulse,
            DifferentialStream::Sines,
            DifferentialStream::Noise,
            DifferentialStream::Transients,
            DifferentialStream::NonFinite,
            DifferentialStream::Denormals,
        ] {
            for block_size in [1, 31, 64, 65, 256] {
                let document = ronie_oracle::synthetic::synthetic_a2_document(channels)
                    .expect("synthetic document");
                let case = case(document, stream, block_size, block_size + 31);
                let report = run_differential(&case, &mut PublicMathOracle::new())
                    .unwrap_or_else(|error| panic!("{error}"));
                assert_eq!(report.compared_samples, block_size + 31);
                assert_eq!(report.mismatch_count, 0);
            }
        }
    }

    let document = ronie_oracle::synthetic::synthetic_a2_document(3).expect("synthetic document");
    let case = case(document, DifferentialStream::Sines, 256, 4096);
    let report = run_differential(&case, &mut PublicMathOracle::new()).expect("long stream");
    assert_eq!(report.compared_samples, 4096);
    assert_eq!(report.mismatch_count, 0);
}

#[test]
fn comparator_distinguishes_signed_zero_and_tolerance_boundaries() {
    let document = ronie_oracle::synthetic::synthetic_a2_document(3).expect("synthetic document");
    let mut case = case(document, DifferentialStream::Silence, 1, 1);
    case.finite = FiniteEquivalence::ExactBits;

    let report = compare_samples(&[0.0], &[-0.0], &case);
    assert_eq!(report.mismatch_count, 1);

    case.finite = FiniteEquivalence::MaxAbs(0.0);
    let report = compare_samples(&[0.0], &[-0.0], &case);
    assert_eq!(report.mismatch_count, 0);
    assert_eq!(report.max_abs_error, Some(0.0));

    case.finite = FiniteEquivalence::MaxAbs(0.5);
    assert_eq!(compare_samples(&[1.0], &[1.5], &case).mismatch_count, 0);
    assert_eq!(compare_samples(&[1.0], &[1.5001], &case).mismatch_count, 1);
}

#[test]
fn comparator_applies_explicit_nonfinite_rules() {
    let document = ronie_oracle::synthetic::synthetic_a2_document(3).expect("synthetic document");
    let mut case = case(document, DifferentialStream::NonFinite, 1, 1);
    let nan = f64::NAN;
    let other_nan = f64::from_bits(nan.to_bits() + 1);

    case.nonfinite = NonFiniteEquivalence::ExactBits;
    assert_eq!(compare_samples(&[nan], &[nan], &case).mismatch_count, 0);
    assert_eq!(
        compare_samples(&[nan], &[other_nan], &case).mismatch_count,
        1
    );
    assert_eq!(compare_samples(&[nan], &[0.0], &case).mismatch_count, 1);
    assert_eq!(
        compare_samples(&[f64::INFINITY], &[f64::INFINITY], &case).mismatch_count,
        0
    );
    assert_eq!(
        compare_samples(&[f64::INFINITY], &[f64::NEG_INFINITY], &case).mismatch_count,
        1
    );

    case.nonfinite = NonFiniteEquivalence::SameClass;
    let report = compare_samples(&[nan], &[other_nan], &case);
    assert_eq!(report.mismatch_count, 0);
    assert_eq!(report.max_abs_error, None);
    assert!(report.max_abs_error.is_none_or(|error| error.is_finite()));
}

#[test]
fn comparator_preserves_denormal_exactness() {
    let document = ronie_oracle::synthetic::synthetic_a2_document(3).expect("synthetic document");
    let mut case = case(document, DifferentialStream::Denormals, 1, 1);
    case.finite = FiniteEquivalence::ExactBits;
    let denormal = f64::from_bits(1);

    assert_eq!(
        compare_samples(&[denormal], &[denormal], &case).mismatch_count,
        0
    );
    assert_eq!(
        compare_samples(&[denormal], &[0.0], &case).mismatch_count,
        1
    );
}

#[test]
#[ignore = "requires private Reference source and build directories"]
fn private_reference_matches_full_tier_when_configured() {
    let Some((root, _build)) = reference_paths().expect("valid Reference configuration") else {
        eprintln!("skipping private Reference case: source and build paths are not configured");
        return;
    };
    let model_file = root.join("test/files/a2-amp-test.nam");
    let bytes = std::fs::read(&model_file).expect("private model bytes");
    let document = parse_nam(&bytes).expect("private model document");
    let mut oracle = ReferenceOracle::new().expect("Reference adapter");
    for stream in [
        DifferentialStream::Silence,
        DifferentialStream::Impulse,
        DifferentialStream::Sines,
        DifferentialStream::Noise,
        DifferentialStream::Transients,
        DifferentialStream::NonFinite,
        DifferentialStream::Denormals,
    ] {
        for block_size in [1, 31, 64, 65, 256] {
            let mut case = case(document.clone(), stream, block_size, block_size + 31);
            case.model_file = Some(model_file.clone());
            let report =
                run_differential(&case, &mut oracle).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(report.mismatch_count, 0);
        }
    }
}
