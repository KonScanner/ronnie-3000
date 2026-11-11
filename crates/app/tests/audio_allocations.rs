//! Isolated real-time allocation and callback-thread affinity tests.

#![allow(unsafe_code)]

use ronie_app::audio::{
    AudioBackendKind, AudioError, AudioEvent, AudioWorker, DeviceConfig, NonFiniteReport,
};
use ronie_dsp::rt_chain::ChainControls;

struct CountingAllocator;

static ALLOCATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // SAFETY: delegates to the system allocator with the original layout.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: delegates to the system allocator with the original layout.
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn identity_block(id: &str) -> ronie_core::ChainBlock {
    let tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 1,
        "title": "Identity IR",
        "format": "ir",
        "models": [{"id": 1, "name": "Identity"}],
    }))
    .expect("tone");
    ronie_core::ChainBlock::tone(id, tone, ronie_core::ModelId(1))
}

fn chain_with_a_block_that_makes_nan() -> ronie_dsp::prepare::PreparedChainChange {
    let mut block = identity_block("bad");
    block.params.output_gain = f64::NAN;
    chain_of(1, vec![block])
}

fn chain_of(
    generation: u64,
    blocks: Vec<ronie_core::ChainBlock>,
) -> ronie_dsp::prepare::PreparedChainChange {
    let model = ronie_dsp::ir::compile_ir(
        &ronie_core::AudioDocument {
            sample_rate: 48_000.0,
            channels: vec![vec![1.0]],
        },
        None,
    )
    .expect("identity IR");
    let models = blocks
        .iter()
        .map(|_| Some(ronie_dsp::block::BlockModel::Ir(model.clone())))
        .collect();
    let empty = ronie_dsp::lane::LaneSnapshot {
        blocks: Vec::new(),
        models: Vec::new(),
    };
    ronie_dsp::prepare::PreparedChainChange::new(
        generation,
        ronie_dsp::PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            oversampling: 1,
            input_mode: ronie_core::InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        },
        ronie_dsp::lane::LaneSnapshot { blocks, models },
        empty,
        false,
        ronie_dsp::LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("chain preparation")
}

/// The counter is global, so that this runs inside the only test of this file.
fn a_route_a_channel_level_and_a_test_tone_have_no_allocations() {
    let config = DeviceConfig {
        backend: AudioBackendKind::Dummy,
        sample_rate: Some(48_000),
        buffer_frames: Some(128),
        input_channels: 1,
        output_channels: 2,
        input_route: [2, 2],
        ..DeviceConfig::default()
    };
    let mut worker = AudioWorker::new(&config).expect("worker");
    let telemetry = worker.telemetry();
    let capture = [0.1_f32; 128 * 4];
    let mut output = [0.0_f32; 128 * 4];
    let wide_capture = [0.1_f32; 128 * 40];
    let mut stereo_output = [0.0_f32; 256];
    // The first call binds the thread of the callback.
    worker
        .process_interleaved(&capture, 4, &mut output, 4)
        .expect("first callback");

    telemetry.request_test_tone();
    telemetry.open_tuner_tap();
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    for callback in 0..800 {
        // A new route and a new pair arrive between two callbacks.
        telemetry.set_input_route(callback % 4, (callback + 1) % 4);
        telemetry.set_output_first(if callback % 2 == 0 { 0 } else { 2 });
        worker
            .process_interleaved(&capture, 4, &mut output, 4)
            .expect("callback");
    }
    worker
        .process_interleaved(&wide_capture, 40, &mut stereo_output, 2)
        .expect("a device with more channels than the meters");
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(
        !telemetry.test_tone_active(),
        "the tone ended during this test"
    );
}

#[test]
fn prepared_callbacks_have_no_allocations_or_ui_thread_calls() {
    a_route_a_channel_level_and_a_test_tone_have_no_allocations();
    let config = DeviceConfig {
        backend: AudioBackendKind::Dummy,
        input_device: Some("guitar".to_owned()),
        output_device: Some("monitors".to_owned()),
        sample_rate: Some(48_000),
        buffer_frames: Some(128),
        input_channels: 1,
        output_channels: 2,
        ..DeviceConfig::default()
    };
    let mut worker = AudioWorker::new(&config).expect("worker");
    let input = [0.1_f32; 128];
    let mut output = [0.0_f32; 256];
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    worker
        .process_interleaved(&input, 1, &mut output, 2)
        .expect("prepared callback");
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Control snapshots, an oversized host buffer, non-finite samples, and a malformed buffer.
    worker
        .command_sender()
        .try_send_controls(ChainControls::default())
        .expect("queue controls");
    let oversized_input = vec![f32::NAN; 300];
    let mut oversized_output = vec![0.0_f32; 600];
    let mut malformed_output = vec![0.0_f32; 3];
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    worker
        .process_interleaved(&oversized_input, 1, &mut oversized_output, 2)
        .expect("oversized callback");
    assert!(matches!(
        worker.process_interleaved(&oversized_input, 1, &mut malformed_output, 2),
        Err(AudioError::InvalidCallback(_))
    ));
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);

    // A poisoned output runs the repair path, which clears the output stage.
    let mut poison = ChainControls::default();
    poison.input.input_gain = f32::INFINITY;
    worker
        .command_sender()
        .try_send_controls(poison)
        .expect("queue poison");
    let finite_input = [0.1_f32; 128];
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    worker
        .process_interleaved(&finite_input, 1, &mut output, 2)
        .expect("poisoned callback");
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(worker.telemetry().last_event() == Some(AudioEvent::NonFiniteOutput));

    // A block that makes non-finite samples: the callback must find and publish it.
    let mut blamed = AudioWorker::with_chain(
        &config,
        chain_with_a_block_that_makes_nan(),
        ChainControls::default(),
    )
    .expect("worker with a block");
    let finite_input = [0.1_f32; 128];
    let mut output = [0.0_f32; 256];
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    blamed
        .process_interleaved(&finite_input, 1, &mut output, 2)
        .expect("callback with a bad block");
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(blamed.telemetry().last_event() == Some(AudioEvent::NonFiniteOutput));
    assert_eq!(
        blamed.telemetry().non_finite_report(),
        NonFiniteReport {
            generation: 1,
            block: Some((ronie_core::ChainSide::Left, 0)),
        }
    );

    // A block that fails in a callback: the chain disables it and the callback names it.
    let mut chain = chain_of(1, vec![identity_block("first"), identity_block("second")]);
    chain.blocks.left.fail_next_for_test(1);
    let mut failing = AudioWorker::with_chain(&config, chain, ChainControls::default())
        .expect("worker with a block");
    let telemetry = failing.telemetry();
    let finite_input = [0.1_f32; 128];
    let mut output = [0.0_f32; 256];
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    failing
        .process_interleaved(&finite_input, 1, &mut output, 2)
        .expect("callback with a failed block");
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(telemetry.last_event(), Some(AudioEvent::ProcessingFailed));
    let report = telemetry.failed_blocks();
    assert_eq!(report.generation, 1);
    assert_eq!(
        report.blocks.positions().collect::<Vec<_>>(),
        [(ronie_core::ChainSide::Left, 1)]
    );

    // The bypassed block does not fail again, and the report stays.
    assert!(
        telemetry
            .take_events()
            .contains(AudioEvent::ProcessingFailed)
    );
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    failing
        .process_interleaved(&finite_input, 1, &mut output, 2)
        .expect("callback after the failure");
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(telemetry.last_event(), None);
    assert_eq!(telemetry.failed_blocks(), report);

    // A new chain replaces the bits of the old chain, and its own failure also allocates nothing.
    let mut next = chain_of(2, vec![identity_block("first"), identity_block("second")]);
    next.blocks.left.fail_next_for_test(0);
    failing
        .command_sender()
        .try_send(ronie_dsp::rt_chain::ChainCommand::Replace(next))
        .expect("queue the chain");
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    failing
        .process_interleaved(&finite_input, 1, &mut output, 2)
        .expect("callback on the new chain");
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(telemetry.active_generation(), 2);
    let report = telemetry.failed_blocks();
    assert_eq!(report.generation, 2);
    assert_eq!(
        report.blocks.positions().collect::<Vec<_>>(),
        [(ronie_core::ChainSide::Left, 0)]
    );

    // An open tuner tap copies the raw input and allocates nothing.
    worker.telemetry().open_tuner_tap();
    let telemetry = worker.telemetry();
    let finite_input = [0.1_f32; 128];
    let mut output = [0.0_f32; 256];
    ALLOCATIONS.store(0, std::sync::atomic::Ordering::SeqCst);
    worker
        .process_interleaved(&finite_input, 1, &mut output, 2)
        .expect("callback with a tuner reader");
    assert_eq!(ALLOCATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(telemetry.tuner_samples_written(), 128);

    let mut second = worker;
    let result = std::thread::spawn(move || {
        let input = [0.1_f32; 128];
        let mut output = [0.0_f32; 256];
        second.process_interleaved(&input, 1, &mut output, 2)
    })
    .join()
    .expect("callback thread joins");
    assert!(matches!(result, Err(AudioError::WrongThread)));
}
