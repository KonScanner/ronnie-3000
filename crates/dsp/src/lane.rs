use crate::{ChainError, PlanarBuffer};
use crate::{
    PrepareSpec,
    block::{BlockControls, BlockModel, PreparedBlock, prepare_block},
};
use ronie_core::{BlockId, ChainBlock};

/// Immutable model and settings for one lane.
#[derive(Clone)]
pub struct LaneSnapshot {
    /// Ordered block settings and identities.
    pub blocks: Vec<ChainBlock>,
    /// Compiled models in the same order; insert slots contain `None`.
    pub models: Vec<Option<BlockModel>>,
}

/// Real-time lane state with independent processors and scratch.
pub struct Lane {
    /// Current ordered control snapshot.
    pub blocks: Vec<ChainBlock>,
    processors: Vec<(BlockId, PreparedBlock)>,
    /// Lane-owned scratch storage, never shared with another lane.
    pub scratch: Vec<f32>,
    processing_failed: bool,
    /// One bit for the position of each block that failed since the last take.
    failed_blocks: u64,
    non_finite_block: Option<usize>,
}

impl Lane {
    /// Prepares every processor from an immutable snapshot off-thread.
    ///
    /// # Errors
    ///
    /// Returns model or preparation errors for inconsistent snapshots.
    pub fn from_snapshot(snapshot: LaneSnapshot, spec: PrepareSpec) -> Result<Self, ChainError> {
        if snapshot.blocks.len() != snapshot.models.len() {
            return Err(ChainError::Preparation(
                "lane block/model counts differ".into(),
            ));
        }
        let mut processors = Vec::with_capacity(snapshot.blocks.len());
        for (block, model) in snapshot.blocks.iter().zip(snapshot.models) {
            if !block.is_insert {
                let model = model.ok_or_else(|| {
                    ChainError::Preparation("tone block has no compiled model".into())
                })?;
                processors.push((
                    block.id.clone(),
                    prepare_block(model, spec, block.params.clone())?,
                ));
            }
        }
        Ok(Self {
            scratch: vec![0.0; spec.max_host_frames],
            blocks: snapshot.blocks,
            processors,
            processing_failed: false,
            failed_blocks: 0,
            non_finite_block: None,
        })
    }

    /// Captures the current settings without sharing mutable processing state.
    #[must_use]
    pub fn snapshot(&self) -> LaneSnapshot {
        LaneSnapshot {
            blocks: self.blocks.clone(),
            models: self
                .blocks
                .iter()
                .map(|block| {
                    if block.is_insert {
                        None
                    } else {
                        self.processors
                            .iter()
                            .find(|(id, _)| *id == block.id)
                            .map(|(_, processor)| processor.model_clone())
                    }
                })
                .collect(),
        }
    }

    /// Processes one inclusive block range without allocating.
    ///
    /// # Errors
    ///
    /// Returns block processing or stale-identity errors.
    pub fn process_range(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        start: usize,
        end: usize,
        controls: BlockControls,
    ) -> Result<(), ChainError> {
        self.process_range_with_pool(buffer, start, end, controls, None)
    }

    /// Processes a block range with optional phase/voice scheduling.
    ///
    /// # Errors
    ///
    /// Returns block processing or stale-identity errors.
    pub fn process_range_with_pool(
        &mut self,
        buffer: &mut PlanarBuffer<'_>,
        start: usize,
        end: usize,
        controls: BlockControls,
        pool: Option<&crate::rt::RtWorkerPool>,
    ) -> Result<(), ChainError> {
        if start > end || end > self.blocks.len() {
            return Err(ChainError::InvalidLayout("block range is out of bounds"));
        }
        // A block is to blame only for what was finite at its input.
        let mut clean = self.non_finite_block.is_none() && all_finite(buffer.as_slice());
        for (offset, block) in self.blocks[start..end].iter().enumerate() {
            if block.is_insert {
                continue;
            }
            let processor = self
                .processors
                .iter_mut()
                .find(|(id, _)| *id == block.id)
                .ok_or(ChainError::Processing("block processor is stale"))?;
            if let Err(error) =
                processor
                    .1
                    .process_with_pool(buffer, controls.with_params(&block.params), pool)
            {
                if !matches!(error, ChainError::Processing(_)) {
                    return Err(error);
                }
                processor.1.disable_and_restore_dry(buffer);
                self.processing_failed = true;
                self.failed_blocks |= 1_u64
                    .checked_shl((start + offset) as u32)
                    .unwrap_or_default();
            }
            if clean && !all_finite(buffer.as_slice()) {
                self.non_finite_block = Some(start + offset);
                clean = false;
            }
        }
        Ok(())
    }

    /// Each block with a processor and the lock-free output of that block.
    pub fn spectrum_taps(
        &self,
    ) -> impl Iterator<Item = (&BlockId, std::sync::Arc<crate::block::SpectrumTap>)> {
        self.processors
            .iter()
            .map(|(id, processor)| (id, processor.spectrum_tap()))
    }

    /// Visits the position and the input and output levels in dB of each block with a processor.
    ///
    /// This does not allocate, so a callback can call it.
    pub fn visit_block_meters(&self, mut visit: impl FnMut(usize, f32, f32)) {
        for (position, block) in self.blocks.iter().enumerate() {
            if block.is_insert {
                continue;
            }
            if let Some((_, processor)) = self.processors.iter().find(|(id, _)| *id == block.id) {
                visit(
                    position,
                    processor.input_meter_db(),
                    processor.output_meter_db(),
                );
            }
        }
    }

    /// Returns and clears whether a processor failed during the last callback.
    pub fn take_processing_failed(&mut self) -> bool {
        std::mem::take(&mut self.processing_failed)
    }

    /// Returns and clears the position of the first block that made non-finite samples.
    pub fn take_non_finite_block(&mut self) -> Option<usize> {
        self.non_finite_block.take()
    }

    /// Returns and clears the positions of the blocks that failed, as one bit for each block.
    pub fn take_failed_blocks(&mut self) -> u64 {
        std::mem::take(&mut self.failed_blocks)
    }

    /// Makes the block at one position fail in its next callback.
    ///
    /// No model can fail on request, so a test of the failure path needs this.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn fail_next_for_test(&mut self, position: usize) {
        let id = &self.blocks[position].id;
        if let Some((_, processor)) = self.processors.iter_mut().find(|(other, _)| other == id) {
            processor.fail_next_for_test();
        }
    }

    /// Finds a block position by ID.
    #[must_use]
    pub fn position(&self, id: &BlockId) -> Option<usize> {
        self.blocks.iter().position(|block| block.id == *id)
    }

    /// Ensures the five-slot minimum and at least one insert.
    pub fn normalize_inserts(&mut self) {
        let tones = self.blocks.iter().filter(|block| !block.is_insert).count();
        let required = (5_usize.saturating_sub(tones)).max(1);
        let mut inserts = self.blocks.iter().filter(|block| block.is_insert).count();
        while inserts > required {
            if let Some(index) = self.blocks.iter().rposition(|block| block.is_insert) {
                self.blocks.remove(index);
                inserts -= 1;
            } else {
                break;
            }
        }
        while inserts < required {
            self.blocks.push(ChainBlock::insert(unique_block_id().0));
            inserts += 1;
        }
    }

    /// Number of trailing insert slots that can be removed without losing the last insert.
    pub fn trim_trailing_inserts_to(&mut self, target_slots: usize) {
        let mut inserts = self.blocks.iter().filter(|block| block.is_insert).count();
        while self.blocks.len() > target_slots && inserts > 1 {
            if self.blocks.last().is_some_and(|block| block.is_insert) {
                self.blocks.pop();
                inserts -= 1;
            } else {
                break;
            }
        }
    }

    /// Inserts or replaces a block at a clamped index and returns its position.
    pub fn land_block(&mut self, block: ChainBlock, processor: PreparedBlock, index: usize) {
        let index = index.min(self.blocks.len());
        if index < self.blocks.len() && self.blocks[index].is_insert {
            self.blocks[index] = block.clone();
        } else {
            self.blocks.insert(index, block.clone());
        }
        self.processors.push((block.id, processor));
        self.normalize_inserts();
    }

    /// Removes a block by ID and returns it with its model for undo history.
    pub fn remove(&mut self, id: &BlockId) -> Option<(ChainBlock, PreparedBlock)> {
        let block_index = self.position(id)?;
        if self.blocks[block_index].is_insert {
            return None;
        }
        let block = self.blocks.remove(block_index);
        let processor_index = self
            .processors
            .iter()
            .position(|(processor_id, _)| processor_id == id)?;
        let processor = self.processors.remove(processor_index).1;
        self.normalize_inserts();
        Some((block, processor))
    }
}

/// Whether every sample is finite. This has no early exit, so its cost does not vary with audio.
fn all_finite(samples: &[f32]) -> bool {
    samples
        .iter()
        .fold(true, |finite, sample| finite & sample.is_finite())
}

/// Generates a process-unique neutral block identifier.
#[must_use]
pub fn unique_block_id() -> BlockId {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    BlockId(format!("block-{id}"))
}
