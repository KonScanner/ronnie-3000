//! Bounded worker scheduling, nesting, and lifecycle tests.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use ronie_dsp::rt::{Job, JobError, RtWorkerPool};

fn increment(context: *mut ()) -> bool {
    // SAFETY: each job points to a live AtomicUsize for the duration of the join.
    unsafe { &*(context.cast::<AtomicUsize>()) }.fetch_add(1, Ordering::Relaxed);
    true
}

fn fail(context: *mut ()) -> bool {
    increment(context);
    false
}

fn job(counter: &mut AtomicUsize) -> Job {
    // SAFETY: the counter outlives the fork/join call.
    unsafe { Job::new(increment, counter) }
}

#[test]
fn every_job_runs_exactly_once_across_pool_sizes_and_toggles() {
    for workers in [0, 1, 2, 4] {
        let mut pool = RtWorkerPool::prepare(workers, 16).expect("worker pool");
        for enabled in [true, false, true] {
            pool.set_enabled(enabled);
            let mut counter = AtomicUsize::new(0);
            let mut jobs = (0..16).map(|_| job(&mut counter)).collect::<Vec<_>>();
            pool.fork_join(&mut jobs).expect("all jobs complete");
            assert_eq!(counter.load(Ordering::Relaxed), 16);
        }
        pool.release();
    }
}

struct Nested<'a> {
    pool: &'a RtWorkerPool,
    counter: &'a AtomicUsize,
}

fn nested(context: *mut ()) -> bool {
    // SAFETY: the nested context remains live until the outer join completes.
    let nested = unsafe { &mut *context.cast::<Nested<'_>>() };
    let mut counter = AtomicUsize::new(0);
    let mut jobs = (0..4).map(|_| job(&mut counter)).collect::<Vec<_>>();
    let completed = nested.pool.fork_join(&mut jobs).is_ok();
    nested
        .counter
        .fetch_add(counter.load(Ordering::Relaxed), Ordering::Relaxed);
    completed
}

#[test]
fn one_nested_level_and_sleeping_workers_still_complete() {
    let mut pool = RtWorkerPool::prepare(2, 16).expect("worker pool");
    std::thread::sleep(std::time::Duration::from_millis(110));
    let counter = AtomicUsize::new(0);
    let mut contexts = std::array::from_fn::<_, 4, _>(|_| Nested {
        pool: &pool,
        counter: &counter,
    });
    let mut jobs = contexts
        .iter_mut()
        .map(|nested| {
            // SAFETY: each nested context outlives its outer job and join.
            unsafe { Job::new(self::nested, nested) }
        })
        .collect::<Vec<_>>();
    pool.fork_join(&mut jobs).expect("nested jobs complete");
    assert_eq!(counter.load(Ordering::Relaxed), 16);
    pool.release();
}

#[test]
fn overflow_and_failures_are_contained_after_every_job_runs() {
    let mut pool = RtWorkerPool::prepare(2, 16).expect("worker pool");
    let mut counter = AtomicUsize::new(0);
    let mut jobs = (0..17).map(|_| job(&mut counter)).collect::<Vec<_>>();
    assert_eq!(pool.fork_join(&mut jobs), Err(JobError::Overflow));
    assert_eq!(counter.load(Ordering::Relaxed), 17);

    counter.store(0, Ordering::Relaxed);
    let mut jobs = (0..16)
        .map(|index| {
            if index == 7 {
                // SAFETY: the counter outlives the join.
                unsafe { Job::new(fail, &mut counter) }
            } else {
                job(&mut counter)
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(pool.fork_join(&mut jobs), Err(JobError::Failed));
    assert_eq!(counter.load(Ordering::Relaxed), 16);
    pool.release();
}

#[test]
fn prepare_release_churn_does_not_leak_or_duplicate_work() {
    for _ in 0..12 {
        let mut pool = RtWorkerPool::prepare(3, 16).expect("worker pool");
        let mut counter = AtomicUsize::new(0);
        let mut jobs = (0..16).map(|_| job(&mut counter)).collect::<Vec<_>>();
        pool.fork_join(&mut jobs).expect("jobs complete");
        assert_eq!(counter.load(Ordering::Relaxed), 16);
        pool.release();
    }
}
