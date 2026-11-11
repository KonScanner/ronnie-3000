//! Bit-exact serial and worker phase/voice scheduling comparisons.

use std::num::{NonZeroU8, NonZeroUsize};

use ronie_dsp::{PhaseEngine, compile_model, rt::RtWorkerPool};
use ronie_oracle::synthetic::synthetic_a2_document;

fn input(frames: usize, channels: usize, offset: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|index| {
            let frame = index % frames;
            let channel = index / frames;
            (((frame + offset) * 37 % 257) as f32 - 128.0) / (128.0 + channel as f32)
        })
        .collect()
}

#[test]
fn serial_and_worker_outputs_are_bit_identical_for_every_factor_and_voice_layout() {
    let compiled = compile_model(&synthetic_a2_document(3).expect("synthetic document"))
        .expect("compiled model");
    let mut pool = RtWorkerPool::prepare(4, 16).expect("worker pool");
    for factor in [1, 2, 4, 8] {
        for dual_mono in [false, true] {
            let channels = if dual_mono { 2 } else { 1 };
            let mut serial = PhaseEngine::new(
                &compiled,
                NonZeroU8::new(factor).expect("factor"),
                NonZeroU8::new(2).expect("voices"),
            )
            .expect("serial engine");
            let mut parallel = PhaseEngine::new(
                &compiled,
                NonZeroU8::new(factor).expect("factor"),
                NonZeroU8::new(2).expect("voices"),
            )
            .expect("parallel engine");
            serial
                .prepare(48_000.0, NonZeroUsize::new(67).expect("frames"), false)
                .expect("serial preparation");
            parallel
                .prepare(48_000.0, NonZeroUsize::new(67).expect("frames"), false)
                .expect("parallel preparation");
            for frames in [1, 3, 16, 31, 64, 67, 5] {
                let mut expected = input(frames, channels, frames);
                let mut actual = expected.clone();
                serial
                    .process_planar(&mut expected, channels, dual_mono)
                    .expect("serial processing");
                parallel
                    .process_planar_with_pool(&mut actual, channels, dual_mono, &pool)
                    .expect("parallel processing");
                assert_eq!(
                    actual, expected,
                    "factor {factor}, dual mono {dual_mono}, frames {frames}"
                );
            }
            for _ in 0..62 {
                let mut expected = vec![f32::from_bits(1); channels * 67];
                let mut actual = expected.clone();
                serial
                    .process_planar(&mut expected, channels, dual_mono)
                    .expect("serial tail");
                parallel
                    .process_planar_with_pool(&mut actual, channels, dual_mono, &pool)
                    .expect("parallel tail");
                assert_eq!(actual, expected, "denormal tail factor {factor}");
            }
        }
    }
    pool.release();
}
