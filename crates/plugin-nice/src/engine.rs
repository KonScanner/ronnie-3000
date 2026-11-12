//! The control side of the plugin audio. It keeps the real-time chain in step with the session.
//!
//! The audio thread owns an [`RtChain`] and never takes a lock here. A worker thread compares the
//! session with the state that the running chain reproduces. It compiles a new chain off the audio
//! thread when the structure changes, and it sends chains and block settings through the bounded
//! queues of the chain. The audio thread makes the controls itself, from the host values. The
//! worker also destroys the chains that the audio thread retires. It runs on a timer, so all of
//! this also happens while the editor is closed.

use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::TrySendError,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use ronie_core::{
    BlockId, BlockParams,
    params::{ParameterId, ParameterValue},
};
use ronie_dsp::{
    command::{ChainCommand, ChainCommandSender},
    prepare::PreparedChainChange,
    rt_chain::RtChain,
};
use ronie_plugin::{AudioSessionState, ProcessorBackend, StreamFormat, UiBackend};

use crate::params::lock;

/// How many commands the chain queue holds. It also limits how many chains exist at one time.
const COMMAND_CAPACITY: usize = 8;
/// How often the worker compares the session with the running chain.
const SYNC_PERIOD: Duration = Duration::from_millis(10);
/// How long the worker waits before it tries a failed chain compile again. The delay doubles with
/// each failure in a row, up to [`MAX_COMPILE_RETRY`].
const COMPILE_RETRY: Duration = Duration::from_secs(2);
/// The longest delay before the worker tries a failed chain compile again.
const MAX_COMPILE_RETRY: Duration = Duration::from_secs(60);

/// The delay before the next try, after `failures` failed compiles in a row. A repair of a failed
/// chain uses the same delay, after `failures` repairs in a row.
fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    COMPILE_RETRY
        .saturating_mul(1 << doublings)
        .min(MAX_COMPILE_RETRY)
}
/// The oversampling factors that the `osFactor` choices select.
const OVERSAMPLING_FACTORS: [u8; 3] = [2, 4, 8];

/// The oversampling that the session parameters select.
#[must_use]
pub(crate) fn oversampling(enabled: Option<ParameterValue>, factor: Option<ParameterValue>) -> u8 {
    if !enabled.is_some_and(|enabled| enabled.0 >= 0.5) {
        return 1;
    }
    let choice = factor.map_or(0.0, |factor| factor.0.round()).max(0.0) as usize;
    OVERSAMPLING_FACTORS[choice.min(OVERSAMPLING_FACTORS.len() - 1)]
}

/// What the worker could not send yet, because a queue was full.
#[derive(Default)]
struct Outbox {
    chain: Option<PreparedChainChange>,
    block_params: Option<Vec<(BlockId, BlockParams)>>,
}

/// The link from the control side to one active audio chain.
struct Link {
    sender: ChainCommandSender,
    format: StreamFormat,
    /// The session state that the chain reproduces, after the queued items arrive.
    mirror: AudioSessionState,
    outbox: Outbox,
    /// When the worker tries again a compile that failed.
    retry_at: Option<Instant>,
    /// How many plans or compiles failed in a row.
    failures: u32,
    /// Whether the audio thread asked for a repair that did not start yet.
    repair_pending: bool,
    /// When the next repair can start. A fault that stays, for example a bad input or a model that
    /// always gives values that are not finite, must not make the worker compile all the time.
    repair_at: Option<Instant>,
    /// How many repairs started in a row.
    repairs: u32,
    /// When the audio thread asked for a repair last.
    last_repair_request: Option<Instant>,
}

impl Link {
    fn new(sender: ChainCommandSender, format: StreamFormat, mirror: AudioSessionState) -> Self {
        Self {
            sender,
            format,
            mirror,
            outbox: Outbox::default(),
            retry_at: None,
            failures: 0,
            repair_pending: false,
            repair_at: None,
            repairs: 0,
            last_repair_request: None,
        }
    }

    /// Records a repair request, if `requested`, and returns whether a repair starts now.
    ///
    /// A request in the delay after a repair stays pending until the delay ends. The delay grows
    /// with each repair in a row, as for a failed compile. A run of repairs ends when no request
    /// comes for [`MAX_COMPILE_RETRY`].
    fn start_repair(&mut self, requested: bool, now: Instant) -> bool {
        if requested {
            let quiet = self
                .last_repair_request
                .is_none_or(|last| now.saturating_duration_since(last) >= MAX_COMPILE_RETRY);
            if quiet {
                self.repairs = 0;
                self.repair_at = None;
            }
            self.last_repair_request = Some(now);
            self.repair_pending = true;
        }
        if !self.repair_pending || self.repair_at.is_some_and(|at| now < at) {
            return false;
        }
        if self.repairs == 0 {
            nice_plug::nice_warn!("Ronnie replaces its audio chain, because the chain failed");
        }
        self.repair_pending = false;
        self.repairs = self.repairs.saturating_add(1);
        self.repair_at = Some(now + retry_delay(self.repairs));
        true
    }

    /// Records a failed plan or compile of `state`. The old chain keeps running.
    ///
    /// The worker tries again after a delay, because a restore can leave the old models playing.
    /// The mirror takes the structure of `state`, so that the worker does not try again at each
    /// pass. It keeps the block settings of the running chain, so a later edit of them still
    /// reaches that chain.
    fn compile_failed(&mut self, error: &str, state: Option<AudioSessionState>, now: Instant) {
        if self.failures == 0 {
            nice_plug::nice_error!("Ronnie could not prepare the audio chain: {error}");
        }
        self.failures = self.failures.saturating_add(1);
        if let Some(state) = state {
            let running = std::mem::take(&mut self.mirror.block_params);
            self.mirror = state;
            self.mirror.block_params = running;
        }
        self.retry_at = Some(now + retry_delay(self.failures));
    }
}

/// The control side of the plugin audio, which the plugin and its worker share.
pub(crate) struct Engine {
    backend: Arc<ProcessorBackend>,
    params: Arc<crate::RonnieParams>,
    extras: crate::controls::SessionExtras,
    link: Mutex<Option<Link>>,
    /// Counts the links, so that a compile for an older link is dropped.
    epoch: AtomicU64,
    /// Whether the running chain failed and must be replaced.
    repair: AtomicBool,
    running: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Engine {
    pub(crate) fn new(
        backend: Arc<ProcessorBackend>,
        params: Arc<crate::RonnieParams>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend,
            params,
            extras: crate::controls::SessionExtras::new(),
            link: Mutex::new(None),
            epoch: AtomicU64::new(0),
            repair: AtomicBool::new(false),
            running: AtomicBool::new(false),
            worker: Mutex::new(None),
        })
    }

    /// The session values that the audio thread reads for its controls.
    pub(crate) const fn extras(&self) -> &crate::controls::SessionExtras {
        &self.extras
    }

    /// The format of a stream with the oversampling that the session selects now.
    pub(crate) fn stream_format(
        &self,
        sample_rate: f64,
        max_frames: usize,
        stereo_input: bool,
    ) -> StreamFormat {
        StreamFormat {
            sample_rate,
            max_frames,
            stereo_input,
            stereo_output: true,
            oversampling: self.wanted_oversampling(),
        }
    }

    /// The oversampling that the session selects now.
    fn wanted_oversampling(&self) -> u8 {
        oversampling(
            self.backend.parameter(ParameterId("osEnabled")),
            self.backend.parameter(ParameterId("osFactor")),
        )
    }

    /// Compiles the chain of the session for `format` and returns its real-time owner.
    ///
    /// Call this on the main thread when the host activates the plugin. It can take some time,
    /// because it compiles the models of the session.
    ///
    /// # Errors
    /// Returns a description of a lock or a chain preparation error.
    pub(crate) fn activate(self: &Arc<Self>, format: StreamFormat) -> Result<RtChain, String> {
        // The new chain reproduces the session from here, so an earlier restore needs no chain.
        // A restore after this point stays for the worker.
        let _ = self.backend.take_restore();
        let plan = self
            .backend
            .chain_plan(format)
            .map_err(|error| error.to_string())?;
        let chain = plan.prepare().map_err(|error| error.to_string())?;
        let mirror = plan.into_state();
        self.extras
            .publish(&self.backend, mirror.controls.routing.input_mode);
        let (mut rt, sender) =
            RtChain::prepare(chain.change, COMMAND_CAPACITY).map_err(|error| error.to_string())?;
        rt.set_controls(mirror.controls);
        self.repair.store(false, Ordering::Release);
        let mut link = lock(&self.link);
        self.epoch.fetch_add(1, Ordering::AcqRel);
        *link = Some(Link::new(sender, format, mirror));
        drop(link);
        self.start_worker();
        Ok(rt)
    }

    /// Forgets the active chain. Call this on the main thread, before the chain drops.
    pub(crate) fn deactivate(&self) {
        let mut link = lock(&self.link);
        self.epoch.fetch_add(1, Ordering::AcqRel);
        if let Some(link) = link.take() {
            link.sender.drain_retired();
        }
    }

    /// Stops the worker and waits for it. Call this after the real-time owner drops.
    pub(crate) fn stop(&self) {
        self.running.store(false, Ordering::Release);
        if let Some(worker) = lock(&self.worker).take() {
            let _ = worker.join();
        }
    }

    fn start_worker(self: &Arc<Self>) {
        let mut worker = lock(&self.worker);
        if worker.is_some() {
            return;
        }
        self.running.store(true, Ordering::Release);
        let engine: Weak<Self> = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("ronie-plugin-chain".to_owned())
            .spawn(move || {
                loop {
                    std::thread::sleep(SYNC_PERIOD);
                    let Some(engine) = engine.upgrade() else {
                        return;
                    };
                    if !engine.running.load(Ordering::Acquire) {
                        return;
                    }
                    engine.sync();
                }
            });
        match spawned {
            Ok(handle) => *worker = Some(handle),
            Err(error) => {
                // With no worker, the audio keeps the first chain and follows only host values.
                nice_plug::nice_error!("Ronnie could not start its chain worker: {error}");
            }
        }
    }

    /// Asks the worker for a new chain of the session, because the running one failed.
    ///
    /// The audio thread calls this. It only stores a flag.
    pub(crate) fn request_repair(&self) {
        self.repair.store(true, Ordering::Release);
    }

    /// Brings the active chain in step with the session.
    ///
    /// A compile can take long, so it runs with no lock. A chain for a link that changed in the
    /// meantime, because the host activated the plugin again, is dropped.
    pub(crate) fn sync(&self) {
        crate::params::take_host_changes(&self.backend, &self.params);
        let Some((epoch, plan)) = self.plan_or_pump() else {
            return;
        };
        let compiled = plan
            .prepare()
            .map(|chain| (chain.change, plan.into_state()));
        let mut guard = lock(&self.link);
        let Some(link) = guard.as_mut() else {
            return;
        };
        if self.epoch.load(Ordering::Acquire) != epoch {
            return;
        }
        match compiled {
            Ok((change, mirror)) => {
                link.outbox.chain = Some(change);
                link.outbox.block_params = None;
                link.mirror = mirror;
                link.failures = 0;
            }
            Err(error) => {
                let state = self.backend.audio_state().ok();
                link.compile_failed(&error.to_string(), state, Instant::now());
            }
        }
        Self::pump(link);
    }

    /// Sends what the session changed to the chain. Returns a plan to compile, when only a new
    /// chain can reproduce the session.
    fn plan_or_pump(&self) -> Option<(u64, ronie_plugin::SessionChainPlan)> {
        let mut guard = lock(&self.link);
        let link = guard.as_mut()?;
        let wanted = self.wanted_oversampling();
        let format_changed = link.format.oversampling != wanted;
        link.format.oversampling = wanted;
        // The state comes first, so that an error here cannot lose a restore.
        let state = self.backend.audio_state().ok()?;
        // A project restore compiles no chain, and it can bring new model bytes for the same
        // structure. So any restore needs a new chain.
        let restored = self.backend.take_restore().ok().flatten().is_some();
        self.extras
            .publish(&self.backend, state.controls.routing.input_mode);
        let now = Instant::now();
        let repair = link.start_repair(self.repair.swap(false, Ordering::AcqRel), now);
        let retry = link.retry_at.is_some_and(|at| now >= at);
        // The audio thread makes the controls from the host values, so no controls go here.
        if repair
            || restored
            || retry
            || format_changed
            || link.mirror.fingerprint != state.fingerprint
        {
            link.retry_at = None;
            match self.backend.chain_plan(link.format) {
                Ok(plan) => {
                    // A new chain for any cause also replaces a chain that failed.
                    link.repair_pending = false;
                    return Some((self.epoch.load(Ordering::Acquire), plan));
                }
                Err(error) => link.compile_failed(&error.to_string(), Some(state), now),
            }
        } else if link.mirror.block_params != state.block_params {
            link.mirror.block_params.clone_from(&state.block_params);
            link.outbox.block_params = Some(state.block_params);
        }
        Self::pump(link);
        None
    }

    /// Sends what the queues can take now, and destroys the retired chains.
    fn pump(link: &mut Link) {
        if let Some(chain) = link.outbox.chain.take()
            && let Err(TrySendError::Full(ChainCommand::Replace(chain))) =
                link.sender.try_send(ChainCommand::Replace(chain))
        {
            link.outbox.chain = Some(chain);
        }
        if link.outbox.chain.is_none()
            && let Some(params) = link.outbox.block_params.take()
            && let Err(TrySendError::Full(ChainCommand::UpdateBlockParams(params))) = link
                .sender
                .try_send(ChainCommand::UpdateBlockParams(params))
        {
            link.outbox.block_params = Some(params);
        }
        link.sender.drain_retired();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ronie_core::InputMode;
    use ronie_dsp::{PlanarBuffer, PrepareSpec};
    use ronie_plugin::RonieProcessor;

    const FRAMES: usize = 256;

    fn engine() -> Arc<Engine> {
        let processor = RonieProcessor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: FRAMES,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        });
        let backend = Arc::new(ProcessorBackend::new(processor));
        Engine::new(backend.clone(), Arc::new(crate::RonnieParams::new(backend)))
    }

    /// Runs `blocks` blocks of a 220 Hz sine and returns the peak of the last block.
    fn peak_after(chain: &mut RtChain, blocks: usize) -> f32 {
        let mut peak = 0.0_f32;
        let mut phase = 0.0_f32;
        for _ in 0..blocks {
            let mut samples = vec![0.0_f32; 2 * FRAMES];
            for frame in 0..FRAMES {
                let value = 0.5 * (phase * std::f32::consts::TAU).sin();
                samples[frame] = value;
                samples[FRAMES + frame] = value;
                phase = (phase + 220.0 / 48_000.0).fract();
            }
            let mut buffer = PlanarBuffer::new(&mut samples, 2, FRAMES).expect("buffer");
            chain.process(&mut buffer).expect("process");
            peak = samples
                .iter()
                .fold(0.0, |peak, sample| peak.max(sample.abs()));
        }
        peak
    }

    /// The host values of the defaults, with `id` set to `host`.
    fn host_values(params: &crate::RonnieParams, id: &str, host: f32) -> Vec<f32> {
        params
            .bindings
            .iter()
            .map(|binding| {
                if binding.spec.id.0 == id {
                    host
                } else {
                    binding.to_host(ParameterValue(binding.spec.default))
                }
            })
            .collect()
    }

    #[test]
    fn a_host_value_reaches_the_chain_at_the_next_block() {
        let engine = engine();
        let format = engine.stream_format(48_000.0, FRAMES, false);
        let mut chain = engine.activate(format).expect("chain");
        let mut controls = crate::controls::HostControls::new(&engine.params, engine.extras());
        let defaults = host_values(&engine.params, "outputLevel", 0.5);
        let first = controls.update_values(defaults.into_iter(), &engine.params, engine.extras());
        chain.set_controls(first.expect("the first update makes controls"));
        let loud = peak_after(&mut chain, 40);
        assert!(loud > 0.0, "the chain passes the sine");

        let silent = host_values(&engine.params, "outputLevel", 0.0);
        let changed =
            controls.update_values(silent.iter().copied(), &engine.params, engine.extras());
        chain.set_controls(changed.expect("a host change makes controls"));
        let quiet = peak_after(&mut chain, 40);

        assert!(quiet < loud * 0.1, "loud {loud}, quiet {quiet}");
        let again = controls.update_values(silent.into_iter(), &engine.params, engine.extras());
        assert!(again.is_none(), "no change makes no controls");
        engine.deactivate();
        drop(chain);
        engine.stop();
    }

    #[test]
    fn a_new_block_structure_reaches_the_chain_through_the_worker() {
        let engine = engine();
        let format = engine.stream_format(48_000.0, FRAMES, false);
        let mut chain = engine.activate(format).expect("chain");
        let generation = chain.active_generation();

        engine
            .backend
            .dispatch(ronie_ui::backend::UiAction::SetStereoMode(true))
            .expect("stereo mode");
        engine.sync();
        let _ = peak_after(&mut chain, 1);

        assert!(
            chain.active_generation() > generation,
            "a new chain is active"
        );
        engine.deactivate();
        drop(chain);
        engine.stop();
    }

    #[test]
    fn a_project_restore_reaches_the_running_chain() {
        // The same structure as the running chain. A restore can bring other model bytes for
        // the same structure, so it needs a new chain in any case.
        let bytes = engine().backend.session_bytes().expect("bytes");
        let engine = engine();
        let format = engine.stream_format(48_000.0, FRAMES, false);
        let mut chain = engine.activate(format).expect("chain");
        let generation = chain.active_generation();

        engine
            .backend
            .restore_host_session_bytes(&bytes)
            .expect("restore");
        engine.sync();
        let _ = peak_after(&mut chain, 1);

        assert!(
            chain.active_generation() > generation,
            "the worker compiled the restored session"
        );
        engine.deactivate();
        drop(chain);
        engine.stop();
    }

    #[test]
    fn a_failed_chain_is_replaced_by_a_fresh_one() {
        let engine = engine();
        let format = engine.stream_format(48_000.0, FRAMES, false);
        let mut chain = engine.activate(format).expect("chain");
        let generation = chain.active_generation();
        engine.sync();
        let _ = peak_after(&mut chain, 1);
        assert_eq!(
            chain.active_generation(),
            generation,
            "no change, no new chain"
        );

        engine.request_repair();
        engine.sync();
        let _ = peak_after(&mut chain, 1);

        assert!(
            chain.active_generation() > generation,
            "the repair made a new chain"
        );
        engine.deactivate();
        drop(chain);
        engine.stop();
    }

    #[test]
    fn a_compile_for_an_older_activation_is_dropped() {
        let engine = engine();
        let format = engine.stream_format(48_000.0, FRAMES, false);
        let _chain = engine.activate(format).expect("chain");
        engine.request_repair();
        let (epoch, _plan) = engine.plan_or_pump().expect("a plan");

        engine.deactivate();

        assert_ne!(engine.epoch.load(Ordering::Acquire), epoch);
        engine.stop();
    }

    #[test]
    fn a_failed_compile_is_tried_again_after_a_growing_delay() {
        let engine = engine();
        let format = engine.stream_format(48_000.0, FRAMES, false);
        let _chain = engine.activate(format).expect("chain");
        engine
            .backend
            .dispatch(ronie_ui::backend::UiAction::SetStereoMode(true))
            .expect("stereo mode");
        let state = engine.backend.audio_state().expect("state");
        let start = Instant::now();
        let mut guard = lock(&engine.link);
        let link = guard.as_mut().expect("link");
        let running = link.mirror.block_params.clone();

        link.compile_failed("test", Some(state.clone()), start);

        assert_eq!(link.retry_at, Some(start + COMPILE_RETRY));
        assert_eq!(
            link.mirror.fingerprint, state.fingerprint,
            "no new try at each pass"
        );
        assert_eq!(
            link.mirror.block_params, running,
            "the running chain keeps its settings"
        );
        link.compile_failed("test", None, start);
        assert_eq!(link.retry_at, Some(start + COMPILE_RETRY * 2));
        for _ in 0..20 {
            link.compile_failed("test", None, start);
        }
        assert_eq!(link.retry_at, Some(start + MAX_COMPILE_RETRY));

        link.retry_at = start.checked_sub(Duration::from_millis(1));
        drop(guard);
        assert!(engine.plan_or_pump().is_some(), "a try after the delay");
        assert!(
            lock(&engine.link)
                .as_ref()
                .expect("link")
                .retry_at
                .is_none(),
            "the try clears the delay"
        );
        engine.deactivate();
        engine.stop();
    }

    #[test]
    fn a_repair_soon_after_a_repair_waits_for_the_delay() {
        let engine = engine();
        let format = engine.stream_format(48_000.0, FRAMES, false);
        let _chain = engine.activate(format).expect("chain");
        engine.request_repair();
        assert!(engine.plan_or_pump().is_some(), "the first repair compiles");

        engine.request_repair();
        assert!(engine.plan_or_pump().is_none(), "a second repair waits");
        assert!(engine.plan_or_pump().is_none(), "it still waits");
        let mut guard = lock(&engine.link);
        let link = guard.as_mut().expect("link");
        assert!(link.repair_pending, "the request stays");
        link.repair_at = Instant::now().checked_sub(Duration::from_millis(1));
        drop(guard);

        assert!(engine.plan_or_pump().is_some(), "a repair after the delay");
        let guard = lock(&engine.link);
        let link = guard.as_ref().expect("link");
        assert_eq!(link.repairs, 2);
        assert!(!link.repair_pending);
        drop(guard);
        engine.deactivate();
        engine.stop();
    }

    #[test]
    fn the_repair_delay_grows_and_starts_again_after_a_quiet_time() {
        let engine = engine();
        let format = engine.stream_format(48_000.0, FRAMES, false);
        let _chain = engine.activate(format).expect("chain");
        let start = Instant::now();
        let mut guard = lock(&engine.link);
        let link = guard.as_mut().expect("link");

        assert!(link.start_repair(true, start));
        assert_eq!(link.repair_at, Some(start + COMPILE_RETRY));
        assert!(!link.start_repair(false, start + COMPILE_RETRY / 2));
        assert!(!link.start_repair(true, start + COMPILE_RETRY / 2));
        let second = start + COMPILE_RETRY;
        assert!(link.start_repair(false, second), "the pending request");
        assert_eq!(link.repair_at, Some(second + COMPILE_RETRY * 2));
        assert!(
            !link.start_repair(false, second + COMPILE_RETRY * 3),
            "no request"
        );

        let quiet = second + COMPILE_RETRY / 2 + MAX_COMPILE_RETRY;
        assert!(link.start_repair(true, quiet), "a new run");
        assert_eq!(link.repairs, 1);
        assert_eq!(link.repair_at, Some(quiet + COMPILE_RETRY));
        drop(guard);
        engine.deactivate();
        engine.stop();
    }

    #[test]
    fn the_session_parameters_select_the_oversampling() {
        let on = Some(ParameterValue(1.0));
        assert_eq!(oversampling(None, None), 1);
        assert_eq!(
            oversampling(Some(ParameterValue(0.0)), Some(ParameterValue(2.0))),
            1
        );
        assert_eq!(oversampling(on, Some(ParameterValue(0.0))), 2);
        assert_eq!(oversampling(on, Some(ParameterValue(2.0))), 8);
        assert_eq!(oversampling(on, Some(ParameterValue(9.0))), 8);
    }
}
