use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicUsize, Ordering},
    mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
};

use crate::command::ChainCommand;

pub(crate) struct RetirementBudget {
    capacity: usize,
    in_use: AtomicUsize,
}

impl RetirementBudget {
    pub(crate) fn acquire(self: &Arc<Self>) -> Option<RetirementPermit> {
        self.in_use
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < self.capacity).then_some(used + 1)
            })
            .ok()?;
        Some(RetirementPermit {
            budget: Arc::clone(self),
        })
    }
}

pub(crate) struct RetirementPermit {
    budget: Arc<RetirementBudget>,
}

impl Drop for RetirementPermit {
    fn drop(&mut self) {
        self.budget.in_use.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct RetiringChain {
    pub(crate) _work: ChainCommand,
    pub(crate) _permit: RetirementPermit,
}

/// Control-thread endpoint that destroys retired work away from the audio callback.
///
/// The callback only sends into the retirement channel and never takes this lock.
#[derive(Clone)]
pub(crate) struct RetiredDrain {
    receiver: Arc<Mutex<Receiver<RetiringChain>>>,
}

impl RetiredDrain {
    pub(crate) fn drain(&self) -> usize {
        self.receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_iter()
            .count()
    }
}

/// Fixed-capacity retirement storage shared by queued, active, and retired chains.
pub struct RetirementQueue {
    sender: SyncSender<RetiringChain>,
    retired: RetiredDrain,
    deferred: Option<RetiringChain>,
    pub(crate) budget: Arc<RetirementBudget>,
}

impl RetirementQueue {
    pub(crate) fn new(command_capacity: usize) -> Self {
        let capacity = command_capacity.saturating_add(1);
        let (sender, receiver) = sync_channel(capacity);
        Self {
            sender,
            retired: RetiredDrain {
                receiver: Arc::new(Mutex::new(receiver)),
            },
            deferred: None,
            budget: Arc::new(RetirementBudget {
                capacity,
                in_use: AtomicUsize::new(0),
            }),
        }
    }

    pub(crate) fn acquire(&self) -> Option<RetirementPermit> {
        self.budget.acquire()
    }

    pub(crate) fn retired_drain(&self) -> RetiredDrain {
        self.retired.clone()
    }

    // A rejected retirement must return the work without allocating.
    #[allow(clippy::result_large_err)]
    pub(crate) fn retire(
        &mut self,
        work: ChainCommand,
        permit: RetirementPermit,
    ) -> Result<(), (ChainCommand, RetirementPermit)> {
        if self.deferred.is_some() {
            return Err((work, permit));
        }
        match self.sender.try_send(RetiringChain {
            _work: work,
            _permit: permit,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(retiring) | TrySendError::Disconnected(retiring)) => {
                self.deferred = Some(retiring);
                Ok(())
            }
        }
    }

    pub(crate) fn drain(&mut self) -> usize {
        let mut count = usize::from(self.deferred.take().is_some());
        count += self.retired.drain();
        count
    }
}
