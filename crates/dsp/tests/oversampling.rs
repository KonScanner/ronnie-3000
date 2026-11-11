//! Half-band coefficient, island, streaming, and allocation tests.
#![allow(unsafe_code)]

use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use ronie_core::InputMode;
use ronie_dsp::{
    ChainError, PlanarBuffer,
    oversampling::ChainOversampler,
    rate::halfband::{COEFFICIENTS_A, COEFFICIENTS_B},
};
use ronie_oracle::{ChainOracle, ChainScenario, OracleError};
use serde_json::json;

static TEST_LOCK: Mutex<()> = Mutex::new(());
thread_local! {
    static ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DEALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: every method forwards the original contract and only adds counters.
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the layout is forwarded unchanged.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        DEALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the pointer and layout are the allocator's original pair.
        unsafe { std::alloc::System.dealloc(pointer, layout) }
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

fn identity(_: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
    Ok(())
}

fn planar_chunk(input: &[f32], total_frames: usize, offset: usize, frames: usize) -> Vec<f32> {
    let mut chunk = vec![0.0; frames * 2];
    for channel in 0..2 {
        chunk[channel * frames..(channel + 1) * frames].copy_from_slice(
            &input[channel * total_frames + offset..channel * total_frames + offset + frames],
        );
    }
    chunk
}

fn store_planar_chunk(
    output: &mut [f32],
    chunk: &[f32],
    total_frames: usize,
    offset: usize,
    frames: usize,
) {
    for channel in 0..2 {
        output[channel * total_frames + offset..channel * total_frames + offset + frames]
            .copy_from_slice(&chunk[channel * frames..(channel + 1) * frames]);
    }
}

#[test]
fn halfband_coefficients_match_reference_bits() {
    assert_eq!(
        COEFFICIENTS_A.map(f32::to_bits),
        [
            0x3d163f58, 0x3e8c9c89, 0x3f0fa42f, 0x3f450dcd, 0x3f646b34, 0x3f764bd4,
        ]
    );
    assert_eq!(
        COEFFICIENTS_B.map(f32::to_bits),
        [
            0x3e0bd324, 0x3ed8a5a0, 0x3f2d7344, 0x3f570302, 0x3f6e7989, 0x3f7ce189,
        ]
    );
}

#[test]
fn factor_one_is_transparent() {
    let mut oversampler = ChainOversampler::new(1, 3).expect("factor one");
    let mut input_data = [0.25, -0.5, 0.75, 1.0, -1.0, 0.5];
    let input = PlanarBuffer::new(&mut input_data, 2, 3).expect("input");
    let mut output_data = [0.0; 6];
    let mut output = PlanarBuffer::new(&mut output_data, 2, 3).expect("output");

    oversampler
        .process(&input, &mut output, identity)
        .expect("process");

    assert_eq!(output_data, input_data);
    assert_eq!(oversampler.latency_samples(), 0);
}

#[test]
fn every_factor_is_bit_identical_across_base_callback_slicing() {
    let mut input_data: Vec<f32> = (0..192)
        .map(|index| ((index as f32 + 1.0) * 0.17).sin() * 0.5)
        .collect();
    for factor in [2, 4, 8] {
        let mut whole = ChainOversampler::new(factor, 96).expect("whole");
        let input = PlanarBuffer::new(&mut input_data, 2, 96).expect("whole input");
        let mut expected = vec![0.0; 192];
        whole
            .process(
                &input,
                &mut PlanarBuffer::new(&mut expected, 2, 96).expect("whole output"),
                identity,
            )
            .expect("whole render");

        let mut chunked = ChainOversampler::new(factor, 64).expect("chunked");
        let mut actual = vec![0.0; 192];
        let mut offset = 0;
        for frames in [31, 33, 32] {
            let mut chunk_data = planar_chunk(&input_data, 96, offset, frames);
            let input = PlanarBuffer::new(&mut chunk_data, 2, frames).expect("chunk input");
            let mut chunk_output = vec![0.0; frames * 2];
            let mut output = PlanarBuffer::new(&mut chunk_output, 2, frames).expect("chunk output");
            chunked
                .process(&input, &mut output, identity)
                .expect("chunk");
            store_planar_chunk(&mut actual, &chunk_output, 96, offset, frames);
            offset += frames;
        }
        assert_eq!(actual, expected, "factor {factor}");
        assert_eq!(chunked.latency_samples(), 0);
    }
}

#[test]
fn invalid_calls_are_atomic_and_prepared_callbacks_do_not_allocate() {
    let _guard = lock();
    let mut oversampler = ChainOversampler::new(8, 8).expect("preparation");
    let mut island_oversampler = ChainOversampler::new(8, 8).expect("island preparation");
    let mut control = ChainOversampler::new(8, 8).expect("control");
    let mut input_data = [0.25, -0.5, 0.75, 1.0, -1.0, 0.5, 0.125, -0.25];
    let mut island_data = [input_data, input_data].concat();
    let input = PlanarBuffer::new(&mut input_data, 2, 4).expect("input");
    let mut island = PlanarBuffer::new(&mut island_data, 2, 8).expect("island");
    let mut invalid_data = [7.0; 4];
    let mut invalid = PlanarBuffer::new(&mut invalid_data, 1, 4).expect("invalid output");
    let mut actual_data = [0.0; 8];
    let mut actual = PlanarBuffer::new(&mut actual_data, 2, 4).expect("output");
    let mut expected_data = [0.0; 8];
    let mut expected = PlanarBuffer::new(&mut expected_data, 2, 4).expect("control");
    let mut invalid_result = Ok(());
    let mut island_result = Ok(());
    let mut valid_result = Ok(());

    assert_no_allocations(|| {
        invalid_result = oversampler.process(&input, &mut invalid, identity);
        island_result = island_oversampler.base_rate_island(&mut island, identity);
        valid_result = oversampler.process(&input, &mut actual, identity);
    });
    control
        .process(&input, &mut expected, identity)
        .expect("control");

    assert!(matches!(invalid_result, Err(ChainError::InvalidLayout(_))));
    assert!(island_result.is_ok());
    assert!(valid_result.is_ok());
    assert_eq!(invalid_data, [7.0; 4]);
    assert_eq!(actual_data, expected_data);
}

#[test]
fn phase_slots_have_independent_checked_views() {
    let document = ronie_oracle::synthetic::synthetic_a2_document(3).expect("document");
    let compiled = ronie_dsp::compile_model(&document).expect("compiled model");
    let mut engine = ronie_dsp::PhaseEngine::new(
        &compiled,
        std::num::NonZeroU8::new(2).expect("factor"),
        std::num::NonZeroU8::new(2).expect("voices"),
    )
    .expect("phase engine");

    let first = engine.slot(0, 0).expect("first slot") as *const _;
    let second = engine.slot(1, 0).expect("second slot") as *const _;
    assert_ne!(first, second);
    assert!(engine.slot(2, 0).is_none());
    assert!(engine.slot_mut(0, 2).is_none());
}

#[test]
#[ignore = "requires the private full-chain Reference runner"]
fn private_impulse_and_high_frequency_responses_match() {
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    for factor in [2, 4, 8] {
        for high_frequency in [false, true] {
            let mut scenario = scenario(factor, vec![96], "oversampling");
            scenario.input.fill(0.0);
            if high_frequency {
                for frame in 0..96 {
                    let phase = std::f32::consts::TAU * 18_000.0 * frame as f32 / 48_000.0;
                    scenario.input[frame] = phase.sin() * 0.5;
                    scenario.input[96 + frame] = phase.cos() * 0.5;
                }
            } else {
                scenario.input[0] = 1.0;
                scenario.input[96] = 1.0;
            }
            let reference = oracle.render(&scenario).expect("Reference response");
            let mut oversampler = ChainOversampler::new(factor, 96).expect("preparation");
            let input = PlanarBuffer::new(&mut scenario.input, 2, 96).expect("input");
            let mut actual = vec![0.0; 192];
            let mut output = PlanarBuffer::new(&mut actual, 2, 96).expect("output");
            oversampler
                .process(&input, &mut output, identity)
                .expect("process");

            assert_eq!(
                actual, reference.samples,
                "factor {factor}, high frequency {high_frequency}"
            );
        }
    }
}

fn scenario(factor: u8, callbacks: Vec<usize>, probe: &str) -> ChainScenario {
    let frames: usize = callbacks.iter().sum();
    ChainScenario {
        schema_version: 1,
        spec: ronie_dsp::PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: factor,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        callback_frames: callbacks,
        input: (0..frames * 2)
            .map(|index| ((index as f32 + 1.0) * 0.17).sin() * 0.5)
            .collect(),
        parameters: BTreeMap::new(),
        state_changes: Vec::new(),
        snapshot: json!({"probe": probe}),
    }
}

#[test]
#[ignore = "requires the private full-chain Reference runner"]
fn private_oversampling_and_ir_islands_match_every_factor() {
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    for factor in [2, 4, 8] {
        let round_trip = scenario(factor, vec![31, 33, 32], "oversampling");
        let reference = oracle.render(&round_trip).expect("Reference oversampling");
        let mut oversampler = ChainOversampler::new(factor, 64).expect("preparation");
        let mut actual = vec![0.0; 192];
        let mut offset = 0;
        for frames in [31, 33, 32] {
            let mut chunk_data = planar_chunk(&round_trip.input, 96, offset, frames);
            let input = PlanarBuffer::new(&mut chunk_data, 2, frames).expect("input");
            let mut chunk_output = vec![0.0; frames * 2];
            let mut output = PlanarBuffer::new(&mut chunk_output, 2, frames).expect("output");
            oversampler
                .process(&input, &mut output, identity)
                .expect("process");
            store_planar_chunk(&mut actual, &chunk_output, 96, offset, frames);
            offset += frames;
        }
        assert_eq!(actual, reference.samples, "round trip factor {factor}");

        let island_scenario = scenario(factor, vec![32, 64], "ir-island");
        let reference = oracle.render(&island_scenario).expect("Reference island");
        let mut oversampler = ChainOversampler::new(factor, 64).expect("island preparation");
        let mut actual = vec![0.0; 192];
        let mut offset = 0;
        for frames in [32, 64] {
            let mut chunk_data = planar_chunk(&island_scenario.input, 96, offset, frames);
            let mut island = PlanarBuffer::new(&mut chunk_data, 2, frames).expect("island");
            oversampler
                .base_rate_island(&mut island, identity)
                .expect("island process");
            store_planar_chunk(&mut actual, &chunk_data, 96, offset, frames);
            offset += frames;
        }
        assert_eq!(actual, reference.samples, "IR island factor {factor}");
    }
}
