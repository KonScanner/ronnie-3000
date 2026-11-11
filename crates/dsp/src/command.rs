use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use ronie_core::{BlockId, BlockParams};

use crate::{
    prepare::PreparedChainChange,
    retire::{RetiredDrain, RetirementBudget, RetirementPermit},
    rt_chain::ChainControls,
};

/// Queued control snapshots before the control side must retry. The newest value wins.
const CONTROL_CAPACITY: usize = 16;

/// An engine-owning command sent from control code to the audio callback.
// `Replace` moves once through a bounded queue, and boxing it would change every publisher.
#[allow(clippy::large_enum_variant)]
pub enum ChainCommand {
    /// Replace the active chain when this generation is newer.
    Replace(PreparedChainChange),
    /// Swap new settings into matching blocks by identity and keep all engine state.
    UpdateBlockParams(Vec<(BlockId, BlockParams)>),
}

pub(crate) struct QueuedCommand {
    pub(crate) command: ChainCommand,
    pub(crate) _permit: RetirementPermit,
}

/// Bounded control-side command endpoint.
#[derive(Clone)]
pub struct ChainCommandSender {
    sender: SyncSender<QueuedCommand>,
    controls: SyncSender<ChainControls>,
    retirement: std::sync::Arc<RetirementBudget>,
    retired: RetiredDrain,
}

impl ChainCommandSender {
    /// Sends one command or returns it when capacity or the receiver is unavailable.
    ///
    /// # Errors
    ///
    /// Returns the standard full/disconnected error with the original command.
    // Returning ownership avoids boxing or allocation on the control thread.
    #[allow(clippy::result_large_err)]
    pub fn try_send(&self, command: ChainCommand) -> Result<(), TrySendError<ChainCommand>> {
        let Some(permit) = self.retirement.acquire() else {
            return Err(TrySendError::Full(command));
        };
        self.sender
            .try_send(QueuedCommand {
                command,
                _permit: permit,
            })
            .map_err(|error| match error {
                TrySendError::Full(queued) => TrySendError::Full(queued.command),
                TrySendError::Disconnected(queued) => TrySendError::Disconnected(queued.command),
            })
    }

    /// Queues one control snapshot for the next callback boundary.
    ///
    /// # Errors
    ///
    /// Returns the standard full/disconnected error with the original snapshot.
    // Returning the Copy snapshot by value avoids boxing on the control thread.
    #[allow(clippy::result_large_err)]
    pub fn try_send_controls(
        &self,
        controls: ChainControls,
    ) -> Result<(), TrySendError<ChainControls>> {
        self.controls.try_send(controls)
    }

    /// Destroys every retired chain on the calling control thread and returns their count.
    ///
    /// Each destroyed chain releases its share of the publication budget.
    pub fn drain_retired(&self) -> usize {
        self.retired.drain()
    }
}

/// Bounded callback-side command endpoint.
pub struct ChainCommandReceiver {
    pub(crate) receiver: Receiver<QueuedCommand>,
    pub(crate) controls: Receiver<ChainControls>,
}

pub(crate) fn channel(
    capacity: usize,
    retirement: &std::sync::Arc<RetirementBudget>,
    retired: RetiredDrain,
) -> (ChainCommandSender, ChainCommandReceiver) {
    let (sender, receiver) = sync_channel(capacity);
    let (controls_sender, controls) = sync_channel(CONTROL_CAPACITY);
    (
        ChainCommandSender {
            sender,
            controls: controls_sender,
            retirement: std::sync::Arc::clone(retirement),
            retired,
        },
        ChainCommandReceiver { receiver, controls },
    )
}
