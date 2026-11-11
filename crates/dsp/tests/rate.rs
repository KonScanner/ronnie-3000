//! Host-rate boundary, streaming, and allocation tests.
#![allow(unsafe_code)]

use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use ronie_core::InputMode;
use ronie_dsp::{ChainError, PlanarBuffer, rate::ChainBoundaryResampler};
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

fn allocation_counts(operation: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    operation();
    (
        ALLOCATIONS.with(std::cell::Cell::get),
        DEALLOCATIONS.with(std::cell::Cell::get),
    )
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
fn direct_48k_boundary_is_transparent_and_reports_zero_latency() {
    let mut boundary = ChainBoundaryResampler::new();
    boundary.prepare(48_000.0, 2, 3).expect("preparation");
    let mut input_data = [0.25, -0.5, 0.75, 1.0, -1.0, 0.5];
    let input = PlanarBuffer::new(&mut input_data, 2, 3).expect("input");
    let mut output_data = [0.0; 6];
    let mut output = PlanarBuffer::new(&mut output_data, 2, 3).expect("output");

    boundary
        .process(&input, &mut output, identity)
        .expect("direct process");

    assert_eq!(output_data, input_data);
    assert_eq!(boundary.latency_samples(), 0);
}

#[test]
fn invalid_calls_are_atomic_and_prepared_callbacks_do_not_allocate() {
    let _guard = lock();
    let mut boundary = ChainBoundaryResampler::new();
    let mut control = ChainBoundaryResampler::new();
    boundary.prepare(44_100.0, 2, 3).expect("preparation");
    control.prepare(44_100.0, 2, 3).expect("preparation");
    let mut input_data = [0.25, -0.5, 0.75, 1.0, -1.0, 0.5];
    let input = PlanarBuffer::new(&mut input_data, 2, 3).expect("input");
    let mut invalid_data = [7.0; 3];
    let mut invalid = PlanarBuffer::new(&mut invalid_data, 1, 3).expect("invalid output");
    let mut actual_data = [0.0; 6];
    let mut actual = PlanarBuffer::new(&mut actual_data, 2, 3).expect("output");
    let mut expected_data = [0.0; 6];
    let mut expected = PlanarBuffer::new(&mut expected_data, 2, 3).expect("control");
    let mut invalid_result = Ok(());
    let mut valid_result = Ok(());

    let counts = allocation_counts(|| {
        invalid_result = boundary.process(&input, &mut invalid, identity);
        valid_result = boundary.process(&input, &mut actual, identity);
    });
    control
        .process(&input, &mut expected, identity)
        .expect("control");

    assert!(matches!(invalid_result, Err(ChainError::InvalidLayout(_))));
    assert!(valid_result.is_ok(), "{valid_result:?}");
    assert_eq!(invalid_data, [7.0; 3]);
    assert_eq!(actual_data, expected_data);
    assert_eq!(counts, (0, 0));
}

#[test]
fn non_divisible_callbacks_match_a_whole_render() {
    let mut input_data: Vec<f32> = (0..192)
        .map(|index| ((index as f32 + 1.0) * 0.17).sin() * 0.5)
        .collect();
    let mut whole = ChainBoundaryResampler::new();
    whole.prepare(44_100.0, 2, 96).expect("whole preparation");
    let whole_input = PlanarBuffer::new(&mut input_data, 2, 96).expect("whole input");
    let mut expected = vec![0.0; 192];
    whole
        .process(
            &whole_input,
            &mut PlanarBuffer::new(&mut expected, 2, 96).expect("whole output"),
            identity,
        )
        .expect("whole render");

    let mut chunked = ChainBoundaryResampler::new();
    chunked
        .prepare(44_100.0, 2, 33)
        .expect("chunked preparation");
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

    assert_eq!(actual, expected);
}

fn scenario(host_rate: f64, callbacks: Vec<usize>) -> ChainScenario {
    let frames: usize = callbacks.iter().sum();
    ChainScenario {
        schema_version: 1,
        spec: ronie_dsp::PrepareSpec {
            host_sample_rate: host_rate,
            max_host_frames: 64,
            oversampling: 1,
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
        snapshot: json!({"probe": "boundary"}),
    }
}

#[test]
#[ignore = "requires the private full-chain Reference runner"]
fn private_oversampling_engages_without_changing_host_latency() {
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let mut factor_two = scenario(44_100.0, vec![31, 33, 32]);
    factor_two.snapshot = json!({});
    factor_two.spec.oversampling = 2;
    let mut factor_eight = factor_two.clone();
    factor_eight.spec.oversampling = 8;

    let two = oracle.render(&factor_two).expect("2x full-chain render");
    let eight = oracle.render(&factor_eight).expect("8x full-chain render");

    assert_eq!(two.latency.samples, eight.latency.samples);
    assert_ne!(
        two.samples, eight.samples,
        "oversampling must actually engage"
    );
}

#[test]
#[ignore = "requires the private full-chain Reference runner"]
fn private_boundary_matches_44k1_and_96k_streams() {
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    for host_rate in [44_100.0, 96_000.0] {
        let scenario = scenario(host_rate, vec![31, 33, 32]);
        let reference = oracle.render(&scenario).expect("Reference boundary");
        let mut boundary = ChainBoundaryResampler::new();
        boundary.prepare(host_rate, 2, 64).expect("preparation");
        let mut actual = vec![0.0; 192];
        let mut offset = 0;
        for frames in [31, 33, 32] {
            let mut chunk_data = planar_chunk(&scenario.input, 96, offset, frames);
            let input = PlanarBuffer::new(&mut chunk_data, 2, frames).expect("input");
            let mut chunk_output = vec![0.0; frames * 2];
            let mut output = PlanarBuffer::new(&mut chunk_output, 2, frames).expect("output");
            boundary
                .process(&input, &mut output, identity)
                .expect("process");
            store_planar_chunk(&mut actual, &chunk_output, 96, offset, frames);
            offset += frames;
        }

        assert_eq!(boundary.latency_samples(), reference.latency.samples);
        let max_error = actual
            .iter()
            .zip(&reference.samples)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_error <= 1.0e-6,
            "host {host_rate}: max error {max_error}"
        );

        let mut impulse = self::scenario(host_rate, vec![96]);
        impulse.input.fill(0.0);
        impulse.input[0] = 1.0;
        impulse.input[96] = 1.0;
        let reference = oracle.render(&impulse).expect("Reference impulse");
        let mut boundary = ChainBoundaryResampler::new();
        boundary
            .prepare(host_rate, 2, 96)
            .expect("impulse preparation");
        let input = PlanarBuffer::new(&mut impulse.input, 2, 96).expect("impulse input");
        let mut actual = vec![0.0; 192];
        let mut output = PlanarBuffer::new(&mut actual, 2, 96).expect("impulse output");
        boundary
            .process(&input, &mut output, identity)
            .expect("impulse process");
        let max_error = actual
            .iter()
            .zip(&reference.samples)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_error <= 1.0e-6,
            "host {host_rate} impulse: max error {max_error}"
        );
    }
}
