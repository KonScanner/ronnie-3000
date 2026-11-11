//! Prepared A2 state tests.

use ronie_core::DspError;
use ronie_dsp::{A2Engine, CompiledA2};
use ronie_oracle::synthetic::synthetic_a2_document;
use serde_json as _;
use std::{num::NonZeroUsize, sync::Arc};

const KERNELS: [usize; 23] = [
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 15, 15, 6, 6, 6, 6, 6, 6, 6,
];
const DILATIONS: [usize; 23] = [
    1, 3, 7, 17, 41, 101, 239, 1, 3, 7, 17, 41, 101, 239, 1, 13, 1, 3, 7, 17, 41, 101, 239,
];

fn compiled(channels: usize) -> Arc<CompiledA2> {
    let document = synthetic_a2_document(channels).expect("valid synthetic document");
    Arc::new(ronie_dsp::compile_a2(&document).expect("valid compiled model"))
}

fn next_power_of_two(value: usize) -> usize {
    value.checked_next_power_of_two().expect("small test value")
}

#[test]
fn prepares_exact_history_layouts_for_supported_block_sizes() {
    for max_frames in [1, 64, 65, 4096] {
        let model = compiled(3);
        let mut engine = A2Engine::<3>::new(model).expect("matching channels");
        assert_eq!(engine.max_frames(), 0);
        assert_eq!(engine.prewarm_samples(), 6_346);

        engine
            .prepare(
                48_000.0,
                NonZeroUsize::new(max_frames).expect("nonzero"),
                false,
            )
            .expect("valid preparation");

        assert_eq!(engine.max_frames(), max_frames);
        for index in 0..23 {
            let lookback = (KERNELS[index] - 1) * DILATIONS[index];
            let capacity = next_power_of_two(lookback + max_frames);
            let info = engine.layer_history(index).expect("prepared layer history");
            assert_eq!(
                info.ring_capacity, capacity,
                "layer {index}, block {max_frames}"
            );
            assert_eq!(info.storage_values, 3 * (capacity + max_frames));
            assert_eq!(info.tail_values, 3 * max_frames);
            assert!(info.all_zero);
        }

        let head_capacity = next_power_of_two(15 + max_frames);
        let head = engine.head_history().expect("prepared head history");
        assert_eq!(head.ring_capacity, head_capacity);
        assert_eq!(head.storage_values, 3 * (head_capacity + max_frames));
        assert_eq!(head.tail_values, 3 * max_frames);
        assert!(head.all_zero);
    }
}

#[test]
fn shares_compiled_weights_but_not_prewarmed_history() {
    let model = compiled(3);
    assert_eq!(Arc::strong_count(&model), 1);
    let mut first = A2Engine::<3>::new(Arc::clone(&model)).expect("first engine");
    let mut second = A2Engine::<3>::new(Arc::clone(&model)).expect("second engine");
    assert_eq!(Arc::strong_count(&model), 3);

    first
        .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), true)
        .expect("prewarm should process zero input");
    second
        .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
        .expect("reset without prewarm should succeed");

    assert!(!first.layer_history(1).expect("first layer").all_zero);
    assert!(second.layer_history(1).expect("second layer").all_zero);
    assert!(!first.head_history().expect("first head").all_zero);
    assert!(second.head_history().expect("second head").all_zero);

    first
        .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
        .expect("reset without prewarm should succeed");
    assert!(first.layer_history(1).expect("reset layer").all_zero);
    assert!(first.head_history().expect("reset head").all_zero);
}

#[test]
fn rejects_invalid_sample_rates() {
    for sample_rate in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut engine = A2Engine::<3>::new(compiled(3)).expect("matching channels");
        let result = engine.prepare(sample_rate, NonZeroUsize::new(64).expect("nonzero"), false);
        assert!(matches!(result, Err(DspError::InvalidSampleRate)));
        assert_eq!(engine.max_frames(), 0);
    }
}

#[test]
fn rejects_channel_mismatch_and_overflowing_layout() {
    let result = A2Engine::<8>::new(compiled(3));
    assert!(matches!(result, Err(DspError::ChannelMismatch { .. })));

    let mut engine = A2Engine::<3>::new(compiled(3)).expect("matching channels");
    let result = engine.prepare(
        48_000.0,
        NonZeroUsize::new(usize::MAX).expect("nonzero"),
        false,
    );
    assert!(matches!(result, Err(DspError::InvalidBufferSize)));
}
