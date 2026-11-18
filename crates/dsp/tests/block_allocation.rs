//! Prepared block allocation and invalid-callback tests.
#![allow(unsafe_code)]

use ronie_core::{AudioDocument, BlockParams, InputMode};
use ronie_dsp::{
    PlanarBuffer, PrepareSpec,
    block::{BlockControls, BlockModel, prepare_block},
    compile_model,
    ir::compile_ir,
};
use ronie_oracle::synthetic::synthetic_a2_document;

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

fn counts(operation: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    operation();
    (
        ALLOCATIONS.with(std::cell::Cell::get),
        DEALLOCATIONS.with(std::cell::Cell::get),
    )
}

#[test]
fn prepared_block_and_invalid_callbacks_do_not_allocate() {
    let compiled = compile_ir(
        &AudioDocument {
            sample_rate: 48_000.0,
            channels: vec![vec![1.0]],
        },
        None,
    )
    .expect("identity IR");
    let spec = PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    };
    let mut block = prepare_block(BlockModel::Ir(compiled), spec, BlockParams::default())
        .expect("block preparation");
    let mut valid = [0.25; 64];
    let mut oversized = [0.25; 65];
    let controls = BlockControls {
        spectrum_enabled: true,
        ..Default::default()
    };
    let mut valid_result = Ok(());
    let mut invalid_result = Ok(());

    let counts = counts(|| {
        valid_result = block.process(
            &mut PlanarBuffer::new(&mut valid, 1, 64).expect("valid"),
            controls,
        );
        invalid_result = block.process(
            &mut PlanarBuffer::new(&mut oversized, 1, 65).expect("oversized"),
            controls,
        );
    });

    assert!(valid_result.is_ok(), "{valid_result:?}");
    assert!(invalid_result.is_err());
    assert_eq!(counts, (0, 0));
}

#[test]
fn an_open_spectrum_tap_copies_block_output_with_no_allocation_and_then_closes() {
    let compiled = compile_ir(
        &AudioDocument {
            sample_rate: 48_000.0,
            channels: vec![vec![1.0]],
        },
        None,
    )
    .expect("identity IR");
    let spec = PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    };
    let mut block = prepare_block(BlockModel::Ir(compiled), spec, BlockParams::default())
        .expect("block preparation");
    let tap = block.spectrum_tap();
    let mut samples = [0.25; 64];
    let controls = BlockControls::default();

    let process = |block: &mut ronie_dsp::block::PreparedBlock, samples: &mut [f32; 64]| {
        block
            .process(
                &mut PlanarBuffer::new(samples, 1, 64).expect("buffer"),
                controls,
            )
            .expect("process");
    };
    process(&mut block, &mut samples);
    assert_eq!(tap.samples_written(), 0, "no page, no copy");

    tap.open();
    let counts = counts(|| process(&mut block, &mut samples));
    assert_eq!(counts, (0, 0));
    assert_eq!(tap.samples_written(), 64);

    let mut callbacks = 1;
    while tap.is_open() {
        process(&mut block, &mut samples);
        callbacks += 1;
        assert!(callbacks < 1_000, "the tap must close by itself");
    }
    let written = tap.samples_written();
    process(&mut block, &mut samples);
    assert_eq!(
        tap.samples_written(),
        written,
        "a closed page costs no copy"
    );
}

#[test]
fn nam_metadata_does_not_allocate_during_processing() {
    let mut document = synthetic_a2_document(3).expect("synthetic document");
    document
        .metadata
        .extra
        .insert("label".into(), serde_json::json!("heap-backed metadata"));
    let compiled = compile_model(&document).expect("compiled model");
    let spec = PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: true,
        stereo_output: true,
    };
    let mut block = prepare_block(BlockModel::Nam(compiled), spec, BlockParams::default())
        .expect("block preparation");
    let mut audio = [0.25; 64];
    let mut result = Ok(());

    let counts = counts(|| {
        result = block.process(
            &mut PlanarBuffer::new(&mut audio, 1, 64).expect("audio"),
            BlockControls::default(),
        );
    });

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(counts, (0, 0));
}

#[test]
fn the_pair_path_does_not_allocate() {
    let a = compile_ir(
        &AudioDocument {
            sample_rate: 44_100.0,
            channels: vec![(0..700).map(|index| 0.99_f32.powi(index)).collect()],
        },
        None,
    )
    .expect("mono IR");
    let b = compile_ir(
        &AudioDocument {
            sample_rate: 96_000.0,
            channels: vec![
                (0..3000).map(|index| 0.995_f32.powi(index)).collect(),
                (0..3000).map(|index| -0.995_f32.powi(index)).collect(),
            ],
        },
        None,
    )
    .expect("stereo IR");
    for oversampling in [1, 2, 8] {
        let spec = PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        };
        let mut block = prepare_block(
            BlockModel::IrPair {
                a: a.clone(),
                b: b.clone(),
            },
            spec,
            BlockParams::default(),
        )
        .expect("pair preparation");
        let frames = 64 * usize::from(oversampling);
        let mut stereo = vec![0.25; 2 * frames];
        let mut mono = vec![0.25; frames];
        let settings = [
            BlockControls::default(),
            BlockControls {
                ir_blend: 0.9,
                ir_a_level: 0.2,
                ir_b_level: 0.8,
                ..Default::default()
            },
            BlockControls {
                ir_blend: 0.1,
                ir_b_invert: true,
                dual_mono: true,
                ..Default::default()
            },
        ];
        let mut results = Vec::with_capacity(2 * settings.len());

        let counts = counts(|| {
            for controls in settings {
                results.push(block.process(
                    &mut PlanarBuffer::new(&mut stereo, 2, frames).expect("stereo"),
                    controls,
                ));
                results.push(block.process(
                    &mut PlanarBuffer::new(&mut mono, 1, frames).expect("mono"),
                    controls,
                ));
            }
        });

        assert!(results.iter().all(Result::is_ok), "{results:?}");
        assert_eq!(counts, (0, 0), "{oversampling}x");
    }
}
