#![allow(unsafe_code)]

use std::{
    cell::{Cell, UnsafeCell},
    ptr,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use super::{Job, thread};

const LEVELS: usize = 2;
const FALLBACK_PARK: Duration = Duration::from_millis(100);
/// The longest time that [`RtWorkerPool::prepare`] waits for its workers to start.
const START_LIMIT: Duration = Duration::from_secs(1);

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Bounded scheduling failure returned after all submitted work completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobError {
    /// More jobs arrived than the prepared registry can hold; they ran inline.
    Overflow,
    /// At least one job reported failure; every other job still ran.
    Failed,
}

struct Batch {
    jobs: UnsafeCell<[Job; 16]>,
    len: Cell<usize>,
    next: AtomicUsize,
    remaining: AtomicUsize,
    failed: AtomicBool,
    active: AtomicUsize,
}

// SAFETY: jobs are written only while the level is not ready and `active` is
// zero. Published jobs are immutable and each index is claimed exactly once.
unsafe impl Sync for Batch {}
// SAFETY: the same publication protocol permits moving batches between threads.
unsafe impl Send for Batch {}

impl Batch {
    fn new() -> Self {
        Self {
            jobs: UnsafeCell::new([Job::noop(); 16]),
            len: Cell::new(0),
            next: AtomicUsize::new(1),
            remaining: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
            active: AtomicUsize::new(0),
        }
    }

    fn reset(&self, jobs: &mut [Job]) {
        while self.active.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
        // SAFETY: this level is exclusively reserved by the caller and no
        // worker can access its job storage while `active` is zero.
        let storage = unsafe { &mut *self.jobs.get() };
        for (destination, source) in storage.iter_mut().zip(jobs.iter()) {
            *destination = *source;
        }
        self.len.set(jobs.len());
        self.failed.store(false, Ordering::Relaxed);
        self.next.store(1, Ordering::Relaxed);
        self.remaining.store(jobs.len(), Ordering::Release);
    }

    fn run(&self, job: Job) {
        if !job.run() {
            self.failed.store(true, Ordering::Relaxed);
        }
        self.remaining.fetch_sub(1, Ordering::Release);
    }

    fn run_first(&self) {
        // SAFETY: index zero is reserved for the calling thread until join.
        self.run(unsafe { (*self.jobs.get())[0] });
    }

    fn claim(&self) -> bool {
        let index = self.next.fetch_add(1, Ordering::AcqRel);
        if index >= self.len.get() {
            return false;
        }
        // SAFETY: the claimed index is in range and issued to only one thread.
        let job = unsafe { (*self.jobs.get())[index] };
        self.run(job);
        true
    }
}

struct Level {
    current: AtomicPtr<Batch>,
    ready: AtomicBool,
    batch: Batch,
}

impl Level {
    fn new() -> Self {
        Self {
            current: AtomicPtr::new(ptr::null_mut()),
            ready: AtomicBool::new(false),
            batch: Batch::new(),
        }
    }
}

pub(crate) struct Shared {
    levels: [Level; LEVELS],
    enabled: AtomicBool,
    stop: AtomicBool,
    started: AtomicUsize,
    wake: AtomicU64,
    wait_lock: Mutex<()>,
    wait: Condvar,
    max_jobs: usize,
}

impl Shared {
    fn new(max_jobs: usize) -> Self {
        Self {
            levels: std::array::from_fn(|_| Level::new()),
            enabled: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            started: AtomicUsize::new(0),
            wake: AtomicU64::new(0),
            wait_lock: Mutex::new(()),
            wait: Condvar::new(),
            max_jobs,
        }
    }

    fn notify(&self) {
        self.wake.fetch_add(1, Ordering::AcqRel);
        self.wait.notify_all();
    }
}

/// Fixed-capacity worker pool with caller execution, stealing, and inline fallback.
pub struct RtWorkerPool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl RtWorkerPool {
    /// Allocates workers and all job storage on the control thread, and waits until each
    /// worker runs.
    ///
    /// # Errors
    ///
    /// Returns an invalid-capacity error unless `max_jobs` is in `1..=16`.
    pub fn prepare(worker_count: usize, max_jobs: usize) -> Result<Self, &'static str> {
        prepare_with_spawn(worker_count, max_jobs, thread::spawn)
    }

    /// Enables worker scheduling or selects the serial inline path.
    pub fn set_enabled(&self, enabled: bool) {
        self.shared.enabled.store(enabled, Ordering::Release);
        self.shared.notify();
    }

    /// Executes borrowed jobs exactly once and joins before returning.
    ///
    /// Overflow and concurrent publication conflicts run inline. Job failures
    /// are collected after all work completes.
    ///
    /// # Errors
    ///
    /// Returns [`JobError::Overflow`] or [`JobError::Failed`] after execution.
    pub fn fork_join(&self, jobs: &mut [Job]) -> Result<(), JobError> {
        if jobs.is_empty() {
            return Ok(());
        }
        if jobs.len() > self.shared.max_jobs {
            let _ = run_inline(jobs);
            return Err(JobError::Overflow);
        }
        if !self.shared.enabled.load(Ordering::Acquire) || self.shared.stop.load(Ordering::Acquire)
        {
            return run_inline(jobs);
        }
        let depth = DEPTH.with(Cell::get);
        if depth >= LEVELS {
            return run_inline(jobs);
        }
        let level = &self.shared.levels[depth];
        let batch = ptr::from_ref(&level.batch).cast_mut();
        if level
            .current
            .compare_exchange(ptr::null_mut(), batch, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return run_inline(jobs);
        }
        level.batch.reset(jobs);
        DEPTH.with(|value| value.set(depth + 1));
        level.ready.store(true, Ordering::Release);
        self.shared.notify();
        level.batch.run_first();
        while level.batch.remaining.load(Ordering::Acquire) != 0 {
            if !level.batch.claim() {
                std::hint::spin_loop();
            }
        }
        DEPTH.with(|value| value.set(depth));
        level.ready.store(false, Ordering::Release);
        level.current.store(ptr::null_mut(), Ordering::Release);
        if level.batch.failed.load(Ordering::Acquire) {
            Err(JobError::Failed)
        } else {
            Ok(())
        }
    }

    /// Stops and joins every worker on the control thread.
    pub fn release(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.notify();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for RtWorkerPool {
    fn drop(&mut self) {
        self.release();
    }
}

fn prepare_with_spawn(
    worker_count: usize,
    max_jobs: usize,
    mut spawn: impl FnMut(Arc<Shared>) -> Result<JoinHandle<()>, &'static str>,
) -> Result<RtWorkerPool, &'static str> {
    if !(1..=16).contains(&max_jobs) {
        return Err("maximum job count must be in 1..=16");
    }
    let shared = Arc::new(Shared::new(max_jobs));
    let mut workers = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        match spawn(Arc::clone(&shared)) {
            Ok(worker) => workers.push(worker),
            Err(error) => {
                shared.stop.store(true, Ordering::Release);
                shared.notify();
                for worker in workers.drain(..) {
                    let _ = worker.join();
                }
                return Err(error);
            }
        }
    }
    // A thread allocates while it starts, for example for its name on Windows. The pool waits
    // for that, so that it does not occur during the first callback.
    let deadline = Instant::now() + START_LIMIT;
    while shared.started.load(Ordering::Acquire) < workers.len() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_micros(200));
    }
    Ok(RtWorkerPool { shared, workers })
}

pub(crate) fn worker_main(shared: Arc<Shared>) {
    shared.started.fetch_add(1, Ordering::AcqRel);
    let mut observed = 0_u64;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let mut worked = false;
        for level in &shared.levels {
            let pointer = level.current.load(Ordering::Acquire);
            if pointer.is_null() || !level.ready.load(Ordering::Acquire) {
                continue;
            }
            // SAFETY: batch storage belongs to `shared` and outlives this worker.
            let batch = unsafe { &*pointer };
            batch.active.fetch_add(1, Ordering::AcqRel);
            if level.current.load(Ordering::Acquire) == pointer
                && level.ready.load(Ordering::Acquire)
            {
                worked |= batch.claim();
            }
            batch.active.fetch_sub(1, Ordering::AcqRel);
        }
        if worked {
            continue;
        }
        let wake = shared.wake.load(Ordering::Acquire);
        if wake != observed {
            observed = wake;
            continue;
        }
        let Ok(guard) = shared.wait_lock.lock() else {
            return;
        };
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        if shared.wake.load(Ordering::Acquire) != observed {
            continue;
        }
        let _ = shared.wait.wait_timeout(guard, FALLBACK_PARK);
    }
}

fn run_inline(jobs: &mut [Job]) -> Result<(), JobError> {
    let mut failed = false;
    for job in jobs {
        failed |= !job.run();
    }
    if failed {
        Err(JobError::Failed)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_failure_stops_and_joins_every_started_worker() {
        let mut observed = None;
        let mut calls = 0;
        let result = prepare_with_spawn(4, 16, |shared| {
            calls += 1;
            observed = Some(Arc::clone(&shared));
            if calls == 3 {
                Err("injected spawn failure")
            } else {
                std::thread::Builder::new()
                    .spawn(|| {})
                    .map_err(|_| "spawn failed")
            }
        });
        assert!(result.is_err());
        assert!(observed.expect("shared state").stop.load(Ordering::Acquire));
    }

    #[test]
    fn a_prepared_pool_has_every_worker_running() {
        let pool = RtWorkerPool::prepare(4, 16).expect("worker pool");
        assert_eq!(pool.shared.started.load(Ordering::Acquire), 4);
    }
}
