//! Deep module that keeps the running audio stream equal to the processor session.
//!
//! The interface is small: change the session through [`AudioEngine::edit_session`], ask for
//! device changes, read the state, and wait with [`AudioEngine::settle`] when a test or a
//! background flow needs the stream to match the session. Each call returns soon, because the
//! engine does all slow work on its own threads with no engine lock:
//!
//! - The chain worker thread compiles each chain for a new structure and each repair. The engine
//!   lock reads the plan of the chain and its session state together, and the compile reads only
//!   that plan. The callback keeps the old chain until the new chain is ready. A chain whose
//!   structure the session left during its compile is dropped, and the worker compiles again.
//! - A device worker thread opens and closes streams and talks to the sound server. The engine
//!   gives up on a job that does not end in time, and a new thread takes the later jobs.
//!
//! Meters read the telemetry atomics of the stream through a view that the engine publishes, so
//! a meter read never waits for the engine lock. The cpal and dummy backends are the two adapters
//! at the [`super::AudioBackend`] seam.

mod state;

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use ronie_core::BlockId;
use ronie_dsp::block::SpectrumTap;
use ronie_plugin::ProcessorBackend;
use ronie_ui::{
    backend::{ChainFault, DeviceAction, MeterSnapshot, UiError},
    default::SystemDeviceState,
};

#[cfg(test)]
pub(crate) use state::PrepareChain;
pub use state::{AudioBackendFactory, AudioEngineStatus, new_backend};
use state::{EngineState, ViewSlot};

use super::{AudioBackendKind, AudioTelemetry, DummyBackend};
use crate::{audio_settings::StandaloneAudioSettings, diagnostics::DiagnosticLog};

/// The longest wait of [`AudioEngine::settle`] before it looks at the state again. Each worker
/// wakes the wait when its work ends, so this bound only limits a missed wake.
const SETTLE_RECHECK: Duration = Duration::from_millis(250);

/// Owner of the running stream and of the threads that do its slow work.
///
/// A drop stops the chain worker and waits for it. A device worker with no job also ends, and
/// the drop waits for it. A device worker in a job that the sound server holds ends after that
/// job, so that an exit does not wait for the sound server.
pub struct AudioEngine {
    core: Arc<Core>,
    chain_worker: Option<JoinHandle<()>>,
}

/// What the engine shares with its threads.
struct Core {
    state: Mutex<EngineState>,
    /// What meters read with no engine lock.
    view: Arc<ViewSlot>,
    /// Wakes the chain worker for chain work or for the shutdown.
    chain_work: Condvar,
    /// Wakes a caller of [`AudioEngine::settle`] when work ends.
    work_done: Condvar,
    /// The newest device worker thread, with its worker id. A thread that the engine gave up on
    /// has an old id, so that a drop does not wait for it.
    device_thread: Mutex<Option<(u64, JoinHandle<()>)>>,
    /// Whether an edit waits until the chain worker has the chain of the new structure ready.
    edits_wait: bool,
}

impl AudioEngine {
    /// Starts an engine on the dummy adapter with a device that the caller also controls. The
    /// first stream opens on this thread.
    ///
    /// This engine gives a test a known result: each edit returns when the chain of its new
    /// structure waits for the next callback. The chain worker still does the compile, with no
    /// engine lock. For the timing of a real stream on a dummy device, use [`Self::deferred`].
    ///
    /// # Errors
    /// Returns a device-enumeration or open error, or an error when no thread can start.
    pub fn with_dummy_device(
        session: ProcessorBackend,
        settings: StandaloneAudioSettings,
        device: DummyBackend,
        log: DiagnosticLog,
    ) -> Result<Self, UiError> {
        let factory: AudioBackendFactory = Arc::new(move |kind| {
            if kind == AudioBackendKind::Dummy {
                Box::new(device.clone())
            } else {
                new_backend(kind)
            }
        });
        let state = EngineState::started(
            session,
            settings,
            factory,
            AudioBackendKind::Dummy,
            Some(log),
        )?;
        Self::around(state, true)
    }

    /// Starts an engine that has no stream yet, and touches no device on this thread.
    ///
    /// The device worker finds the devices and opens the stream, so that a window can open before
    /// the sound server answers.
    ///
    /// # Errors
    /// Returns an error when no thread can start.
    pub fn deferred(
        session: ProcessorBackend,
        settings: StandaloneAudioSettings,
        make_backend: AudioBackendFactory,
        kind: AudioBackendKind,
        log: DiagnosticLog,
    ) -> Result<Self, UiError> {
        let state = EngineState::deferred(session, settings, make_backend, kind, Some(log));
        let engine = Self::around(state, false)?;
        engine.core.step(|_| ())?;
        Ok(engine)
    }

    fn around(mut state: EngineState, edits_wait: bool) -> Result<Self, UiError> {
        state.chain_worker = true;
        state.publish_view();
        let core = Arc::new(Core {
            view: state.view_slot(),
            state: Mutex::new(state),
            chain_work: Condvar::new(),
            work_done: Condvar::new(),
            device_thread: Mutex::default(),
            edits_wait,
        });
        let worker_core = Arc::clone(&core);
        let chain_worker = std::thread::Builder::new()
            .name("ronie-audio-chain".to_owned())
            .spawn(move || run_chain_worker(&worker_core))
            .map_err(|error| UiError::Control(format!("starting the chain worker: {error}")))?;
        Ok(Self {
            core,
            chain_worker: Some(chain_worker),
        })
    }

    /// Changes the session and brings the stream up to date with it, in one engine lock.
    ///
    /// A new structure only wakes the chain worker, so this call compiles nothing and returns
    /// soon. `edit` must not take long, because a device job and the chain worker wait for it.
    /// Only an engine from [`Self::with_dummy_device`] waits here for the chain of a new
    /// structure.
    ///
    /// # Errors
    /// Returns the error of `edit`, or an error when the engine or processor state is unavailable.
    /// After an error of the engine lock, `edit` does not run.
    pub fn edit_session<R>(&self, edit: impl FnOnce() -> Result<R, UiError>) -> Result<R, UiError> {
        self.edit(edit, false)
    }

    /// Changes the session as [`Self::edit_session`] does, for a caller that calls
    /// [`Self::settle`] next, maybe from a thread that holds a lock of a load flow.
    ///
    /// An engine from [`Self::with_dummy_device`] prepares no chain before that settle, and a
    /// later edit also waits for it, so that a test sees the order of the load flow.
    ///
    /// # Errors
    /// The errors of [`Self::edit_session`].
    pub fn edit_session_for_settle<R>(
        &self,
        edit: impl FnOnce() -> Result<R, UiError>,
    ) -> Result<R, UiError> {
        self.edit(edit, true)
    }

    fn edit<R>(
        &self,
        edit: impl FnOnce() -> Result<R, UiError>,
        for_settle: bool,
    ) -> Result<R, UiError> {
        let edits_wait = self.core.edits_wait;
        let result = self.core.step(|state| {
            let result = edit()?;
            state.sync()?;
            // The hold starts only after a success, because no settle follows a failed edit. The
            // chain worker cannot see the gap, because this all runs in one engine lock.
            if edits_wait && for_settle {
                state.hold_chain();
            }
            Ok(result)
        })??;
        if edits_wait {
            self.core
                .wait_until(|state| !state.structure_work_pending())?;
        }
        Ok(result)
    }

    /// Records one device action. A device worker does anything slow.
    ///
    /// # Errors
    /// Returns a settings error of a MIDI choice. A stream error appears in the device state.
    pub fn request_device(&self, action: &DeviceAction) -> Result<(), UiError> {
        self.core.step(|state| state.request_device(action))?
    }

    /// Asks a device worker for a new look at the devices.
    pub fn request_device_list(&self) {
        let _ = self.core.step(EngineState::request_device_list);
    }

    /// Gives the engine control-thread time, and returns the device state for settings and
    /// banners. A frame calls this.
    ///
    /// The engine destroys retired chains, retries waiting changes, reads faults from the
    /// callback, and starts the work that they ask for on its threads.
    #[must_use]
    pub fn device_state(&self) -> Option<SystemDeviceState> {
        self.core
            .step(|state| {
                state.poll();
                state.device_state().clone()
            })
            .ok()
    }

    /// Current observable engine state.
    ///
    /// # Errors
    /// Returns an error when the engine lock is poisoned.
    pub fn status(&self) -> Result<AudioEngineStatus, UiError> {
        self.core.step(|state| state.status())
    }

    /// Current audio chain faults, worst first.
    ///
    /// Without a stream, no chain runs. The device banners then report the lost stream, and the
    /// next open prepares a chain and reports its faults.
    #[must_use]
    pub fn chain_faults(&self) -> Vec<ChainFault> {
        self.core
            .step(|state| state.chain_faults())
            .unwrap_or_default()
    }

    /// The levels and the load of the running stream, with no engine lock.
    #[must_use]
    pub fn meter_snapshot(&self) -> MeterSnapshot {
        self.core.view.load().meter_snapshot()
    }

    /// The level of each channel of the capture device, with no engine lock.
    #[must_use]
    pub fn capture_levels_db(&self) -> Vec<f32> {
        self.core
            .view
            .load()
            .telemetry()
            .map(|telemetry| telemetry.capture_levels_db())
            .unwrap_or_default()
    }

    /// A handle to the telemetry of the running stream, for analysis with no engine lock.
    #[must_use]
    pub fn telemetry(&self) -> Option<Arc<AudioTelemetry>> {
        self.core.view.load().telemetry()
    }

    /// The lock-free output of one block of the newest chain, for analysis with no engine lock.
    #[must_use]
    pub fn spectrum_tap(&self, block: &BlockId) -> Option<Arc<SpectrumTap>> {
        self.core.view.load().spectrum_tap(block)
    }

    /// Waits until no device work and no chain work waits or runs, so that the stream matches
    /// the session.
    ///
    /// The time limit of device work bounds the wait for a device job. A chain compile has no
    /// limit, so a compile that never ends holds this wait.
    ///
    /// # Errors
    /// Returns an error when the engine lock is poisoned.
    pub fn settle(&self) -> Result<(), UiError> {
        self.core.step(EngineState::release_chain)?;
        self.core.wait_until(|state| !state.busy())
    }

    /// Renders explicit input through the running dummy stream, as one callback.
    ///
    /// # Errors
    /// Returns a stream-layout or processing error, or an error without a dummy stream.
    pub fn render_dummy(&self, input: &[f32]) -> Result<Vec<f32>, UiError> {
        self.core.step(|state| state.render_dummy(input))?
    }

    /// Renders all channels of the dummy device, as a host gives them, as one callback.
    ///
    /// # Errors
    /// Returns a stream-layout or processing error, or an error without a dummy stream.
    pub fn render_dummy_capture(&self, capture: &[f32]) -> Result<Vec<f32>, UiError> {
        self.core
            .step(|state| state.render_dummy_capture(capture))?
    }

    /// Changes how long device work can run, for a test of a stuck job.
    ///
    /// # Errors
    /// Returns an error when the engine lock is poisoned.
    pub fn set_work_limit(&self, limit: Duration) -> Result<(), UiError> {
        self.core.step(|state| state.set_work_limit(limit))
    }
}

#[cfg(test)]
impl AudioEngine {
    /// Replaces the compile of the chain worker, so that a test can hold a compile open.
    pub(crate) fn set_prepare_for_test(&self, prepare: PrepareChain) {
        self.core.lock().expect("engine").set_prepare(prepare);
    }

    /// Poisons the engine lock, as a panic under it does.
    pub(crate) fn poison_for_test(&self) {
        let core = Arc::clone(&self.core);
        std::thread::spawn(move || {
            let _guard = core.state.lock();
            panic!("this panic poisons the engine lock for a test");
        })
        .join()
        .expect_err("the poisoning thread must panic");
    }

    pub(crate) fn repair_wanted_for_test(&self) -> bool {
        self.core.lock().expect("engine").repair_wanted()
    }

    /// The block size of the stream, with no poll.
    pub(crate) fn audio_frames_for_test(&self) -> usize {
        self.core
            .lock()
            .expect("engine")
            .device_state()
            .banner
            .buffer_size as usize
    }

    pub(crate) fn age_repair_for_test(&self) {
        self.core.lock().expect("engine").age_repair_for_test();
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let (streams, idle_worker) = self
            .core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .shut_down();
        self.core.chain_work.notify_all();
        self.core.work_done.notify_all();
        if let Some(worker) = self.chain_worker.take() {
            // A panic of the worker already ended its role, and nothing else can report it now.
            let _ = worker.join();
        }
        let device = self
            .core
            .device_thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        // Any other thread is in a job that the engine gave up on, and it ends after that job.
        if let Some(idle) = idle_worker
            && let Some((worker, device)) = device
            && worker == idle
        {
            let _ = device.join();
        }
        for stream in streams {
            // Dropping the stream releases its resources also after a failed close.
            let _ = stream.close();
        }
    }
}

impl Core {
    /// Takes the engine lock. A poisoned lock stays an error for the life of the engine.
    ///
    /// This is a decision, not an omission. A panic under this lock, for example in the `edit`
    /// closure of [`AudioEngine::edit_session`], can stop a change halfway, and a state that is
    /// half changed cannot be trusted. So each later edit, status call, device call, and settle
    /// returns this error and does not run on that state. Only the meters keep working, because
    /// they read the view slot and the telemetry atomics with no engine lock.
    ///
    /// Only the teardown paths read a poisoned state: the drop of [`AudioEngine`] and the guards
    /// of the worker threads. They run no edit. They only stop the threads, end the worker roles,
    /// and close the streams, so that a device is not held after the engine stops.
    fn lock(&self) -> Result<MutexGuard<'_, EngineState>, UiError> {
        self.state.lock().map_err(lock_error)
    }

    /// Runs one change of the state in the engine lock, and then starts the work that it asks
    /// for.
    fn step<R>(self: &Arc<Self>, change: impl FnOnce(&mut EngineState) -> R) -> Result<R, UiError> {
        let mut state = self.lock()?;
        let result = change(&mut state);
        let device_worker = self.end_step(&mut state);
        drop(state);
        if let Some(worker) = device_worker {
            self.start_device_worker(worker);
        }
        Ok(result)
    }

    /// Waits until `done` holds. Each worker wakes this wait when its work ends, and the time
    /// limit of device work bounds the wait for a device job.
    fn wait_until(self: &Arc<Self>, done: impl Fn(&EngineState) -> bool) -> Result<(), UiError> {
        let mut state = self.lock()?;
        loop {
            // A device job that runs too long ends here.
            state.poll();
            if let Some(worker) = self.end_step(&mut state) {
                drop(state);
                self.start_device_worker(worker);
                state = self.lock()?;
                continue;
            }
            if done(&state) {
                return Ok(());
            }
            let wait = state
                .device_job_deadline()
                .map_or(SETTLE_RECHECK, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                })
                .clamp(Duration::from_millis(1), SETTLE_RECHECK);
            state = self
                .work_done
                .wait_timeout(state, wait)
                .map_err(lock_error)?
                .0;
        }
    }

    /// What each step ends with, in the same lock: it publishes the meter view, wakes the
    /// threads that wait, and claims the device worker role when device work waits.
    fn end_step(&self, state: &mut EngineState) -> Option<u64> {
        state.publish_view();
        if state.chain_work_wanted() {
            self.chain_work.notify_one();
        }
        self.work_done.notify_all();
        state.claim_device_worker()
    }

    /// Starts the thread of one claimed device worker role.
    fn start_device_worker(self: &Arc<Self>, worker: u64) {
        let core = Arc::clone(self);
        let started = std::thread::Builder::new()
            .name("ronie-audio-devices".to_owned())
            .spawn(move || {
                let _guard = DeviceWorkerGuard {
                    core: &core,
                    worker,
                };
                run_device_worker(&core, worker);
            });
        match started {
            Ok(thread) => {
                let mut newest = self
                    .device_thread
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                // An older worker can start late. It must not hide the handle of a newer worker.
                if newest.as_ref().is_none_or(|(id, _)| *id <= worker) {
                    *newest = Some((worker, thread));
                }
            }
            Err(_) => {
                if let Ok(mut state) = self.lock() {
                    state.device_worker_stopped(worker);
                }
                self.work_done.notify_all();
            }
        }
    }
}

/// The loop of the chain worker. Each compile runs with no engine lock.
fn run_chain_worker(core: &Arc<Core>) {
    let _guard = ChainWorkerGuard { core };
    loop {
        let Ok(state) = core.lock() else {
            return;
        };
        let Ok(mut state) = core.chain_work.wait_while(state, |state| {
            !state.shutdown_started() && !state.chain_work_wanted()
        }) else {
            return;
        };
        if state.shutdown_started() {
            return;
        }
        let deferred = match state.begin_deferred_chain() {
            Ok(Some(deferred)) => deferred,
            Ok(None) => {
                core.work_done.notify_all();
                continue;
            }
            Err(_) => {
                // The processor state is unavailable, so no compile can succeed now.
                state.abandon_deferred_chain();
                core.work_done.notify_all();
                continue;
            }
        };
        let prepare = state.compile_tool();
        drop(state);
        let prepared =
            catch_unwind(AssertUnwindSafe(|| prepare(deferred.plan()))).unwrap_or_else(|panic| {
                Err(UiError::Control(format!(
                    "the chain preparation stopped unexpectedly: {}",
                    panic_message(panic.as_ref())
                )))
            });
        let Ok(mut state) = core.lock() else {
            return;
        };
        if state.finish_deferred_chain(deferred, prepared).is_err() {
            state.abandon_deferred_chain();
        }
        let unused = state.take_unused_chains();
        let device_worker = core.end_step(&mut state);
        drop(state);
        // A chain can own large models, so no engine lock covers its drop.
        drop(unused);
        if let Some(worker) = device_worker {
            core.start_device_worker(worker);
        }
    }
}

/// Ends the chain worker role when its thread ends, also after a panic, so that no edit and no
/// wait depends on a thread that is gone.
struct ChainWorkerGuard<'a> {
    core: &'a Core,
}

impl Drop for ChainWorkerGuard<'_> {
    fn drop(&mut self) {
        self.core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .chain_worker_stopped();
        self.core.work_done.notify_all();
    }
}

/// The loop of one device worker. Each job runs with no engine lock.
fn run_device_worker(core: &Arc<Core>, worker: u64) {
    loop {
        let Ok(Some(job)) = core.step(|state| state.begin_device_work(worker)) else {
            return;
        };
        let outcome = job.run();
        let finished = core.step(|state| {
            state.finish_device_work(outcome);
            // An edit during the job did not reach this new stream. The chain worker compiles
            // what it needs.
            let _ = state.sync();
        });
        if finished.is_err() {
            return;
        }
    }
}

/// Shows a failure when a device worker stops in a panic.
struct DeviceWorkerGuard<'a> {
    core: &'a Core,
    worker: u64,
}

impl Drop for DeviceWorkerGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .device_worker_stopped(self.worker);
            self.core.work_done.notify_all();
        }
    }
}

/// The text of a panic. A panic with a format string or a literal has one, and others do not.
fn panic_message(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message")
}

fn lock_error<T>(error: PoisonError<T>) -> UiError {
    UiError::Control(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    use ronie_core::{InputMode, ModelBytes, ModelId, Tone};
    use ronie_dsp::PrepareSpec;
    use ronie_plugin::{RonieProcessor, SessionChainPlan};
    use ronie_ui::backend::{ChainFaultKind, UiAction, UiBackend};

    use super::*;

    /// A limit for what must happen at once, so that a stall fails and does not hang the test.
    const LIMIT: Duration = Duration::from_secs(20);

    fn ir_tone() -> Tone {
        Tone::from_value(serde_json::json!({
            "id": 6,
            "title": "Remote",
            "format": "ir",
            "models": [{"id": 8, "name": "Cab", "model_url": "https://models.example/8.wav"}],
        }))
        .expect("tone")
    }

    fn ir_bytes() -> ModelBytes {
        let samples = [16_000_i16, 0, 0, 16_000];
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36_u32 + samples.len() as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes.into()
    }

    /// A compile that tells `started` and then waits for its gate.
    fn held_compile(started: mpsc::Sender<()>, gate: mpsc::Receiver<()>) -> PrepareChain {
        let gate = Mutex::new(gate);
        Arc::new(move |plan: &SessionChainPlan| {
            let _ = started.send(());
            let _ = gate.lock().expect("gate").recv_timeout(LIMIT);
            plan.prepare()
        })
    }

    /// An engine with the timing of a real stream on a dummy device, and one tone on its running
    /// chain.
    struct Rig {
        engine: Arc<AudioEngine>,
        session: ProcessorBackend,
        tone: BlockId,
        _directory: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("tempdir");
            let session = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
                host_sample_rate: 48_000.0,
                max_host_frames: 128,
                oversampling: 1,
                input_mode: InputMode::Left,
                stereo_input: false,
                stereo_output: true,
            }));
            let settings = StandaloneAudioSettings::load(directory.path().join("audio.settings"))
                .expect("settings");
            let engine = AudioEngine::deferred(
                session.clone(),
                settings,
                Arc::new(|_| Box::new(DummyBackend::connected())),
                AudioBackendKind::Dummy,
                DiagnosticLog::start(&directory.path().join("logs")),
            )
            .expect("engine");
            engine.settle().expect("first open");
            let tone = session.chain_state(0).chain.left[0].id.clone();
            let rig = Self {
                engine: Arc::new(engine),
                session,
                tone,
                _directory: directory,
            };
            rig.edit(UiAction::LoadToneWithBytes {
                target_insert_id: rig.tone.clone(),
                tone: ir_tone(),
                model_id: ModelId(8),
                bytes: ir_bytes(),
            });
            rig.engine.settle().expect("settle");
            rig.render();
            rig
        }

        fn edit(&self, action: UiAction) {
            let session = self.session.clone();
            self.engine
                .edit_session(|| session.dispatch(action))
                .expect("edit");
        }

        fn render(&self) {
            self.engine.render_dummy(&[0.25; 256]).expect("render");
        }

        fn status(&self) -> AudioEngineStatus {
            self.engine.status().expect("status")
        }

        fn metered_blocks(&self) -> usize {
            self.engine.meter_snapshot().blocks.len()
        }
    }

    /// Runs one call on another thread, and fails if the call does not return.
    fn returns_at_once<T: Send + 'static>(
        what: &str,
        call: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (answer, returned) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer.send(call());
        });
        returned
            .recv_timeout(LIMIT)
            .unwrap_or_else(|_| panic!("{what} waited for the compile"))
    }

    #[test]
    fn a_structure_edit_and_the_meters_return_while_the_compile_is_held_open() {
        let rig = Rig::new();
        let first = rig.status().published_generation;
        assert_eq!(rig.metered_blocks(), 1);
        let (started_tx, started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let compiler = Arc::new(Mutex::new(None));
        let named = Arc::clone(&compiler);
        let held = held_compile(started_tx, gate);
        rig.engine
            .set_prepare_for_test(Arc::new(move |plan: &SessionChainPlan| {
                *named.lock().expect("name") = std::thread::current().name().map(str::to_owned);
                held(plan)
            }));

        let engine = Arc::clone(&rig.engine);
        let session = rig.session.clone();
        let tone = rig.tone.clone();
        returns_at_once("the edit", move || {
            engine
                .edit_session(|| session.dispatch(UiAction::DuplicateBlock(tone)))
                .expect("add a tone");
        });
        started.recv_timeout(LIMIT).expect("a compile");
        assert_eq!(
            compiler.lock().expect("name").as_deref(),
            Some("ronie-audio-chain")
        );

        for _ in 0..20 {
            let engine = Arc::clone(&rig.engine);
            let meter = returns_at_once("a meter read", move || {
                let meter = engine.meter_snapshot();
                let _ = engine.capture_levels_db();
                let _ = engine.telemetry();
                let _ = engine.device_state();
                let _ = engine.chain_faults();
                meter
            });
            assert_eq!(meter.blocks.len(), 1, "the meters keep the running chain");
            // The dummy callback runs on one thread only, as a real callback does.
            rig.render();
        }
        let held = rig.status();
        assert_eq!(
            held.published_generation, first,
            "no chain before the compile ends"
        );
        assert_eq!(
            held.active_generation, first,
            "the callback keeps the old chain"
        );

        drop(release);
        rig.engine.settle().expect("settle");
        rig.render();
        let ready = rig.status();
        assert!(ready.published_generation > first);
        assert_eq!(ready.active_generation, ready.published_generation);
        assert_eq!(rig.metered_blocks(), 2, "the new tone has a meter");
    }

    #[test]
    fn a_preset_load_and_the_meters_return_while_its_compile_is_held_open() {
        let rig = Rig::new();
        // The preset holds the tone of the rig. The session then loses that tone, so that the
        // load needs a new chain.
        rig.edit(UiAction::SavePreset {
            name: "Held".to_owned(),
        });
        let preset = rig
            .session
            .chain_state(0)
            .session
            .active_preset
            .clone()
            .expect("the saved preset is active");
        rig.edit(UiAction::RemoveBlock(rig.tone.clone()));
        rig.engine.settle().expect("settle");
        rig.render();
        assert_eq!(rig.metered_blocks(), 0);
        let before_load = rig.session.audio_state().expect("state");
        let first = rig.status().published_generation;

        let (started_tx, started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let planned = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&planned);
        let held = held_compile(started_tx, gate);
        rig.engine
            .set_prepare_for_test(Arc::new(move |plan: &SessionChainPlan| {
                recorded.lock().expect("record").push(plan.state().clone());
                held(plan)
            }));

        let engine = Arc::clone(&rig.engine);
        let session = rig.session.clone();
        let id = preset.id.clone();
        returns_at_once("the preset load", move || {
            engine
                .edit_session(|| session.dispatch(UiAction::LoadPreset { id }))
                .expect("load the preset");
        });
        started.recv_timeout(LIMIT).expect("a compile");
        assert_eq!(
            rig.session.chain_state(0).session.active_preset,
            Some(preset)
        );

        for _ in 0..20 {
            let engine = Arc::clone(&rig.engine);
            let meter = returns_at_once("a meter read", move || {
                let meter = engine.meter_snapshot();
                let _ = engine.capture_levels_db();
                let _ = engine.device_state();
                let _ = engine.chain_faults();
                meter
            });
            assert!(meter.blocks.is_empty(), "the meters keep the running chain");
            rig.render();
        }
        let held = rig.status();
        assert_eq!(
            held.published_generation, first,
            "no chain before the compile"
        );
        assert_eq!(
            held.active_generation, first,
            "the callback keeps the old chain"
        );

        drop(release);
        rig.engine.settle().expect("settle");
        rig.render();
        let ready = rig.status();
        assert!(ready.published_generation > first);
        assert_eq!(ready.active_generation, ready.published_generation);
        assert!(
            ready.unavailable_blocks.is_empty(),
            "the embedded model runs"
        );
        let loaded = rig.session.audio_state().expect("state");
        assert_eq!(
            planned.lock().expect("planned").as_slice(),
            [loaded],
            "one compile, and its chain reproduces the preset"
        );
        assert_eq!(
            rig.metered_blocks(),
            1,
            "the tone of the preset has a meter"
        );
        assert!(rig.engine.spectrum_tap(&rig.tone).is_some());

        // An undo of the load reaches the stream through the chain worker.
        rig.edit(UiAction::Undo);
        rig.engine.settle().expect("settle");
        rig.render();
        assert_eq!(rig.session.audio_state().expect("state"), before_load);
        assert_eq!(rig.metered_blocks(), 0, "the undo removed the tone again");
        let undone = rig.status();
        assert!(undone.published_generation > ready.published_generation);
        assert_eq!(undone.active_generation, undone.published_generation);
    }

    #[test]
    fn meter_reads_take_no_engine_lock() {
        let rig = Rig::new();
        let tone = rig.tone.clone();
        let _held = rig.engine.core.state.lock().expect("engine");

        let engine = Arc::clone(&rig.engine);
        let (meter, capture, telemetry, tap) = returns_at_once("a meter read", move || {
            (
                engine.meter_snapshot(),
                engine.capture_levels_db(),
                engine.telemetry(),
                engine.spectrum_tap(&tone),
            )
        });

        assert_eq!(meter.blocks.len(), 1);
        assert!(!capture.is_empty());
        assert!(telemetry.is_some());
        assert!(tap.is_some());
    }

    #[test]
    fn a_compile_that_a_later_edit_makes_stale_is_dropped_and_compiled_again() {
        let rig = Rig::new();
        let first = rig.status();
        let generations = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&generations);
        let (started_tx, started) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        rig.engine
            .set_prepare_for_test(Arc::new(move |plan: &SessionChainPlan| {
                let chain = plan.prepare();
                if let Ok(chain) = &chain {
                    recorded
                        .lock()
                        .expect("record")
                        .push(chain.change.generation);
                }
                let _ = started_tx.send(());
                let _ = gate.lock().expect("gate").recv_timeout(LIMIT);
                chain
            }));

        rig.edit(UiAction::DuplicateBlock(rig.tone.clone()));
        started.recv_timeout(LIMIT).expect("the first compile");
        // This edit comes during the first compile, so that compile is stale when it ends.
        rig.edit(UiAction::RemoveBlock(rig.tone.clone()));
        drop(release);
        rig.engine.settle().expect("settle");

        let generations = generations.lock().expect("generations").clone();
        assert_eq!(generations.len(), 2, "the stale chain was compiled again");
        assert_eq!(rig.status().published_generation, generations[1]);
        rig.render();
        let rendered = rig.status();
        assert_eq!(rendered.active_generation, generations[1]);
        assert_eq!(
            rendered.retired_chains,
            first.retired_chains + 1,
            "only the newest chain reached the callback"
        );
        assert_eq!(rig.metered_blocks(), 1, "the copy is the only tone");
    }

    /// What one compile of the hook below read and made.
    struct Compiled {
        generation: u64,
        planned: ronie_plugin::AudioSessionState,
        stereo: bool,
    }

    #[test]
    fn a_chain_compiled_across_an_edit_and_its_undo_reproduces_the_session() {
        let rig = Rig::new();
        let stereo = rig.session.chain_state(0).chain.stereo_enabled;
        let (started_tx, started) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let compiled = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&compiled);
        rig.engine
            .set_prepare_for_test(Arc::new(move |plan: &SessionChainPlan| {
                let _ = started_tx.send(());
                let _ = gate.lock().expect("gate").recv_timeout(LIMIT);
                // The edit and its undo came before this read. Only the plan reaches the chain.
                let chain = plan.prepare();
                if let Ok(chain) = &chain {
                    recorded.lock().expect("record").push(Compiled {
                        generation: chain.change.generation,
                        planned: plan.state().clone(),
                        stereo: chain.change.blocks.snapshot().stereo_enabled,
                    });
                }
                chain
            }));

        // The compile starts for the structure with the copy.
        rig.edit(UiAction::DuplicateBlock(rig.tone.clone()));
        started.recv_timeout(LIMIT).expect("the compile");
        rig.edit(UiAction::SetStereoMode(!stereo));
        rig.edit(UiAction::SetStereoMode(stereo));
        drop(release);
        rig.engine.settle().expect("settle");

        let compiled = compiled.lock().expect("compiled");
        let [chain] = compiled.as_slice() else {
            panic!("one compile, because its chain still matches the session");
        };
        assert_eq!(chain.stereo, stereo, "the chain has no part of the edit");
        assert_eq!(
            chain.planned,
            rig.session.audio_state().expect("state"),
            "the chain reproduces the session"
        );
        assert_eq!(rig.status().published_generation, chain.generation);
        rig.render();
        assert_eq!(rig.status().active_generation, chain.generation);
        assert_eq!(rig.metered_blocks(), 2, "the stream plays the session");
    }

    #[test]
    fn a_drop_waits_for_the_compile_and_ends_every_engine_thread() {
        let rig = Rig::new();
        let token = Arc::new(());
        let kept = Arc::clone(&token);
        let (started_tx, started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let held = held_compile(started_tx, gate);
        rig.engine
            .set_prepare_for_test(Arc::new(move |plan: &SessionChainPlan| {
                let _kept = &kept;
                held(plan)
            }));
        rig.edit(UiAction::DuplicateBlock(rig.tone.clone()));
        started.recv_timeout(LIMIT).expect("a compile");

        let Rig { engine, .. } = rig;
        let engine = Arc::into_inner(engine).expect("one owner");
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(release);
        });
        returns_at_once("the drop", move || drop(engine));
        releaser.join().expect("releaser");

        assert_eq!(
            Arc::strong_count(&token),
            1,
            "a thread of the engine still holds its state"
        );
    }

    #[test]
    fn a_compile_that_panics_shows_a_fault_and_a_later_edit_still_reaches_the_stream() {
        let rig = Rig::new();
        let first = rig.status().published_generation;
        let compiles = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&compiles);
        rig.engine
            .set_prepare_for_test(Arc::new(move |plan: &SessionChainPlan| {
                assert!(
                    counted.fetch_add(1, Ordering::SeqCst) > 0,
                    "this panic is the test"
                );
                plan.prepare()
            }));

        rig.edit(UiAction::DuplicateBlock(rig.tone.clone()));
        let engine = Arc::clone(&rig.engine);
        returns_at_once("the settle", move || engine.settle().expect("settle"));
        assert_eq!(rig.status().published_generation, first);
        assert!(
            rig.engine
                .chain_faults()
                .iter()
                .any(|fault| fault.kind == ChainFaultKind::ChainNotPrepared)
        );
        let error = rig.status().chain_error.expect("a chain error");
        assert!(
            error.contains("this panic is the test"),
            "the error keeps the panic message: {error}"
        );

        rig.edit(UiAction::SetStereoMode(
            !rig.session.chain_state(0).chain.stereo_enabled,
        ));
        rig.engine.settle().expect("settle");
        let ready = rig.status();
        assert!(ready.published_generation > first);
        assert_eq!(ready.chain_error, None);
        assert_eq!(compiles.load(Ordering::SeqCst), 2);
    }
}
