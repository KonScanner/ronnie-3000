use ronie_core::ChainSide;

use crate::{
    ChainError, LatencyReport, PlanarBuffer,
    command::{ChainCommandReceiver, QueuedCommand, channel},
    input::InputControls,
    output::OutputControls,
    prepare::PreparedChainChange,
    retire::{RetirementPermit, RetirementQueue},
    routing::RoutingControls,
};

pub use crate::command::{ChainCommand, ChainCommandSender};

/// Complete controls sampled at one callback boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ChainControls {
    /// Input gain, gate, and pitch controls.
    pub input: InputControls,
    /// Lane and per-block routing controls.
    pub routing: RoutingControls,
    /// Image, tone, and main output controls.
    pub output: OutputControls,
}

/// The blocks that failed in a callback and now run as bypass, as one position bit in each lane.
///
/// A lane holds at most 64 blocks, so each position has a bit. A later position sets no bit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FailedBlocks {
    /// Positions in the left lane.
    pub left: u64,
    /// Positions in the right lane.
    pub right: u64,
}

impl FailedBlocks {
    /// Whether no block failed.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.left | self.right == 0
    }

    /// Each failed block, left lane first and in chain order.
    pub fn positions(self) -> impl Iterator<Item = (ChainSide, usize)> {
        let bits = |mask: u64, side| {
            (0..u64::BITS as usize)
                .filter(move |position| mask >> position & 1 == 1)
                .map(move |position| (side, position))
        };
        bits(self.left, ChainSide::Left).chain(bits(self.right, ChainSide::Right))
    }
}

struct ActiveChain {
    change: PreparedChainChange,
    _permit: RetirementPermit,
}

struct LaneStages<'a> {
    topology: &'a mut crate::topology::RtTopology,
    blocks: &'a mut crate::topology::LaneSet,
    auto_offset: &'a mut crate::analysis::AutoOffset,
    probe_scratch: &'a mut [f32],
}

/// What the lanes report to the chain after they process audio.
struct LaneFlags<'a> {
    processing_failed: &'a mut bool,
    failed_blocks: &'a mut FailedBlocks,
    non_finite_block: &'a mut Option<(ChainSide, usize)>,
}

/// Callback-exclusive owner of the active chain and bounded command transfer.
pub struct RtChain {
    active: ActiveChain,
    commands: ChainCommandReceiver,
    retirements: RetirementQueue,
    controls: ChainControls,
    processing_failed: bool,
    failed_blocks: FailedBlocks,
    output_sanitized: bool,
    non_finite_block: Option<(ChainSide, usize)>,
    worker_pool: Option<crate::rt::RtWorkerPool>,
}

impl RtChain {
    /// Publishes an initial chain and creates its bounded control endpoint.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error for a zero command capacity.
    pub fn prepare(
        initial: PreparedChainChange,
        command_capacity: usize,
    ) -> Result<(Self, ChainCommandSender), ChainError> {
        if command_capacity == 0 {
            return Err(ChainError::InvalidLayout(
                "command capacity must be nonzero",
            ));
        }
        let retirements = RetirementQueue::new(command_capacity);
        let permit = retirements.acquire().ok_or(ChainError::Preparation(
            "initial retirement budget is full".into(),
        ))?;
        let (sender, commands) = channel(
            command_capacity,
            &retirements.budget,
            retirements.retired_drain(),
        );
        Ok((
            Self {
                active: ActiveChain {
                    change: initial,
                    _permit: permit,
                },
                commands,
                retirements,
                controls: ChainControls::default(),
                processing_failed: false,
                failed_blocks: FailedBlocks::default(),
                output_sanitized: false,
                non_finite_block: None,
                worker_pool: None,
            },
            sender,
        ))
    }

    /// Current published generation.
    #[must_use]
    pub const fn active_generation(&self) -> u64 {
        self.active.change.generation
    }

    /// Current host-visible latency and tail report.
    #[must_use]
    pub const fn latency(&self) -> LatencyReport {
        self.active.change.latency
    }

    /// Computes the current host-visible bus, delay, and tail report.
    #[must_use]
    pub fn host_report(&self) -> crate::host::HostReport {
        let change = &self.active.change;
        let input_latency = change.input.latency_samples(self.controls.input);
        crate::host::HostReport::from_parts(
            crate::host::HostCapabilities::from_spec(change.spec),
            LatencyReport {
                samples: change.latency.samples.saturating_add(input_latency),
                tail_seconds: change.latency.tail_seconds
                    + input_latency as f64 / change.spec.host_sample_rate,
            },
        )
    }

    /// Shared callback controls.
    #[must_use]
    pub const fn controls(&self) -> ChainControls {
        self.controls
    }

    /// Replaces callback controls without allocating.
    pub fn set_controls(&mut self, controls: ChainControls) {
        self.controls = controls;
    }

    /// Installs or removes the prepared worker pool on the control thread.
    pub fn set_worker_pool(&mut self, pool: Option<crate::rt::RtWorkerPool>) {
        self.worker_pool = pool;
    }

    /// Whether a block failed and was disabled during a prior callback.
    #[must_use]
    pub const fn processing_failed(&self) -> bool {
        self.processing_failed
    }

    /// Clears and returns the preallocated processing-error flag off-thread.
    pub fn take_processing_failed(&mut self) -> bool {
        std::mem::take(&mut self.processing_failed)
    }

    /// Clears and returns whether the chain output needed a non-finite repair.
    pub fn take_output_sanitized(&mut self) -> bool {
        std::mem::take(&mut self.output_sanitized)
    }

    /// Clears and returns the first block that made non-finite samples in the latest callback.
    pub fn take_non_finite_block(&mut self) -> Option<(ChainSide, usize)> {
        self.non_finite_block.take()
    }

    /// Clears and returns the blocks that failed since the last take, without allocating.
    pub fn take_failed_blocks(&mut self) -> FailedBlocks {
        std::mem::take(&mut self.failed_blocks)
    }

    /// Visits the side, position, and input and output levels in dB of each running block.
    ///
    /// This does not allocate, so a callback can call it.
    pub fn visit_block_meters(&self, mut visit: impl FnMut(ChainSide, usize, f32, f32)) {
        let lanes = &self.active.change.blocks;
        lanes.left.visit_block_meters(|position, input, output| {
            visit(ChainSide::Left, position, input, output);
        });
        lanes.right.visit_block_meters(|position, input, output| {
            visit(ChainSide::Right, position, input, output);
        });
    }

    /// Applies every queued command and the newest controls at one audio-buffer boundary.
    pub fn apply_pending(&mut self) {
        while let Ok(queued) = self.commands.receiver.try_recv() {
            self.apply(queued);
        }
        while let Ok(controls) = self.commands.controls.try_recv() {
            self.controls = controls;
        }
    }

    /// Processes one complete callback without allocation or engine destruction.
    ///
    /// A non-finite sample never enters the chain and never leaves it. A repair at the
    /// output also clears the output-stage state and sets [`Self::take_output_sanitized`],
    /// so that the control side can replace any other poisoned state with a fresh chain.
    ///
    /// # Errors
    ///
    /// Returns an invalid-layout error before changing audio.
    pub fn process(&mut self, buffer: &mut PlanarBuffer<'_>) -> Result<(), ChainError> {
        let channels = buffer.channels();
        let frames = buffer.frames();
        self.apply_pending();
        self.non_finite_block = None;
        let spec = self.active.change.spec;
        if !matches!(channels, 1 | 2) || frames > spec.max_host_frames {
            return Err(ChainError::InvalidLayout(
                "invalid real-time chain callback",
            ));
        }
        sanitize_non_finite(buffer.as_mut_slice());
        let change = &mut self.active.change;
        let mut controls = self.controls;
        controls.routing.stereo_input = spec.stereo_input;
        controls.routing.stereo_output = spec.stereo_output && channels == 2;
        controls.output.image.stereo_rig = controls.routing.stereo_output;
        controls.output.image.stereo_chains = change.blocks.stereo_enabled;

        change.tuner.push_samples(buffer.channel(0)?);
        change.input.process(buffer, controls.input)?;
        let flags = &mut LaneFlags {
            processing_failed: &mut self.processing_failed,
            failed_blocks: &mut self.failed_blocks,
            non_finite_block: &mut self.non_finite_block,
        };
        let PreparedChainChange {
            resampler,
            topology,
            blocks,
            auto_offset,
            probe_scratch,
            host_scratch,
            mono_scratch,
            output: output_stage,
            auto_balance,
            ..
        } = &mut *change;
        let routing = controls.routing;
        let worker_pool = self.worker_pool.as_ref();

        if channels == 1 {
            host_scratch[..frames].copy_from_slice(buffer.channel(0)?);
            host_scratch[frames..2 * frames].copy_from_slice(buffer.channel(0)?);
            let input = PlanarBuffer::new(&mut host_scratch[..2 * frames], 2, frames)?;
            let mut output = PlanarBuffer::new(&mut mono_scratch[..2 * frames], 2, frames)?;
            resampler.process(&input, &mut output, |chain| {
                Self::process_lanes(
                    LaneStages {
                        topology,
                        blocks,
                        auto_offset,
                        probe_scratch,
                    },
                    flags,
                    chain,
                    routing,
                    worker_pool,
                )
            })?;
            output_stage.process(&mut output, controls.output, |left, right| {
                auto_balance.process(left, right);
            })?;
            auto_offset.apply_output_gain(&mut output)?;
            buffer
                .channel_mut(0)?
                .copy_from_slice(&mono_scratch[..frames]);
        } else {
            host_scratch[..2 * frames].copy_from_slice(buffer.as_slice());
            let input = PlanarBuffer::new(&mut host_scratch[..2 * frames], 2, frames)?;
            resampler.process(&input, buffer, |chain| {
                Self::process_lanes(
                    LaneStages {
                        topology,
                        blocks,
                        auto_offset,
                        probe_scratch,
                    },
                    flags,
                    chain,
                    routing,
                    worker_pool,
                )
            })?;
            output_stage.process(buffer, controls.output, |left, right| {
                auto_balance.process(left, right);
            })?;
            auto_offset.apply_output_gain(buffer)?;
        }
        if sanitize_non_finite(buffer.as_mut_slice()) {
            self.active.change.output.reset();
            self.output_sanitized = true;
        }
        Ok(())
    }

    fn process_lanes(
        stages: LaneStages<'_>,
        flags: &mut LaneFlags<'_>,
        buffer: &mut PlanarBuffer<'_>,
        controls: RoutingControls,
        pool: Option<&crate::rt::RtWorkerPool>,
    ) -> Result<(), ChainError> {
        let LaneStages {
            topology,
            blocks,
            auto_offset,
            probe_scratch,
        } = stages;
        let frames = buffer.frames();
        if auto_offset.render_probe_input(&mut probe_scratch[..frames]) {
            for channel in 0..2 {
                buffer
                    .channel_mut(channel)?
                    .copy_from_slice(&probe_scratch[..frames]);
            }
        }
        topology.process_with_pool(buffer, blocks, controls, pool)?;
        auto_offset.capture_chain_outputs(buffer.channel(0)?, buffer.channel(1)?);
        *flags.processing_failed |= blocks.left.take_processing_failed();
        *flags.processing_failed |= blocks.right.take_processing_failed();
        flags.failed_blocks.left |= blocks.left.take_failed_blocks();
        flags.failed_blocks.right |= blocks.right.take_failed_blocks();
        let left = blocks.left.take_non_finite_block();
        let right = blocks.right.take_non_finite_block();
        *flags.non_finite_block = flags
            .non_finite_block
            .or(left.map(|position| (ChainSide::Left, position)))
            .or(right.map(|position| (ChainSide::Right, position)));
        Ok(())
    }

    /// Drains pending and retired engines on the control thread, including stopped audio.
    pub fn drain_retirements_off_thread(&mut self) -> usize {
        while let Ok(queued) = self.commands.receiver.try_recv() {
            let QueuedCommand { command, _permit } = queued;
            let _ = self.retirements.retire(command, _permit);
        }
        self.retirements.drain()
    }

    fn apply(&mut self, queued: QueuedCommand) {
        let QueuedCommand { command, _permit } = queued;
        match command {
            ChainCommand::Replace(change) => {
                if change.generation <= self.active.change.generation {
                    let _ = self
                        .retirements
                        .retire(ChainCommand::Replace(change), _permit);
                    return;
                }
                let previous = std::mem::replace(&mut self.active, ActiveChain { change, _permit });
                let _ = self
                    .retirements
                    .retire(ChainCommand::Replace(previous.change), previous._permit);
            }
            ChainCommand::UpdateBlockParams(mut update) => {
                let lanes = &mut self.active.change.blocks;
                for (id, params) in &mut update {
                    if let Some(block) = lanes
                        .left
                        .blocks
                        .iter_mut()
                        .chain(&mut lanes.right.blocks)
                        .find(|block| block.id == *id)
                    {
                        std::mem::swap(&mut block.params, params);
                    }
                }
                let _ = self
                    .retirements
                    .retire(ChainCommand::UpdateBlockParams(update), _permit);
            }
        }
    }
}

/// Replaces every non-finite sample with silence and returns whether any was found.
fn sanitize_non_finite(samples: &mut [f32]) -> bool {
    let mut found = false;
    for sample in samples {
        if !sample.is_finite() {
            *sample = 0.0;
            found = true;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use ronie_core::{AudioDocument, ChainBlock, InputMode, ModelId, Tone};
    use serde_json::json;

    use super::*;
    use crate::{PrepareSpec, block::BlockModel, ir::compile_ir, lane::LaneSnapshot};

    #[test]
    fn model_failure_restores_dry_audio_and_disables_only_that_block() {
        let spec = PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: false,
            stereo_output: false,
        };
        let empty = || LaneSnapshot {
            blocks: vec![],
            models: vec![],
        };
        let report = LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        };
        let block = ChainBlock::tone(
            "identity",
            Tone::from_value(json!({
                "id": 1,
                "title": "Identity IR",
                "format": "ir",
                "models": [{"id": 1, "name": "Identity"}]
            }))
            .expect("tone"),
            ModelId(1),
        );
        let model = BlockModel::Ir(
            compile_ir(
                &AudioDocument {
                    sample_rate: 48_000.0,
                    channels: vec![vec![1.0]],
                },
                None,
            )
            .expect("identity IR"),
        );
        let failing = PreparedChainChange::new(
            1,
            spec,
            LaneSnapshot {
                blocks: vec![block],
                models: vec![Some(model)],
            },
            empty(),
            false,
            report,
        )
        .expect("failing chain preparation");
        let dry = PreparedChainChange::new(1, spec, empty(), empty(), false, report)
            .expect("dry chain preparation");
        let (mut failing_chain, _) = RtChain::prepare(failing, 1).expect("failing chain");
        let (mut dry_chain, _) = RtChain::prepare(dry, 1).expect("dry chain");
        let mut controls = ChainControls::default();
        controls.input.gate_enabled = false;
        failing_chain.set_controls(controls);
        dry_chain.set_controls(controls);
        failing_chain
            .active
            .change
            .blocks
            .left
            .fail_next_for_test(0);

        let mut actual = [0.25; 64];
        let mut expected = [0.25; 64];
        failing_chain
            .process(&mut PlanarBuffer::new(&mut actual, 1, 64).expect("actual buffer"))
            .expect("failure is contained");
        dry_chain
            .process(&mut PlanarBuffer::new(&mut expected, 1, 64).expect("expected buffer"))
            .expect("dry processing");
        assert_eq!(actual, expected);
        assert!(failing_chain.processing_failed());
        assert!(failing_chain.take_processing_failed());
        assert!(!failing_chain.take_processing_failed());

        actual.fill(0.25);
        expected.fill(0.25);
        failing_chain
            .process(&mut PlanarBuffer::new(&mut actual, 1, 64).expect("actual buffer"))
            .expect("disabled block processing");
        dry_chain
            .process(&mut PlanarBuffer::new(&mut expected, 1, 64).expect("expected buffer"))
            .expect("dry processing");
        assert_eq!(actual, expected);
        assert!(!failing_chain.processing_failed());
    }

    fn identity_block(id: &str) -> (ChainBlock, BlockModel) {
        let block = ChainBlock::tone(
            id,
            Tone::from_value(json!({
                "id": 1,
                "title": "Identity IR",
                "format": "ir",
                "models": [{"id": 1, "name": "Identity"}]
            }))
            .expect("tone"),
            ModelId(1),
        );
        let model = BlockModel::Ir(
            compile_ir(
                &AudioDocument {
                    sample_rate: 48_000.0,
                    channels: vec![vec![1.0]],
                },
                None,
            )
            .expect("identity IR"),
        );
        (block, model)
    }

    fn chain_with(left: LaneSnapshot, right: LaneSnapshot, stereo: bool) -> RtChain {
        let spec = PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: false,
            stereo_output: true,
        };
        let change = PreparedChainChange::new(
            1,
            spec,
            left,
            right,
            stereo,
            LatencyReport {
                samples: 0,
                tail_seconds: 0.0,
            },
        )
        .expect("chain preparation");
        let (mut chain, _) = RtChain::prepare(change, 1).expect("chain");
        let mut controls = ChainControls::default();
        controls.input.gate_enabled = false;
        chain.set_controls(controls);
        chain
    }

    /// One lane with an open slot, a clean block, and a block that makes NaN samples.
    fn lane_with_bad_block(prefix: &str) -> LaneSnapshot {
        let (clean, clean_model) = identity_block(&format!("{prefix}-clean"));
        let (mut bad, bad_model) = identity_block(&format!("{prefix}-bad"));
        bad.params.output_gain = f64::NAN;
        LaneSnapshot {
            blocks: vec![ChainBlock::insert(format!("{prefix}-slot")), clean, bad],
            models: vec![None, Some(clean_model), Some(bad_model)],
        }
    }

    fn empty_lane() -> LaneSnapshot {
        LaneSnapshot {
            blocks: vec![],
            models: vec![],
        }
    }

    fn process_stereo(chain: &mut RtChain) {
        let mut samples = [0.25; 128];
        chain
            .process(&mut PlanarBuffer::new(&mut samples, 2, 64).expect("buffer"))
            .expect("process");
        assert!(samples.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn a_block_that_makes_nan_is_named_by_its_position_in_the_lane() {
        let mut chain = chain_with(lane_with_bad_block("left"), empty_lane(), false);

        process_stereo(&mut chain);

        assert!(chain.take_output_sanitized());
        assert_eq!(chain.take_non_finite_block(), Some((ChainSide::Left, 2)));
        assert_eq!(chain.take_non_finite_block(), None);
    }

    #[test]
    fn a_block_in_the_right_lane_is_named_with_its_side() {
        let (clean, clean_model) = identity_block("left-clean");
        let left = LaneSnapshot {
            blocks: vec![clean],
            models: vec![Some(clean_model)],
        };
        let mut chain = chain_with(left, lane_with_bad_block("right"), true);

        process_stereo(&mut chain);

        assert!(chain.take_output_sanitized());
        assert_eq!(chain.take_non_finite_block(), Some((ChainSide::Right, 2)));
    }

    #[test]
    fn clean_blocks_name_no_block() {
        let (clean, clean_model) = identity_block("clean");
        let left = LaneSnapshot {
            blocks: vec![clean],
            models: vec![Some(clean_model)],
        };
        let mut chain = chain_with(left, empty_lane(), false);

        process_stereo(&mut chain);

        assert!(!chain.take_output_sanitized());
        assert_eq!(chain.take_non_finite_block(), None);
    }

    #[test]
    fn a_block_that_gets_nan_at_its_input_is_not_to_blame() {
        let (clean, clean_model) = identity_block("clean");
        let left = LaneSnapshot {
            blocks: vec![clean],
            models: vec![Some(clean_model)],
        };
        let mut chain = chain_with(left, empty_lane(), false);
        let mut controls = chain.controls();
        controls.input.input_gain = f32::INFINITY;
        chain.set_controls(controls);
        // Infinity times the silence of the first sample of a fade is not a number.
        let mut samples = [0.0; 128];

        chain
            .process(&mut PlanarBuffer::new(&mut samples, 2, 64).expect("buffer"))
            .expect("process");

        assert!(chain.take_output_sanitized());
        assert_eq!(chain.take_non_finite_block(), None);
    }

    fn lane_of_two_blocks(prefix: &str) -> LaneSnapshot {
        let (first, first_model) = identity_block(&format!("{prefix}-first"));
        let (second, second_model) = identity_block(&format!("{prefix}-second"));
        LaneSnapshot {
            blocks: vec![ChainBlock::insert(format!("{prefix}-slot")), first, second],
            models: vec![None, Some(first_model), Some(second_model)],
        }
    }

    fn failed_positions(chain: &mut RtChain) -> Vec<(ChainSide, usize)> {
        chain.take_failed_blocks().positions().collect()
    }

    #[test]
    fn a_failed_block_is_named_by_its_position_in_the_lane() {
        let mut chain = chain_with(lane_of_two_blocks("left"), empty_lane(), false);
        chain.active.change.blocks.left.fail_next_for_test(2);

        process_stereo(&mut chain);

        assert!(chain.take_processing_failed());
        assert_eq!(failed_positions(&mut chain), [(ChainSide::Left, 2)]);
        assert!(chain.take_failed_blocks().is_empty(), "the take clears it");

        process_stereo(&mut chain);
        assert!(!chain.take_processing_failed(), "the block is bypassed now");
        assert!(chain.take_failed_blocks().is_empty());
    }

    #[test]
    fn every_block_that_fails_in_one_callback_is_named() {
        let mut chain = chain_with(
            lane_of_two_blocks("left"),
            lane_of_two_blocks("right"),
            true,
        );
        let lanes = &mut chain.active.change.blocks;
        lanes.left.fail_next_for_test(1);
        lanes.left.fail_next_for_test(2);
        lanes.right.fail_next_for_test(2);

        process_stereo(&mut chain);

        assert_eq!(
            failed_positions(&mut chain),
            [
                (ChainSide::Left, 1),
                (ChainSide::Left, 2),
                (ChainSide::Right, 2)
            ]
        );
    }

    #[test]
    fn a_failure_in_a_later_callback_keeps_an_earlier_failure_that_nobody_took() {
        let mut chain = chain_with(lane_of_two_blocks("left"), empty_lane(), false);
        chain.active.change.blocks.left.fail_next_for_test(1);
        process_stereo(&mut chain);
        chain.active.change.blocks.left.fail_next_for_test(2);
        process_stereo(&mut chain);

        assert_eq!(
            failed_positions(&mut chain),
            [(ChainSide::Left, 1), (ChainSide::Left, 2)]
        );
    }

    #[test]
    fn block_settings_change_without_a_new_chain_and_the_old_settings_retire() {
        let spec = PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: false,
            stereo_output: false,
        };
        let block = ChainBlock::tone(
            "identity",
            Tone::from_value(json!({
                "id": 1,
                "title": "Identity IR",
                "format": "ir",
                "models": [{"id": 1, "name": "Identity"}]
            }))
            .expect("tone"),
            ModelId(1),
        );
        let id = block.id.clone();
        let model = BlockModel::Ir(
            compile_ir(
                &AudioDocument {
                    sample_rate: 48_000.0,
                    channels: vec![vec![1.0]],
                },
                None,
            )
            .expect("identity IR"),
        );
        let change = PreparedChainChange::new(
            4,
            spec,
            LaneSnapshot {
                blocks: vec![block.clone()],
                models: vec![Some(model)],
            },
            LaneSnapshot {
                blocks: vec![],
                models: vec![],
            },
            false,
            LatencyReport {
                samples: 0,
                tail_seconds: 0.0,
            },
        )
        .expect("chain preparation");
        let (mut chain, sender) = RtChain::prepare(change, 1).expect("chain");
        let mut params = block.params.clone();
        params.mix = 0.25;
        sender
            .try_send(ChainCommand::UpdateBlockParams(vec![(id.clone(), params)]))
            .expect("queue settings");
        assert_eq!(sender.drain_retired(), 0);

        chain.apply_pending();

        assert_eq!(chain.active_generation(), 4);
        let active = chain
            .active
            .change
            .blocks
            .left
            .position(&id)
            .expect("block");
        assert_eq!(
            chain.active.change.blocks.left.blocks[active].params.mix,
            0.25
        );
        assert_eq!(sender.drain_retired(), 1);
    }
}
