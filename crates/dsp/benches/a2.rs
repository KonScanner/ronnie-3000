//! Criterion A2 microbenchmarks.

// Criterion generates public entry points without doc comments.
#![allow(missing_docs)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ronie_dsp::{PhaseEngine, compile_model};
use ronie_oracle::synthetic::synthetic_a2_document;
use std::num::{NonZeroU8, NonZeroUsize};

fn a2_standard(c: &mut Criterion) {
    let document = synthetic_a2_document(8).expect("synthetic document");
    let compiled = compile_model(&document).expect("compiled model");
    let mut engine = PhaseEngine::new(
        &compiled,
        NonZeroU8::new(1).expect("factor"),
        NonZeroU8::new(1).expect("voices"),
    )
    .expect("phase engine");
    engine
        .prepare(48_000.0, NonZeroUsize::new(64).expect("block size"), true)
        .expect("preparation");
    let mut buffer = vec![0.25_f32; 64];

    c.bench_function("a2_standard_48k_mono_64_serial", |bencher| {
        bencher.iter(|| {
            engine
                .process_planar(black_box(&mut buffer), 1, false)
                .expect("benchmark process");
        });
    });
}

fn a2_matrix(c: &mut Criterion) {
    let mut group = c.benchmark_group("a2_matrix");
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(2));
    group.sample_size(20);
    for channels in [3, 8] {
        let document = synthetic_a2_document(channels).expect("synthetic document");
        let compiled = compile_model(&document).expect("compiled model");
        for frames in [32, 64, 128, 256, 512] {
            for factor in [1, 8] {
                for voices in [1, 2] {
                    let mut engine = PhaseEngine::new(
                        &compiled,
                        NonZeroU8::new(factor).expect("factor"),
                        NonZeroU8::new(voices).expect("voices"),
                    )
                    .expect("phase engine");
                    engine
                        .prepare(
                            48_000.0 * f64::from(factor),
                            NonZeroUsize::new(frames).expect("frames"),
                            true,
                        )
                        .expect("preparation");
                    let planar_channels = voices as usize;
                    let mut buffer = vec![0.25_f32; frames * planar_channels];
                    let name = format!("a2_c{channels}_f{frames}_x{factor}_v{voices}");
                    group.bench_function(name, |bencher| {
                        bencher.iter(|| {
                            engine
                                .process_planar(
                                    black_box(&mut buffer),
                                    planar_channels,
                                    voices == 2,
                                )
                                .expect("benchmark process");
                        });
                    });
                }
            }
        }
    }
    group.finish();
}

criterion_group!(benches, a2_standard, a2_matrix);
criterion_main!(benches);
