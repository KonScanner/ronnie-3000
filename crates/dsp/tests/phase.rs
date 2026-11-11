//! Phase-interleaved and dual-mono processing tests.
#![allow(unsafe_code)]

use ronie_core::DspError;
use ronie_dsp::{CompiledModel, ModelEngine, PhaseEngine, compile_model};
use ronie_oracle::synthetic::synthetic_a2_document;
use serde_json as _;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    num::{NonZeroU8, NonZeroUsize},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

static TEST_LOCK: Mutex<()> = Mutex::new(());
// Per thread, because the test harness also allocates on its own threads.
thread_local! {
    static ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DEALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: every method forwards the original GlobalAlloc contract to System;
// the counters are per thread and do not alter pointer ownership or layout.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the caller supplies a valid layout, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        DEALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the pointer and layout are the allocator's original pair.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn lock() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn assert_no_allocations(operation: impl FnOnce()) {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    operation();
    assert_eq!(ALLOCATIONS.with(std::cell::Cell::get), 0);
    assert_eq!(DEALLOCATIONS.with(std::cell::Cell::get), 0);
}

fn compiled() -> CompiledModel {
    compile_model(&synthetic_a2_document(3).expect("valid synthetic document"))
        .expect("valid compiled model")
}

fn planar_input(frames: usize, channels: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames * channels];
    for channel in 0..channels {
        for frame in 0..frames {
            input[channel * frames + frame] =
                ((frame * 37 % 257) as f32 - 128.0) / (128.0 + channel as f32);
        }
    }
    input
}

fn native_render(
    compiled: &CompiledModel,
    input: &[f32],
    channels: usize,
    factor: usize,
    voices: usize,
    dual_mono: bool,
) -> Vec<f32> {
    let frames = input.len() / channels;
    let phase_max = frames.div_ceil(factor);
    let mut engines: Vec<ModelEngine> = (0..voices * factor)
        .map(|_| ModelEngine::new(compiled).expect("native engine"))
        .collect();
    for engine in &mut engines {
        engine
            .prepare(
                48_000.0,
                NonZeroUsize::new(phase_max).expect("nonzero phase maximum"),
                false,
            )
            .expect("native preparation");
    }

    let mut output = vec![0.0; input.len()];
    for voice in 0..voices {
        if !dual_mono && voice > 0 {
            continue;
        }
        let channel = if dual_mono { voice } else { 0 };
        for phase in 0..factor {
            let count = if phase < frames {
                (frames - phase).div_ceil(factor)
            } else {
                0
            };
            let mut native_input = vec![0.0; count];
            let mut native_output = vec![0.0; count];
            for (index, frame) in (phase..frames).step_by(factor).enumerate() {
                native_input[index] = input[channel * frames + frame] as f64;
            }
            engines[voice * factor + phase]
                .process(&native_input, &mut native_output)
                .expect("native process");
            for (index, frame) in (phase..frames).step_by(factor).enumerate() {
                output[channel * frames + frame] = native_output[index] as f32;
            }
        }
    }
    if !dual_mono {
        for channel in 1..channels {
            let source = output[..frames].to_vec();
            output[channel * frames..(channel + 1) * frames].copy_from_slice(&source);
        }
    }
    output
}

fn phase_render(
    compiled: &CompiledModel,
    input: &[f32],
    channels: usize,
    factor: usize,
    voices: usize,
    dual_mono: bool,
    max_frames: usize,
) -> Vec<f32> {
    let mut engine = PhaseEngine::new(
        compiled,
        NonZeroU8::new(factor as u8).expect("factor"),
        NonZeroU8::new(voices as u8).expect("voices"),
    )
    .expect("phase engine");
    engine
        .prepare(
            48_000.0,
            NonZeroUsize::new(max_frames).expect("nonzero maximum"),
            false,
        )
        .expect("phase preparation");
    let frames = input.len() / channels;
    let mut output = input.to_vec();
    for start in (0..frames).step_by(max_frames) {
        let end = (start + max_frames).min(frames);
        let mut block = Vec::with_capacity((end - start) * channels);
        for channel in 0..channels {
            block.extend_from_slice(&output[channel * frames + start..channel * frames + end]);
        }
        engine
            .process_planar(&mut block, channels, dual_mono)
            .expect("phase process");
        for channel in 0..channels {
            let block_start = channel * (end - start);
            output[channel * frames + start..channel * frames + end]
                .copy_from_slice(&block[block_start..block_start + end - start]);
        }
    }
    output
}

fn assert_bits_eq(actual: &[f32], expected: &[f32]) {
    let actual: Vec<u32> = actual.iter().map(|value| value.to_bits()).collect();
    let expected: Vec<u32> = expected.iter().map(|value| value.to_bits()).collect();
    assert_eq!(actual, expected);
}

#[test]
fn matches_native_phase_streams_for_every_factor_and_voice_count() {
    let _guard = lock();
    let compiled = compiled();
    let input = planar_input(65, 2);
    for factor in [1, 2, 4, 8] {
        for voices in [1, 2] {
            for dual_mono in [false, true] {
                if voices == 1 && dual_mono {
                    continue;
                }
                let expected = native_render(&compiled, &input, 2, factor, voices, dual_mono);
                let actual = phase_render(&compiled, &input, 2, factor, voices, dual_mono, 65);
                assert_bits_eq(&actual, &expected);
            }
        }
    }
}

#[test]
fn output_is_bit_identical_across_call_slicing() {
    let _guard = lock();
    let compiled = compiled();
    let input = planar_input(257, 2);
    let expected = phase_render(&compiled, &input, 2, 8, 2, true, 257);
    for block_size in [1, 31, 64, 65, 256] {
        let actual = phase_render(&compiled, &input, 2, 8, 2, true, block_size);
        assert_bits_eq(&actual, &expected);
    }
}

#[test]
fn shares_compiled_weights_once_per_phase_voice_slot() {
    let _guard = lock();
    let document = synthetic_a2_document(8).expect("valid Standard document");
    let compiled = compile_model(&document).expect("valid compiled model");
    let CompiledModel::Single(model) = &compiled else {
        panic!("expected single model");
    };
    let shared = Arc::clone(model);
    assert_eq!(Arc::strong_count(&shared), 2);

    let engine = PhaseEngine::new(
        &compiled,
        NonZeroU8::new(2).expect("factor"),
        NonZeroU8::new(2).expect("voices"),
    )
    .expect("phase engine");

    assert_eq!(Arc::strong_count(&shared), 6);
    assert_eq!(engine.max_frames(), 0);
}

#[test]
fn rejects_invalid_calls_without_allocation_or_buffer_mutation() {
    let _guard = lock();
    let compiled = compiled();
    for factor in [3, 16] {
        assert!(matches!(
            PhaseEngine::new(
                &compiled,
                NonZeroU8::new(factor).expect("nonzero"),
                NonZeroU8::new(1).expect("one voice")
            ),
            Err(DspError::InvalidOversamplingFactor)
        ));
    }
    assert!(matches!(
        PhaseEngine::new(
            &compiled,
            NonZeroU8::new(1).expect("factor"),
            NonZeroU8::new(3).expect("nonzero voices")
        ),
        Err(DspError::InvalidVoiceCount)
    ));

    let mut unprepared = PhaseEngine::new(
        &compiled,
        NonZeroU8::new(2).expect("factor"),
        NonZeroU8::new(1).expect("voice"),
    )
    .expect("phase engine");
    let mut buffer = vec![7.0; 4];
    assert_no_allocations(|| {
        assert!(matches!(
            unprepared.process_planar(&mut buffer, 1, false),
            Err(DspError::NotPrepared)
        ));
    });
    assert_eq!(buffer, vec![7.0; 4]);

    let mut engine = PhaseEngine::new(
        &compiled,
        NonZeroU8::new(2).expect("factor"),
        NonZeroU8::new(2).expect("voices"),
    )
    .expect("phase engine");
    engine
        .prepare(48_000.0, NonZeroUsize::new(64).expect("nonzero"), false)
        .expect("preparation");
    assert_eq!(engine.max_frames(), 64);

    type ErrorCheck = fn(DspError) -> bool;
    type InvalidCase = (usize, bool, usize, ErrorCheck);
    let cases: &[InvalidCase] = &[
        (0, false, 4, |error| {
            matches!(error, DspError::InvalidChannelCount)
        }),
        (3, false, 6, |error| {
            matches!(error, DspError::InvalidChannelCount)
        }),
        (2, false, 3, |error| {
            matches!(error, DspError::InvalidPlanarLength)
        }),
        (1, true, 4, |error| {
            matches!(error, DspError::InvalidVoiceConfiguration)
        }),
        (1, false, 65, |error| {
            matches!(
                error,
                DspError::BlockTooLarge {
                    max: 64,
                    actual: 65
                }
            )
        }),
    ];
    for (channels, dual_mono, length, check) in cases {
        let mut buffer = vec![9.0; *length];
        let original = buffer.clone();
        assert_no_allocations(|| {
            let result = engine.process_planar(&mut buffer, *channels, *dual_mono);
            assert!(check(result.expect_err("invalid call should fail")));
        });
        assert_eq!(buffer, original);
    }
}
