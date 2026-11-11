//! Repeated preparation, replacement, retirement, and worker lifecycle stress.

use ronie_core::InputMode;
use ronie_dsp::{
    LatencyReport, PlanarBuffer, PrepareSpec,
    lane::LaneSnapshot,
    prepare::PreparedChainChange,
    rt::RtWorkerPool,
    rt_chain::{ChainCommand, RtChain},
};

fn change(generation: u64) -> PreparedChainChange {
    let empty = || LaneSnapshot {
        blocks: vec![],
        models: vec![],
    };
    PreparedChainChange::new(
        generation,
        PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        empty(),
        empty(),
        false,
        LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .expect("chain preparation")
}

#[test]
fn repeated_publication_never_applies_stale_generations() {
    let (mut chain, sender) = RtChain::prepare(change(1), 2).expect("real-time chain");
    let mut controls = chain.controls();
    controls.input.gate_enabled = false;
    chain.set_controls(controls);
    let expected = [0.0; 128];
    let baseline = [0.0; 128];
    for generation in 2..42 {
        sender
            .try_send(ChainCommand::Replace(change(generation)))
            .expect("queue current generation");
        sender
            .try_send(ChainCommand::Replace(change(generation - 1)))
            .expect("queue stale generation");
        let mut data = [0.0; 128];
        chain
            .process(&mut PlanarBuffer::new(&mut data, 2, 64).expect("audio buffer"))
            .expect("processing");
        assert_eq!(chain.active_generation(), generation);
        assert_eq!(data, baseline);
        assert_eq!(expected, baseline);
        assert!(chain.drain_retirements_off_thread() >= 2);
    }
    chain.drain_retirements_off_thread();
}

#[test]
fn worker_prepare_release_churn_remains_bounded() {
    for _ in 0..20 {
        let mut pool = RtWorkerPool::prepare(3, 16).expect("worker pool");
        pool.set_enabled(false);
        pool.set_enabled(true);
        pool.release();
    }
}
