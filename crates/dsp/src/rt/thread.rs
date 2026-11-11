use std::sync::Arc;

use super::pool::{Shared, worker_main};

pub(crate) fn spawn(shared: Arc<Shared>) -> Result<std::thread::JoinHandle<()>, &'static str> {
    std::thread::Builder::new()
        .name("ronie-audio-worker".into())
        .spawn(move || {
            configure_worker_thread();
            worker_main(shared);
        })
        .map_err(|_| "worker thread creation failed")
}

fn configure_worker_thread() {
    // No process-global denormal or priority mode is changed. Each worker uses
    // the same default floating-point environment as the calling audio thread.
}
