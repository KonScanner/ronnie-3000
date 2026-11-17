//! The state of the engine, which only one thread at a time changes.
//!
//! Each method here runs under the engine lock and returns soon. Slow work leaves the lock as a
//! [`DeviceJob`] or as a [`DeferredChain`], and the threads of [`super::AudioEngine`] do it.

use std::{
    collections::BTreeMap,
    ops::RangeInclusive,
    sync::{Arc, Mutex, PoisonError, mpsc::TrySendError},
    time::{Duration, Instant},
};

use ronie_core::{
    BlockId, BlockParams, ChainSide, OPTIMIZE_PREFERENCE, optimize_enabled,
    params::{ParameterId, ParameterValue},
};
use ronie_dsp::{
    block::SpectrumTap,
    prepare::PreparedChainChange,
    rt_chain::{ChainCommand, ChainControls},
};
use ronie_plugin::{
    AudioSessionState, ProcessorBackend, SessionChain, SessionChainPlan, SessionRestore,
    StreamFormat,
};
use ronie_ui::{
    backend::{
        AudioDeviceConfig, BlockMeter, ChainFault, ChainFaultKind, DeviceAction, MeterSnapshot,
        UiBackend, UiError,
    },
    default::{
        AudioDeviceOption, AudioDeviceState, AudioInputChannel, MidiInputDevice, SystemDeviceState,
    },
};

use crate::audio::{
    AudioBackend, AudioBackendKind, AudioError, AudioEvent, AudioEvents, AudioStream,
    AudioTelemetry, AudioWorker, COMMON_SAMPLE_RATES, CpalBackend, DEFAULT_BUFFER_FRAMES,
    DeviceCapabilities, DeviceConfig, DeviceDescriptor, DummyBackend, NegotiatedStream,
    NonFiniteReport, ResolvedDevice, ResolvedRoute, Route, SYSTEM_DEFAULT_DEVICE,
    buffer_optimizer::{BufferChoice, BufferOptimizer, chosen_frames},
    delay::{DelayParts, round_trip_ms},
    devices::fit_layout,
    resolve_route, system_default_label,
};
use crate::{
    audio_settings::StandaloneAudioSettings, diagnostics::DiagnosticLog, midi::MidiInputManager,
};

/// Banner text for a failed stream on the system default device, which has no name yet.
const DEFAULT_DEVICE_LABEL: &str = "System default";

/// The short state that the page shows while the worker opens a stream.
const OPENING_TEXT: &str = "Opening…";

/// The callback sizes that the settings can offer, before a device filters them.
const COMMON_BUFFER_FRAMES: [u32; 6] = [32, 64, 128, 256, 512, 1024];

/// How long device work can run before the page stops waiting for it. A sound server that does
/// not answer holds its job for this time, and then the user can choose another device.
const DEVICE_WORK_LIMIT: Duration = Duration::from_secs(15);

/// Shortest time between chain replacements after a repaired output. A model that always
/// produces non-finite samples stays silent without a compile for each frame.
const MIN_REPAIR_INTERVAL: Duration = Duration::from_secs(1);

/// Quiet time before a non-finite fault ends. A model can fail only on some notes, so that a
/// shorter time makes the banner and the window height flicker while the user plays.
const NON_FINITE_HOLD: Duration = Duration::from_secs(5);

/// The oversampling factor of each choice of the `osFactor` parameter.
const OVERSAMPLING_FACTORS: [u8; 3] = [2, 4, 8];

/// Shortest time between log lines about callback events, which can repeat in each callback.
const EVENT_NOTE_INTERVAL: Duration = Duration::from_secs(10);

/// What the log says about each callback event.
const EVENT_NOTES: [(AudioEvent, &str); 4] = [
    (
        AudioEvent::ProcessingFailed,
        "a block failed and runs as bypass",
    ),
    (
        AudioEvent::NonFiniteOutput,
        "the chain made non-finite audio",
    ),
    (AudioEvent::OutputUnderrun, "the output had an underrun"),
    (AudioEvent::InputOverrun, "the input had an overrun"),
];

/// Observable engine state for diagnostics and acceptance tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEngineStatus {
    /// Generation of the chain that the latest callback used.
    pub active_generation: u64,
    /// Generation of the newest chain the engine has handed to the callback queue.
    pub published_generation: u64,
    /// Whether a change waits for capacity in the callback queue.
    pub publish_pending: bool,
    /// Total retired chains and parameter sets destroyed on the control thread.
    pub retired_chains: u64,
    /// Blocks that run as bypass because their model has no usable bytes.
    pub unavailable_blocks: Vec<BlockId>,
    /// Latest chain preparation error. The previous chain continues to run.
    pub chain_error: Option<String>,
}

/// Changes that wait for capacity. Each slot holds only the newest value, and the newest
/// chain replaces any older chain, so that a full queue can delay a change but never lose it.
#[derive(Default)]
struct Outbox {
    chain: Option<PreparedChainChange>,
    block_params: Option<Vec<(BlockId, BlockParams)>>,
    controls: Option<ChainControls>,
}

impl Outbox {
    fn is_empty(&self) -> bool {
        self.chain.is_none() && self.block_params.is_none() && self.controls.is_none()
    }
}

struct RunningStream {
    stream: AudioStream,
    negotiated: NegotiatedStream,
    format: StreamFormat,
    /// The session state that the callback has or will have after the outbox empties.
    mirror: AudioSessionState,
}

/// One chain that the chain worker prepares outside the engine lock.
///
/// The engine lock read the plan and its session state together, and the compile reads only the
/// plan. So the chain always reproduces the state of the plan, also when an edit and its undo
/// come during the compile.
pub(super) struct DeferredChain {
    format: StreamFormat,
    plan: SessionChainPlan,
    /// [`EngineState::rebuild_serial`] when the plan was read. A rebuild request after that read
    /// must get a chain of its own.
    rebuild_serial: u64,
}

impl DeferredChain {
    /// What the compile reads.
    pub(super) const fn plan(&self) -> &SessionChainPlan {
        &self.plan
    }
}

/// The backend of the device work. Only device work holds this lock, and never a frame.
type SharedBackend = Arc<Mutex<Box<dyn AudioBackend + Send>>>;

/// What makes the backend of one driver.
pub type AudioBackendFactory =
    Arc<dyn Fn(AudioBackendKind) -> Box<dyn AudioBackend + Send> + Send + Sync>;

/// The compile of one chain plan. It runs with no engine lock, and a test can hold it open.
pub(crate) type PrepareChain =
    Arc<dyn Fn(&SessionChainPlan) -> Result<SessionChain, UiError> + Send + Sync>;

/// What a meter reads with no engine lock: the telemetry of the running stream and the blocks at
/// the positions of the newest chain.
#[derive(Default)]
pub(super) struct StreamView {
    telemetry: Option<Arc<AudioTelemetry>>,
    /// Under this generation, the positions of the running chain do not match `blocks`.
    structure_floor: u64,
    /// The blocks that have a meter, at their chain positions.
    blocks: Vec<(ChainSide, usize, BlockId)>,
    spectrum_taps: Vec<(BlockId, Arc<SpectrumTap>)>,
}

impl StreamView {
    /// The levels and the load of the running stream, from its atomics.
    ///
    /// Without a stream, each meter is at its floor. The stereo correlation stays at its default,
    /// because nothing measures it.
    pub(super) fn meter_snapshot(&self) -> MeterSnapshot {
        let Some(telemetry) = &self.telemetry else {
            return MeterSnapshot::default();
        };
        let positions_match = telemetry.active_generation() >= self.structure_floor;
        let blocks = self
            .blocks
            .iter()
            .filter(|_| positions_match)
            .filter_map(|(side, position, block_id)| {
                let (input_db, output_db) = telemetry.block_level_db(*side, *position)?;
                Some(BlockMeter {
                    block_id: block_id.clone(),
                    input_db,
                    output_db,
                })
            })
            .collect();
        MeterSnapshot {
            input_db: telemetry.input_levels_db(),
            output_db: telemetry.output_levels_db(),
            blocks,
            cpu: telemetry.callback_load(),
            ..MeterSnapshot::default()
        }
    }

    pub(super) fn telemetry(&self) -> Option<Arc<AudioTelemetry>> {
        self.telemetry.clone()
    }

    pub(super) fn spectrum_tap(&self, block: &BlockId) -> Option<Arc<SpectrumTap>> {
        self.telemetry.as_ref()?;
        self.spectrum_taps
            .iter()
            .find(|(id, _)| id == block)
            .map(|(_, tap)| Arc::clone(tap))
    }
}

/// The newest [`StreamView`]. Its lock covers only one pointer copy, so a reader never waits for
/// engine work.
#[derive(Default)]
pub(super) struct ViewSlot(Mutex<Arc<StreamView>>);

impl ViewSlot {
    pub(super) fn load(&self) -> Arc<StreamView> {
        Arc::clone(&self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn store(&self, view: StreamView) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(view);
    }
}

/// Owner of the running stream. It keeps the stream equal to the processor session through
/// every change, reopen, and device fault.
///
/// No method here touches a device or compiles a new structure while a chain worker runs. Slow
/// work leaves in a [`DeviceJob`] or a [`DeferredChain`], which runs with no engine lock.
pub(super) struct EngineState {
    session: ProcessorBackend,
    backend: SharedBackend,
    make_backend: AudioBackendFactory,
    kind: AudioBackendKind,
    settings: Arc<StandaloneAudioSettings>,
    config: DeviceConfig,
    /// What the last device work found.
    inventory: DeviceInventory,
    /// The next job must open a stream.
    open_wanted: bool,
    /// The next job must find the devices again.
    list_wanted: bool,
    /// The next job must save the configuration.
    save_wanted: bool,
    /// The next job must save the Hear Yourself choice.
    save_hear_wanted: bool,
    /// Streams to close with no engine lock.
    retired: Vec<AudioStream>,
    /// The newest job. An outcome of an older job says nothing about the devices now.
    job_serial: u64,
    /// When the running job started.
    job_started: Option<Instant>,
    /// Whether a worker thread owns the jobs.
    worker_claimed: bool,
    /// The worker thread that owns the jobs, and no thread that the engine gave up on.
    worker_id: u64,
    work_limit: Duration,
    /// Labels of the devices of the last stream, for the banner after it ends.
    last_labels: Option<(String, String)>,
    running: Option<RunningStream>,
    last_negotiated: Option<NegotiatedStream>,
    last_repair: Option<Instant>,
    device: SystemDeviceState,
    outbox: Outbox,
    published_generation: u64,
    retired_chains: u64,
    /// Blocks of the newest chain that run as bypass, with the fault that each one shows.
    unavailable: Vec<(BlockId, ChainFault)>,
    chain_error: Option<String>,
    /// Whether a chain worker thread compiles each new structure. Without one, `sync` compiles on
    /// the calling thread.
    pub(super) chain_worker: bool,
    /// The chain worker must prepare the chain of the current structure, so `sync` must not.
    chain_deferred: bool,
    /// The chain worker must prepare a chain also when the structure did not change, because a
    /// restored chain does not fit the stream.
    rebuild_wanted: bool,
    /// Counts the rebuild requests, so that a compile ends only the requests that came before
    /// its plan.
    rebuild_serial: u64,
    /// The chain worker waits for the next settle, because a caller asked for that order.
    chain_held: bool,
    /// The chain worker prepares a chain now, with no engine lock.
    compiling: bool,
    /// Chains that this state does not need. The chain worker drops them with no engine lock.
    unused_chains: Vec<PreparedChainChange>,
    /// The running chain made a non-finite output, and the chain worker must replace it.
    repair_wanted: bool,
    /// The engine stops. No worker starts new work.
    shutdown: bool,
    /// The compile that the chain worker runs.
    prepare: PrepareChain,
    /// What meters read with no engine lock.
    view: Arc<ViewSlot>,
    /// The view does not show the newest stream, structure, or taps yet.
    view_dirty: bool,
    /// The non-finite fault that the banner shows, and the time of its latest event.
    non_finite: Option<(ChainFault, Instant)>,
    /// The blocks of the running chain that failed in a callback and now run as bypass.
    failed_blocks: Vec<ChainFault>,
    /// Generation of the newest chain for a new structure. A report from an older chain tells
    /// nothing about the session.
    structure_floor: u64,
    /// The lock-free output of each block of the newest chain.
    spectrum_taps: Vec<(BlockId, Arc<SpectrumTap>)>,
    log: Option<DiagnosticLog>,
    /// Callback events that the log has not reported.
    events_to_note: AudioEvents,
    last_event_note: Option<Instant>,
    /// Whether OPTIMIZE is on. The engine reads the setting from the session after each edit
    /// and gives it to the callback as an atomic flag, so that the callback takes no lock.
    optimize: bool,
    /// What OPTIMIZE knows about the buffer of the running stream.
    optimizer: BufferOptimizer,
    /// The buffer that OPTIMIZE found for each device pair.
    optimized_buffers: BTreeMap<String, u32>,
    /// The next job must save `optimized_buffers`.
    save_buffers_wanted: bool,
    /// The next open probes the buffer again: the device changed, or OPTIMIZE came on.
    probe_wanted: bool,
    /// The buffer of the next open with OPTIMIZE: a step up, or a choice of the user.
    forced_buffer: Option<u32>,
    /// The input underrun count of the running stream at the latest poll.
    seen_input_underruns: u32,
    /// Whether OPTIMIZE also chooses the buffer of a dummy stream, as a test of a real device.
    optimize_dummy_buffers: bool,
}

impl EngineState {
    /// Prepares a state that has no stream yet, and touches no device.
    ///
    /// The first [`DeviceJob`] finds the devices and opens the stream, so that a window can open
    /// before the sound server answers.
    pub(super) fn deferred(
        session: ProcessorBackend,
        settings: StandaloneAudioSettings,
        make_backend: AudioBackendFactory,
        kind: AudioBackendKind,
        log: Option<DiagnosticLog>,
    ) -> Self {
        let mut engine = Self::idle(session, settings, make_backend, kind, log);
        engine.want_open(engine.config.clone(), false);
        engine
    }

    /// Opens the stream on this thread. This fits a test and a dummy stream, and not a real
    /// device.
    ///
    /// # Errors
    /// Returns a device-enumeration error, or any open error on the dummy backend.
    pub(super) fn started(
        session: ProcessorBackend,
        settings: StandaloneAudioSettings,
        make_backend: AudioBackendFactory,
        kind: AudioBackendKind,
        log: Option<DiagnosticLog>,
    ) -> Result<Self, UiError> {
        let mut engine = Self::deferred(session, settings, make_backend, kind, log);
        let result = engine.run_device_work_here();
        if engine.inventory.resolved.is_none() || kind == AudioBackendKind::Dummy {
            result?;
        }
        Ok(engine)
    }

    fn idle(
        session: ProcessorBackend,
        settings: StandaloneAudioSettings,
        make_backend: AudioBackendFactory,
        kind: AudioBackendKind,
        log: Option<DiagnosticLog>,
    ) -> Self {
        let mut config = settings.config().clone();
        config.backend = kind;
        let inventory = DeviceInventory::default();
        let mut device = snapshot(SnapshotInput {
            kind,
            effective: &config,
            inventory: &inventory,
            previous: None,
            open: false,
            channels: None,
        });
        // The first job gives this choice to the stream before its first callback, so that a
        // start with Hear Yourself off plays no input.
        device.banner.hear_yourself = settings.hear_yourself();
        let optimize = optimize_enabled(session.preference(OPTIMIZE_PREFERENCE));
        let optimized_buffers = settings.optimized_buffers().clone();
        Self {
            session,
            backend: Arc::new(Mutex::new(make_backend(kind))),
            make_backend,
            kind,
            settings: Arc::new(settings),
            config,
            inventory,
            open_wanted: false,
            list_wanted: false,
            save_wanted: false,
            save_hear_wanted: false,
            retired: Vec::new(),
            job_serial: 0,
            job_started: None,
            worker_claimed: false,
            worker_id: 0,
            work_limit: DEVICE_WORK_LIMIT,
            last_labels: None,
            running: None,
            last_negotiated: None,
            last_repair: None,
            device,
            outbox: Outbox::default(),
            published_generation: 0,
            retired_chains: 0,
            unavailable: Vec::new(),
            chain_error: None,
            chain_worker: false,
            chain_deferred: false,
            rebuild_wanted: false,
            rebuild_serial: 0,
            chain_held: false,
            compiling: false,
            unused_chains: Vec::new(),
            repair_wanted: false,
            shutdown: false,
            prepare: Arc::new(SessionChainPlan::prepare),
            view: Arc::default(),
            view_dirty: false,
            non_finite: None,
            failed_blocks: Vec::new(),
            structure_floor: 0,
            spectrum_taps: Vec::new(),
            log,
            events_to_note: AudioEvents::default(),
            last_event_note: None,
            optimize,
            optimizer: BufferOptimizer::default(),
            optimized_buffers,
            save_buffers_wanted: false,
            probe_wanted: false,
            forced_buffer: None,
            seen_input_underruns: 0,
            optimize_dummy_buffers: false,
        }
    }

    /// The slot where this state publishes what meters read.
    pub(super) fn view_slot(&self) -> Arc<ViewSlot> {
        Arc::clone(&self.view)
    }

    /// Publishes a new [`StreamView`] when the stream, the structure, or the taps changed.
    ///
    /// [`Self::pump`] calls this before it sends a chain, so that no meter reads the positions of
    /// a chain that the callback has with the blocks of an older chain.
    pub(super) fn publish_view(&mut self) {
        if !std::mem::take(&mut self.view_dirty) {
            return;
        }
        let view = self
            .running
            .as_ref()
            .map_or_else(StreamView::default, |running| {
                let fingerprint = &running.mirror.fingerprint;
                let blocks = [ChainSide::Left, ChainSide::Right]
                    .into_iter()
                    .flat_map(|side| {
                        (0..)
                            .map_while(move |position| {
                                fingerprint
                                    .block(side, position)
                                    .map(|block| (side, position, block))
                            })
                            .filter(|(_, _, block)| !block.is_insert)
                            .map(|(side, position, block)| (side, position, block.id.clone()))
                    })
                    .collect();
                StreamView {
                    telemetry: Some(running.stream.shared_telemetry()),
                    structure_floor: self.structure_floor,
                    blocks,
                    spectrum_taps: self.spectrum_taps.clone(),
                }
            });
        self.view.store(view);
    }

    /// Replaces the compile of the chain worker.
    #[cfg(test)]
    pub(super) fn set_prepare(&mut self, prepare: PrepareChain) {
        self.prepare = prepare;
    }

    fn note(&self, message: &str) {
        if let Some(log) = &self.log {
            log.write(message);
        }
    }

    /// Reports the outcome of one stream open. No line holds more than the device state shows.
    fn note_stream(&self, error: Option<String>) {
        let banner = &self.device.banner;
        match error {
            Some(error) => self.note(&format!("stream did not open: {error}")),
            None => self.note(&format!(
                "stream opened: {} input \"{}\" output \"{}\" {} Hz {} frames",
                banner.current_type,
                banner.input_device,
                banner.output_device,
                banner.sample_rate,
                banner.buffer_size,
            )),
        }
    }

    /// Reports callback events, at most one time in each [`EVENT_NOTE_INTERVAL`].
    fn note_events(&mut self, events: AudioEvents) {
        self.events_to_note = self.events_to_note.with(events);
        let waiting = EVENT_NOTES
            .iter()
            .any(|(event, _)| self.events_to_note.contains(*event));
        if !waiting
            || self
                .last_event_note
                .is_some_and(|note| note.elapsed() < EVENT_NOTE_INTERVAL)
        {
            return;
        }
        for (event, text) in EVENT_NOTES {
            if self.events_to_note.contains(event) {
                self.note(text);
            }
        }
        self.events_to_note = AudioEvents::default();
        self.last_event_note = Some(Instant::now());
    }

    /// Latest complete device state for settings and banners.
    #[must_use]
    pub(super) const fn device_state(&self) -> &SystemDeviceState {
        &self.device
    }

    /// Current observable engine state.
    #[must_use]
    pub(super) fn status(&self) -> AudioEngineStatus {
        AudioEngineStatus {
            active_generation: self
                .running
                .as_ref()
                .map_or(0, |running| running.stream.telemetry().active_generation()),
            published_generation: self.published_generation,
            publish_pending: !self.outbox.is_empty(),
            retired_chains: self.retired_chains,
            unavailable_blocks: self.unavailable.iter().map(|(id, _)| id.clone()).collect(),
            chain_error: self.chain_error.clone(),
        }
    }

    /// Current audio chain faults, worst first, from state that the engine already holds.
    ///
    /// Without a stream, no chain runs. The device banners then report the lost stream, and the
    /// next open prepares a chain and reports its faults.
    #[must_use]
    pub(super) fn chain_faults(&self) -> Vec<ChainFault> {
        if self.running.is_none() {
            return Vec::new();
        }
        let non_finite = self.non_finite.iter().map(|(fault, _)| fault.clone());
        let failed = self.failed_blocks.iter().cloned();
        let bypassed = self.unavailable.iter().map(|(_, fault)| fault.clone());
        let not_prepared = self
            .chain_error
            .iter()
            .map(|_| ChainFault::for_chain(ChainFaultKind::ChainNotPrepared));
        non_finite
            .chain(failed)
            .chain(bypassed)
            .chain(not_prepared)
            .collect()
    }

    /// Brings the running stream up to date with the processor session.
    ///
    /// With a chain worker, a new structure only asks that worker for a chain, so this call
    /// compiles nothing. The callback keeps the old chain until the new one is ready. A failed
    /// chain preparation keeps the previous chain and appears in [`Self::status`].
    ///
    /// # Errors
    /// Returns an error when the processor state is unavailable.
    pub(super) fn sync(&mut self) -> Result<(), UiError> {
        self.follow_optimize();
        let restore = self.session.take_restore()?;
        // A new oversampling needs a new chain for the same structure.
        let oversampling = self.wanted_oversampling();
        let mut format_changed = false;
        if let Some(running) = self.running.as_mut()
            && running.format.oversampling != oversampling
        {
            running.format.oversampling = oversampling;
            format_changed = true;
        }
        // Without a stream the session stays the truth, and the next open reads it.
        let Some(format) = self.running.as_ref().map(|running| running.format) else {
            return Ok(());
        };
        self.session.set_stream_format(format)?;
        let state = self.session.audio_state()?;
        let Some(running) = self.running.as_mut() else {
            return Ok(());
        };
        let structure_changed = running.mirror.fingerprint != state.fingerprint;
        let params_changed = running.mirror.block_params != state.block_params;
        let controls_changed = running.mirror.controls != state.controls;
        running.mirror.block_params.clone_from(&state.block_params);
        running.mirror.controls = state.controls;
        // A restore with no chain, or with a chain for an older layout, asks for a new chain of
        // the session, also for the same structure.
        let restored = restore.is_some();
        let candidate = match restore {
            Some(SessionRestore::Prepared(change)) => Some(*change),
            Some(SessionRestore::Compile) | None => None,
        };
        let chain_ready =
            if let Some(change) = candidate.filter(|change| spec_matches(change, format)) {
                self.outbox.block_params = None;
                self.unavailable.clear();
                self.chain_error = None;
                self.spectrum_taps = change.spectrum_taps();
                self.view_dirty = true;
                self.outbox.chain = Some(change);
                self.forget_non_finite();
                self.repair_wanted = false;
                true
            } else if structure_changed || restored || format_changed {
                self.want_chain(!structure_changed)
            } else {
                if params_changed {
                    self.outbox.block_params = Some(state.block_params);
                }
                true
            };
        // The old fingerprint stays until a chain for the new one is ready, so the meters and the
        // faults keep the positions of the running chain, and the next sync tries again.
        if chain_ready && let Some(running) = self.running.as_mut() {
            running.mirror.fingerprint = state.fingerprint;
            self.view_dirty = true;
        }
        if controls_changed {
            self.outbox.controls = Some(state.controls);
        }
        self.pump();
        Ok(())
    }

    /// Reads the OPTIMIZE setting of the session, and gives a change to the running callback.
    /// The session holds the setting in memory, so that this reads no file.
    fn follow_optimize(&mut self) {
        let optimize = optimize_enabled(self.session.preference(OPTIMIZE_PREFERENCE));
        if optimize == self.optimize {
            return;
        }
        self.optimize = optimize;
        if let Some(running) = &self.running {
            running.stream.telemetry().set_optimize(optimize);
        }
        if optimize && self.optimizes_buffer() {
            // Only a new probe can give a smaller buffer again.
            self.probe_wanted = true;
            self.want_open(self.config.clone(), false);
        } else if !optimize && self.optimizer.running().is_some() {
            // The stream goes back to the buffer of the user.
            self.optimizer.stop();
            self.probe_wanted = false;
            self.forced_buffer = None;
            self.want_open(self.config.clone(), false);
        }
    }

    /// The oversampling of the next chain, from OPTIMIZE and the oversampling setting.
    fn wanted_oversampling(&self) -> u8 {
        stream_oversampling(
            self.optimize,
            self.session.parameter(ParameterId("osEnabled")),
            self.session.parameter(ParameterId("osFactor")),
        )
    }

    /// Whether OPTIMIZE chooses the device buffer. A dummy stream keeps its buffer, because it
    /// has no real underruns.
    fn optimizes_buffer(&self) -> bool {
        self.optimize && (self.kind != AudioBackendKind::Dummy || self.optimize_dummy_buffers)
    }

    /// Records the buffer of a stream that a job opened, and stores a new result for its device.
    fn record_buffer(&mut self, choice: Option<BufferChoice>) {
        let Some(mut choice) = choice else {
            self.optimizer.stop();
            return;
        };
        if let Some(running) = &self.running {
            choice.frames = running.negotiated.max_callback_frames;
        }
        if self.optimized_buffers.get(&choice.device) != Some(&choice.frames) {
            self.optimized_buffers
                .insert(choice.device.clone(), choice.frames);
            self.save_buffers_wanted = true;
        }
        self.note(&format!(
            "OPTIMIZE uses a buffer of {} frames",
            choice.frames
        ));
        self.optimizer.installed(choice, Instant::now());
    }

    /// Opens the stream again with a larger buffer when the underruns pass the limit.
    fn grow_buffer_after_underrun(&mut self) {
        if !self.optimizes_buffer() || self.open_wanted {
            return;
        }
        let Some(frames) = self.optimizer.underrun(Instant::now()) else {
            return;
        };
        self.note(&format!(
            "the buffer grows to {frames} frames after underruns"
        ));
        self.forced_buffer = Some(frames);
        self.want_open(self.config.clone(), false);
    }

    /// Asks for a chain of the current session. Returns whether that chain is already queued.
    ///
    /// With a chain worker, this only leaves the chain to that worker. Without one, this compiles
    /// on the calling thread.
    fn want_chain(&mut self, rebuild: bool) -> bool {
        if self.chain_worker {
            self.chain_deferred = true;
            if rebuild {
                self.rebuild_wanted = true;
                self.rebuild_serial += 1;
            }
            return false;
        }
        self.outbox.block_params = None;
        let ready = self.queue_session_chain();
        if ready {
            self.forget_non_finite();
            self.repair_wanted = false;
        }
        ready
    }

    /// Ends a deferral with no chain, so that the next [`Self::sync`] asks for the chain again.
    ///
    /// This also ends a repair request. The next non-finite event after the rate limit asks again.
    pub(super) fn abandon_deferred_chain(&mut self) {
        self.chain_deferred = false;
        self.rebuild_wanted = false;
        self.repair_wanted = false;
        self.compiling = false;
    }

    /// Whether the chain worker has a chain to prepare.
    pub(super) const fn chain_work_wanted(&self) -> bool {
        !self.shutdown
            && !self.chain_held
            && (self.chain_deferred || self.rebuild_wanted || self.repair_wanted)
    }

    /// Keeps the chain worker from new work until [`Self::release_chain`].
    pub(super) const fn hold_chain(&mut self) {
        self.chain_held = true;
    }

    /// Lets the chain worker take the work that waits.
    pub(super) const fn release_chain(&mut self) {
        self.chain_held = false;
    }

    /// Whether device work or chain work waits or runs, so that the stream does not match the
    /// session yet.
    pub(super) const fn busy(&self) -> bool {
        self.device_work_pending()
            || (self.chain_worker && (self.compiling || self.chain_work_wanted()))
    }

    /// Whether the chain worker must still prepare a chain for the current structure. A repair
    /// does not count, because the stream already plays this structure.
    pub(super) const fn structure_work_pending(&self) -> bool {
        self.chain_worker
            && !self.shutdown
            && !self.chain_held
            && (self.chain_deferred || self.rebuild_wanted)
    }

    /// Whether a repair waits for the chain worker.
    #[cfg(test)]
    pub(super) const fn repair_wanted(&self) -> bool {
        self.repair_wanted
    }

    /// Reads the plan of the chain that the stream needs, or returns `None` when the stream needs
    /// no new chain.
    ///
    /// The plan and its session state come from one lock of the processor, in this engine lock.
    /// The chain worker compiles the plan with no engine lock, and then calls
    /// [`Self::finish_deferred_chain`].
    ///
    /// # Errors
    /// Returns an error when the processor state is unavailable.
    pub(super) fn begin_deferred_chain(&mut self) -> Result<Option<DeferredChain>, UiError> {
        if !self.chain_deferred && !self.rebuild_wanted && !self.repair_wanted {
            return Ok(None);
        }
        let Some(format) = self.running.as_ref().map(|running| running.format) else {
            self.abandon_deferred_chain();
            return Ok(None);
        };
        // A plan copies the whole session and reserves a generation. An edit and its undo can
        // leave the stream on the structure of the session, so a deferral first compares the
        // smaller session state, and takes a plan only for a structure that the stream does not
        // play. The check after the plan stays, because the session can change between the reads.
        if !self.repair_wanted && !self.rebuild_wanted && self.stream_plays_session()? {
            self.chain_deferred = false;
            return Ok(None);
        }
        let plan = self.session.chain_plan(format)?;
        // A repair and a rebuild need a new chain for the same structure.
        if !self.repair_wanted
            && !self.rebuild_wanted
            && self
                .running
                .as_ref()
                .is_some_and(|running| running.mirror.fingerprint == plan.state().fingerprint)
        {
            self.chain_deferred = false;
            return Ok(None);
        }
        self.compiling = true;
        Ok(Some(DeferredChain {
            format,
            plan,
            rebuild_serial: self.rebuild_serial,
        }))
    }

    /// Whether the running stream already plays the structure of the session.
    fn stream_plays_session(&self) -> Result<bool, UiError> {
        let current = self.session.audio_state()?;
        Ok(self
            .running
            .as_ref()
            .is_some_and(|running| running.mirror.fingerprint == current.fingerprint))
    }

    /// The compile of each deferred chain, which the chain worker runs with no engine lock.
    pub(super) fn compile_tool(&self) -> PrepareChain {
        Arc::clone(&self.prepare)
    }

    /// Queues a chain that was prepared with no engine lock. Returns false when the structure or
    /// the stream layout changed after the plan: the chain is stale, so the caller prepares again.
    ///
    /// The chain reproduces the state of its plan. So an edit and its undo during the compile do
    /// not make it stale, and a sync sends each block setting that changed after the plan.
    ///
    /// A failed preparation keeps the previous chain and appears in [`Self::status`]. A chain
    /// that this call does not queue waits in [`Self::take_unused_chains`].
    ///
    /// # Errors
    /// Returns an error when the processor state is unavailable.
    pub(super) fn finish_deferred_chain(
        &mut self,
        deferred: DeferredChain,
        prepared: Result<SessionChain, UiError>,
    ) -> Result<bool, UiError> {
        self.compiling = false;
        let DeferredChain {
            format: planned,
            plan,
            rebuild_serial,
        } = deferred;
        let state = plan.into_state();
        let Some(format) = self.running.as_ref().map(|running| running.format) else {
            self.abandon_deferred_chain();
            self.keep_unused(prepared);
            return Ok(true);
        };
        let current = match self.session.audio_state() {
            Ok(current) => current,
            Err(error) => {
                self.keep_unused(prepared);
                return Err(error);
            }
        };
        if format != planned || current.fingerprint != state.fingerprint {
            self.keep_unused(prepared);
            return Ok(false);
        }
        self.chain_deferred = false;
        if self.rebuild_serial == rebuild_serial {
            self.rebuild_wanted = false;
        }
        if std::mem::take(&mut self.repair_wanted) {
            // A compile that takes longer than the limit must not fill all the time.
            self.last_repair = Some(Instant::now());
        }
        let chain = match prepared {
            Ok(chain) => chain,
            Err(error) => {
                self.note(&format!("chain preparation failed: {error}"));
                self.chain_error = Some(error.to_string());
                return Ok(true);
            }
        };
        // A new stream or a repair can queue a newer chain during the preparation.
        let newest = self
            .outbox
            .chain
            .as_ref()
            .map_or(self.published_generation, |change| change.generation);
        if chain.change.generation <= newest {
            self.unused_chains.push(chain.change);
        } else {
            self.unavailable = chain.unavailable;
            self.chain_error = None;
            self.spectrum_taps = chain.change.spectrum_taps();
            if let Some(older) = self.outbox.chain.replace(chain.change) {
                self.unused_chains.push(older);
            }
            self.outbox.block_params = None;
            // A repair keeps the structure, so a report from before it still tells the truth.
            let new_structure = self
                .running
                .as_ref()
                .is_some_and(|running| running.mirror.fingerprint != state.fingerprint);
            if new_structure {
                self.forget_non_finite();
            }
            if let Some(running) = self.running.as_mut() {
                // The chain has the settings of its plan. The sync below sends newer settings.
                running.mirror.fingerprint = state.fingerprint;
                running.mirror.block_params = state.block_params;
            }
            self.view_dirty = true;
        }
        self.sync()?;
        Ok(true)
    }

    /// Keeps a chain that the engine does not queue, so that no engine lock covers its drop.
    fn keep_unused(&mut self, prepared: Result<SessionChain, UiError>) {
        if let Ok(chain) = prepared {
            self.unused_chains.push(chain.change);
        }
    }

    /// Takes the chains that the engine does not need. The caller drops them with no engine lock.
    pub(super) fn take_unused_chains(&mut self) -> Vec<PreparedChainChange> {
        std::mem::take(&mut self.unused_chains)
    }

    /// Gives the engine control-thread time: destroys retired chains, retries waiting changes,
    /// and reads faults from the callback.
    pub(super) fn poll(&mut self) {
        self.pump();
        self.give_up_on_a_stuck_job();
        self.device.banner.delay_ms = self.delay_estimate();
        let Some(running) = &self.running else {
            return;
        };
        if let Some(failure) = running.stream.failure() {
            self.lose_stream(&failure);
            return;
        }
        let telemetry = running.stream.telemetry();
        let events = telemetry.take_events();
        let non_finite = events.contains(AudioEvent::NonFiniteOutput);
        let report = non_finite.then(|| telemetry.non_finite_report());
        let input_underruns = telemetry.input_underruns();
        let underrun = events.contains(AudioEvent::OutputUnderrun)
            || input_underruns != self.seen_input_underruns;
        self.seen_input_underruns = input_underruns;
        self.note_events(events);
        self.track_non_finite(report);
        self.track_failed_blocks();
        if underrun {
            self.grow_buffer_after_underrun();
        }
        // A deferred chain also replaces the running chain, so that it needs no repair.
        if non_finite
            && !self.chain_deferred
            && !self.repair_wanted
            && self
                .last_repair
                .is_none_or(|repair| repair.elapsed() >= MIN_REPAIR_INTERVAL)
        {
            // The callback only repairs the output. A new chain clears all other engine state.
            // This call holds the engine lock on the UI thread, so a compile must not run here.
            self.last_repair = Some(Instant::now());
            self.repair_wanted = true;
        }
    }

    /// The estimated delay of the running stream in milliseconds, from what its callback
    /// publishes.
    fn delay_estimate(&self) -> Option<f64> {
        let running = self.running.as_ref()?;
        let telemetry = running.stream.telemetry();
        // A host that chose its own buffer gives its real callback size only in the callback.
        let callback = match telemetry.callback_frames() {
            0 => running.negotiated.max_callback_frames as usize,
            frames => frames,
        };
        Some(round_trip_ms(DelayParts {
            sample_rate: running.negotiated.sample_rate,
            input_frames: if running.negotiated.input_device.is_some() {
                callback
            } else {
                0
            },
            output_frames: callback,
            // On Linux, the system route plays through PulseAudio unless it runs on ALSA. CoreAudio
            // and WASAPI play the buffer directly.
            server_playback: cfg!(target_os = "linux")
                && self.kind == AudioBackendKind::System
                && self.inventory.route.active.is_none(),
            queue_frames: telemetry.queue_frames(),
            chain_frames: telemetry.chain_latency(),
        }))
    }

    /// Moves the latest repair back, so that the rate limit lets the next one through.
    #[cfg(test)]
    pub(crate) fn age_repair_for_test(&mut self) {
        self.last_repair = self.last_repair.map(|repair| repair - MIN_REPAIR_INTERVAL);
    }

    /// Keeps the non-finite fault equal to what the callback reports.
    fn track_non_finite(&mut self, report: Option<NonFiniteReport>) {
        let Some(report) = report else {
            if self
                .non_finite
                .as_ref()
                .is_some_and(|(_, seen)| seen.elapsed() >= NON_FINITE_HOLD)
            {
                self.non_finite = None;
            }
            return;
        };
        if report.generation < self.structure_floor {
            return;
        }
        let named = report
            .block
            .and_then(|(side, position)| {
                let running = self.running.as_ref()?;
                running.mirror.fingerprint.block(side, position)
            })
            .map(|block| ChainFault::for_block(ChainFaultKind::NonFiniteOutput, block));
        // An event with no block must not lose the name from an earlier event.
        let fault = named
            .or_else(|| self.non_finite.take().map(|(fault, _)| fault))
            .unwrap_or_else(|| ChainFault::for_chain(ChainFaultKind::NonFiniteOutput));
        self.non_finite = Some((fault, Instant::now()));
    }

    /// Keeps the failed-block faults equal to the chain that the callback runs.
    ///
    /// A failed block stays bypassed for the life of its chain, so that time does not end this
    /// fault. The fault ends when the callback takes a new chain, because all its blocks are new.
    fn track_failed_blocks(&mut self) {
        self.failed_blocks.clear();
        let Some(running) = &self.running else {
            return;
        };
        let telemetry = running.stream.telemetry();
        let report = telemetry.failed_blocks();
        // This read comes after the report, so that the report cannot be newer than this chain.
        let active = telemetry.active_generation();
        // Under the floor, the positions of the running chain do not match the mirror.
        if report.generation != active || active < self.structure_floor {
            return;
        }
        let fingerprint = &running.mirror.fingerprint;
        self.failed_blocks.extend(
            report
                .blocks
                .positions()
                .filter_map(|(side, position)| fingerprint.block(side, position))
                .filter(|block| !block.is_insert)
                .map(|block| ChainFault::for_block(ChainFaultKind::ProcessingFailed, block)),
        );
    }

    /// Ends the non-finite fault because a chain for a new structure replaces the chain with it.
    fn forget_non_finite(&mut self) {
        self.non_finite = None;
        if let Some(change) = &self.outbox.chain {
            self.structure_floor = change.generation;
            self.view_dirty = true;
        }
    }

    /// Applies one device action on this thread, with its device work.
    ///
    /// # Errors
    /// Returns a settings or stream error. The stream state stays visible in the device state.
    #[cfg(test)]
    pub(super) fn apply_device(&mut self, action: &DeviceAction) -> Result<(), UiError> {
        self.request_device(action)?;
        self.run_device_work_here()
    }

    /// Runs waiting device work on this thread, and returns the error of the latest stream.
    fn run_device_work_here(&mut self) -> Result<(), UiError> {
        let opening = self.open_wanted;
        while let Some(job) = self.next_job() {
            let outcome = job.run();
            self.finish_device_work(outcome);
        }
        if !opening {
            return Ok(());
        }
        match self.device.error.clone() {
            Some(error) => Err(UiError::Control(error)),
            None => Ok(()),
        }
    }

    /// Records one device action. Anything slow waits for a [`DeviceJob`].
    ///
    /// # Errors
    /// Returns a settings error of a MIDI choice. A stream error appears in the device state.
    pub(super) fn request_device(&mut self, action: &DeviceAction) -> Result<(), UiError> {
        match action {
            DeviceAction::Configure(requested) => {
                let config = self.requested_config(requested);
                if requested
                    .buffer_frames
                    .is_some_and(|frames| frames != self.device.banner.buffer_size)
                {
                    // A buffer that the user chose is the result for the device, also with
                    // OPTIMIZE on.
                    self.forced_buffer = requested.buffer_frames;
                } else if config.input_device != self.config.input_device
                    || config.output_device != self.config.output_device
                {
                    self.probe_wanted = true;
                }
                self.want_open(config, true);
            }
            DeviceAction::SelectBackend(name) => {
                let kind = AudioBackendKind::from_name(name);
                self.backend = Arc::new(Mutex::new((self.make_backend)(kind)));
                self.kind = kind;
                self.inventory = DeviceInventory::default();
                // The identities of one driver mean nothing to another driver.
                let config = DeviceConfig {
                    backend: kind,
                    buffer_frames: self.config.buffer_frames,
                    input_channels: self.config.input_channels,
                    output_channels: self.config.output_channels,
                    ..DeviceConfig::default()
                };
                self.probe_wanted = true;
                self.want_open(config, true);
            }
            DeviceAction::SetHearYourself(enabled) => {
                self.device.banner.hear_yourself = *enabled;
                if let Some(running) = &self.running {
                    running.stream.telemetry().set_hear_yourself(*enabled);
                }
                self.save_hear_wanted = true;
            }
            DeviceAction::SetInputChannels(channels) => self.select_input_channels(channels),
            DeviceAction::SetOutputPair(pair) => {
                let first = usize::try_from((*pair).max(0)).unwrap_or(0) * 2;
                self.config.output_first = first;
                self.device.active_output_pair = *pair;
                if let Some(running) = &mut self.running {
                    running.negotiated.config.output_first = first;
                    running.stream.telemetry().set_output_first(first);
                }
                self.save_wanted = true;
            }
            DeviceAction::SetMidiInputEnabled { id, enabled } => {
                self.settings
                    .set_midi_enabled(id, *enabled)
                    .map_err(|error| UiError::Control(error.to_string()))?;
                if self.kind == AudioBackendKind::Dummy
                    && !self.device.midi_inputs.iter().any(|input| input.id == *id)
                {
                    self.device.midi_inputs.push(MidiInputDevice {
                        id: id.clone(),
                        name: id.clone(),
                        enabled: *enabled,
                    });
                } else if let Some(input) = self
                    .device
                    .midi_inputs
                    .iter_mut()
                    .find(|input| input.id == *id)
                {
                    input.enabled = *enabled;
                }
            }
            DeviceAction::Restart => self.want_open(self.config.clone(), true),
            DeviceAction::PlayTestTone => {
                if let Some(running) = &self.running {
                    running.stream.telemetry().request_test_tone();
                }
            }
            DeviceAction::OpenControlPanel
            | DeviceAction::OpenBluetoothMidi
            | DeviceAction::CycleInputMode => {}
        }
        Ok(())
    }

    /// Selects the device channels. The same layout changes with no new stream and no dropout.
    fn select_input_channels(&mut self, channels: &[usize]) {
        let count = channels.len().clamp(1, 2);
        let first = channels.first().copied().unwrap_or(0);
        let second = channels.get(1).copied().unwrap_or(first);
        let mut config = self.config.clone();
        config.input_channels = count;
        config.input_route = [first, second];
        let same_layout = self
            .running
            .as_ref()
            .is_some_and(|running| running.negotiated.config.input_channels == count);
        if same_layout && let Some(running) = &mut self.running {
            running.negotiated.config.input_route = [first, second];
            running.stream.telemetry().set_input_route(first, second);
            self.config = config;
            self.save_wanted = true;
        } else {
            self.want_open(config, true);
        }
        for channel in &mut self.device.input_channels {
            channel.active = channels.contains(&channel.index);
        }
        self.device.banner.has_active_input = !channels.is_empty();
    }

    /// Asks for a new stream with this configuration.
    fn want_open(&mut self, config: DeviceConfig, save: bool) {
        self.config = config;
        self.open_wanted = true;
        self.list_wanted = true;
        self.save_wanted |= save;
        self.device.status = Some(OPENING_TEXT.to_owned());
    }

    /// Asks for a new look at the devices.
    pub(super) fn request_device_list(&mut self) {
        self.list_wanted = true;
    }

    /// Whether a job waits for a thread that calls [`Self::begin_device_work`].
    #[must_use]
    pub(super) const fn device_work_wanted(&self) -> bool {
        self.open_wanted
            || self.list_wanted
            || self.save_wanted
            || self.save_hear_wanted
            || self.save_buffers_wanted
            || !self.retired.is_empty()
    }

    /// Whether a job waits or runs.
    #[must_use]
    pub(super) const fn device_work_pending(&self) -> bool {
        self.device_work_wanted() || self.job_started.is_some()
    }

    /// Takes the worker role when work waits and no thread has it. The result names the worker.
    pub(super) fn claim_device_worker(&mut self) -> Option<u64> {
        if self.shutdown || self.worker_claimed || !self.device_work_wanted() {
            return None;
        }
        self.worker_claimed = true;
        Some(self.worker_id)
    }

    /// The next job of one worker. With no job, that worker gives up its role in this same lock,
    /// so that new work always finds a worker or starts one.
    pub(super) fn begin_device_work(&mut self, worker: u64) -> Option<DeviceJob> {
        if worker != self.worker_id {
            return None;
        }
        let job = if self.shutdown { None } else { self.next_job() };
        if job.is_none() {
            self.worker_claimed = false;
        }
        job
    }

    fn next_job(&mut self) -> Option<DeviceJob> {
        if !self.device_work_wanted() {
            return None;
        }
        let open = std::mem::take(&mut self.open_wanted);
        if open {
            self.stop_stream();
        }
        let buffer = if open && self.optimizes_buffer() {
            Some(BufferRequest {
                stored: self.optimized_buffers.clone(),
                probe: std::mem::take(&mut self.probe_wanted),
                forced: self.forced_buffer.take(),
            })
        } else {
            if open {
                self.probe_wanted = false;
                self.forced_buffer = None;
            }
            None
        };
        let save_buffers =
            std::mem::take(&mut self.save_buffers_wanted).then(|| self.optimized_buffers.clone());
        self.job_serial += 1;
        self.job_started = Some(Instant::now());
        Some(DeviceJob {
            serial: self.job_serial,
            config: self.config.clone(),
            open,
            list: std::mem::take(&mut self.list_wanted),
            save: std::mem::take(&mut self.save_wanted),
            save_hear: std::mem::take(&mut self.save_hear_wanted),
            retired: std::mem::take(&mut self.retired),
            backend: Arc::clone(&self.backend),
            session: self.session.clone(),
            settings: Arc::clone(&self.settings),
            list_midi: self.kind != AudioBackendKind::Dummy,
            hear_yourself: self.device.banner.hear_yourself,
            optimize: self.optimize,
            oversampling: self.wanted_oversampling(),
            buffer,
            save_buffers,
        })
    }

    /// Installs what one job found. An outcome of a job that the engine gave up on only closes
    /// its stream.
    pub(super) fn finish_device_work(&mut self, outcome: DeviceOutcome) {
        if outcome.serial != self.job_serial {
            if let Some(Ok(opened)) = outcome.opened {
                self.retired.push(opened.stream);
            }
            return;
        }
        self.job_started = None;
        let DeviceOutcome {
            inventory,
            opened,
            other_error,
            buffer,
            ..
        } = outcome;
        if let Some(mut inventory) = inventory {
            if opened.is_none() {
                inventory.input = self.inventory.input.take();
                inventory.output = self.inventory.output.take();
            }
            self.inventory = inventory;
        } else if opened.is_some() {
            self.inventory.resolved = None;
        }
        let opened_now = opened.is_some();
        let error = match opened {
            Some(Ok(opened)) => {
                // The mirror and the chain come from one plan, so they match. The sync after
                // this outcome sends each edit that came after that plan.
                self.install_stream(opened);
                self.record_buffer(buffer);
                other_error
            }
            Some(Err(error)) => Some(error.to_string()),
            None => other_error.or_else(|| self.device.error.clone()),
        };
        self.refresh_device(error.clone());
        if opened_now {
            self.note_stream(error);
        }
        if !self.open_wanted {
            self.device.status = None;
        }
    }

    fn install_stream(&mut self, opened: OpenedStream) {
        let OpenedStream {
            stream,
            negotiated,
            format,
            mirror,
            generation,
            unavailable,
            spectrum_taps,
        } = opened;
        self.unavailable = unavailable;
        self.spectrum_taps = spectrum_taps;
        self.chain_error = None;
        // A new stream has a new chain and new telemetry, so no old report can arrive.
        self.non_finite = None;
        self.repair_wanted = false;
        self.failed_blocks.clear();
        self.structure_floor = 0;
        self.view_dirty = true;
        self.published_generation = generation;
        self.last_negotiated = Some(negotiated.clone());
        // A choice made while the job ran reaches the new stream here.
        stream
            .telemetry()
            .set_hear_yourself(self.device.banner.hear_yourself);
        stream.telemetry().set_optimize(self.optimize);
        self.seen_input_underruns = stream.telemetry().input_underruns();
        self.running = Some(RunningStream {
            stream,
            negotiated,
            format,
            mirror,
        });
    }

    /// Stops waiting for a job that runs too long. A later outcome of that job closes its stream.
    fn give_up_on_a_stuck_job(&mut self) {
        let Some(started) = self.job_started else {
            return;
        };
        if started.elapsed() < self.work_limit {
            return;
        }
        self.job_started = None;
        self.job_serial += 1;
        self.worker_id += 1;
        self.worker_claimed = false;
        // The stuck thread holds the old backend.
        self.backend = Arc::new(Mutex::new((self.make_backend)(self.kind)));
        self.open_wanted = false;
        let message = "The audio system does not answer.".to_owned();
        self.note(&format!("stream did not open: {message}"));
        self.refresh_device(Some(message));
        self.device.status = None;
    }

    /// Changes how long device work can run, for a test of a stuck job.
    pub(super) fn set_work_limit(&mut self, limit: Duration) {
        self.work_limit = limit;
    }

    /// Ends a worker that stopped with no outcome.
    pub(super) fn device_worker_stopped(&mut self, worker: u64) {
        if worker != self.worker_id {
            return;
        }
        self.worker_claimed = false;
        self.job_started = None;
        self.device.status = None;
        self.device.error = Some("The audio work stopped unexpectedly.".to_owned());
    }

    /// Whether the engine stops.
    pub(super) const fn shutdown_started(&self) -> bool {
        self.shutdown
    }

    /// When the running device job reaches the time limit, or `None` with no running job.
    pub(super) fn device_job_deadline(&self) -> Option<Instant> {
        self.job_started.map(|started| started + self.work_limit)
    }

    /// Ends the chain worker role. Each later new structure compiles on the calling thread, so
    /// that an edit still reaches the stream.
    pub(super) fn chain_worker_stopped(&mut self) {
        self.chain_worker = false;
        let wanted = self.compiling || self.chain_work_wanted();
        self.abandon_deferred_chain();
        if wanted && !self.shutdown {
            self.note("the chain work stopped unexpectedly");
            self.chain_error = Some("The chain work stopped unexpectedly.".to_owned());
        }
    }

    /// Stops all work. Returns the streams to close, and the id of the device worker when no
    /// device job runs now. Only a thread with that id ends at once, because a thread that the
    /// engine gave up on can still wait for the sound server.
    ///
    /// The outcome of a job that still runs counts as the outcome of a job that the engine gave
    /// up on, so it installs nothing.
    pub(super) fn shut_down(&mut self) -> (Vec<AudioStream>, Option<u64>) {
        self.shutdown = true;
        self.abandon_deferred_chain();
        let idle = self.job_started.take().is_none();
        self.job_serial += 1;
        self.stop_stream();
        self.publish_view();
        (
            std::mem::take(&mut self.retired),
            idle.then_some(self.worker_id),
        )
    }

    /// Renders explicit input through the running dummy stream.
    ///
    /// # Errors
    /// Returns a stream-layout or processing error, or an error without a dummy stream.
    pub(super) fn render_dummy(&mut self, input: &[f32]) -> Result<Vec<f32>, UiError> {
        self.pump();
        let running = self
            .running
            .as_mut()
            .ok_or_else(|| UiError::Control("dummy audio stream is unavailable".to_owned()))?;
        let output = running
            .stream
            .render_with_input(input)
            .map(Vec::from)
            .map_err(|error| UiError::Control(error.to_string()));
        self.poll();
        output
    }

    /// Renders all channels of the device through the running dummy stream, as a host gives them.
    ///
    /// # Errors
    /// Returns a stream-layout or processing error, or an error without a dummy stream.
    pub(super) fn render_dummy_capture(&mut self, capture: &[f32]) -> Result<Vec<f32>, UiError> {
        self.pump();
        let running = self
            .running
            .as_mut()
            .ok_or_else(|| UiError::Control("dummy audio stream is unavailable".to_owned()))?;
        let output = running
            .stream
            .render_capture(capture)
            .map(Vec::from)
            .map_err(|error| UiError::Control(error.to_string()));
        self.poll();
        output
    }

    /// Sends everything the queue can take, in order, after it destroys retired work here.
    fn pump(&mut self) {
        self.publish_view();
        let Some(running) = &self.running else {
            return;
        };
        let commands = running.stream.commands();
        self.retired_chains += commands.drain_retired() as u64;
        if let Some(change) = self.outbox.chain.take() {
            let generation = change.generation;
            match commands.try_send(ChainCommand::Replace(change)) {
                Ok(()) => self.published_generation = generation,
                Err(TrySendError::Full(command) | TrySendError::Disconnected(command)) => {
                    if let ChainCommand::Replace(change) = command {
                        self.outbox.chain = Some(change);
                    }
                }
            }
        }
        // New settings must follow a chain that waits, or the callback would lose them.
        if self.outbox.chain.is_none()
            && let Some(params) = self.outbox.block_params.take()
            && let Err(TrySendError::Full(command) | TrySendError::Disconnected(command)) =
                commands.try_send(ChainCommand::UpdateBlockParams(params))
            && let ChainCommand::UpdateBlockParams(params) = command
        {
            self.outbox.block_params = Some(params);
        }
        if let Some(controls) = self.outbox.controls.take()
            && let Err(TrySendError::Full(controls) | TrySendError::Disconnected(controls)) =
                commands.try_send_controls(controls)
        {
            self.outbox.controls = Some(controls);
        }
    }

    /// Queues a chain for the session on this thread. Returns whether the preparation succeeded.
    fn queue_session_chain(&mut self) -> bool {
        let Some(format) = self.running.as_ref().map(|running| running.format) else {
            return false;
        };
        let prepared = self
            .session
            .chain_plan(format)
            .and_then(|plan| (self.prepare)(&plan));
        match prepared {
            Ok(chain) => {
                self.unavailable = chain.unavailable;
                self.chain_error = None;
                self.spectrum_taps = chain.change.spectrum_taps();
                self.view_dirty = true;
                self.outbox.chain = Some(chain.change);
                self.outbox.block_params = None;
                true
            }
            Err(error) => {
                self.note(&format!("chain preparation failed: {error}"));
                self.chain_error = Some(error.to_string());
                false
            }
        }
    }

    /// Hands the running stream to the next job, which closes it with no engine lock.
    fn stop_stream(&mut self) {
        self.outbox = Outbox::default();
        // A running compile planned for this stream. The next stream has a newer chain of its
        // own, so the finish of that compile does not queue the older chain.
        if let Some(running) = self.running.take() {
            self.retired.push(running.stream);
            self.view_dirty = true;
        }
    }

    /// Hands a failed stream to the next job and shows the failure. Recovery stays a user action:
    /// the banner opens the settings, where a device, buffer, or rate choice reopens the stream.
    fn lose_stream(&mut self, failure: &AudioError) {
        let capture = self
            .running
            .as_ref()
            .is_some_and(|running| running.stream.telemetry().capture_failed());
        let label = if capture {
            self.device.banner.input_device.clone()
        } else {
            self.device.banner.output_device.clone()
        };
        let message = match failure {
            AudioError::Disconnected if !label.is_empty() => {
                AudioError::DeviceGone(label).to_string()
            }
            other => other.to_string(),
        };
        self.note(&format!("stream lost: {message}"));
        self.stop_stream();
        self.list_wanted = true;
        self.refresh_device(Some(message));
    }

    /// Builds the device state from what the last job found. This touches no device.
    fn refresh_device(&mut self, error: Option<String>) {
        let open = self.running.is_some();
        let effective = self.effective_config();
        let channels = self.running.as_ref().map(|running| {
            (
                running.negotiated.capture_channels,
                running.negotiated.playback_channels,
            )
        });
        let status = self.device.status.take();
        self.device = snapshot(SnapshotInput {
            kind: self.kind,
            effective: &effective,
            inventory: &self.inventory,
            previous: Some(&self.device),
            open,
            channels,
        });
        self.device.error = error;
        self.device.status = status;
        // A real name lets the banner rules tell a lost default device from an empty selection.
        let banner = &mut self.device.banner;
        if let Some(running) = &self.running {
            if banner.input_device.is_empty() {
                banner.input_device = running.negotiated.input_device.clone().unwrap_or_default();
            }
            if banner.output_device.is_empty() {
                banner.output_device = running.negotiated.output_device.clone().unwrap_or_default();
            }
            self.last_labels = Some((banner.input_device.clone(), banner.output_device.clone()));
        } else if let Some((input, output)) = &self.last_labels {
            banner.input_device.clone_from(input);
            banner.output_device.clone_from(output);
        } else if banner.output_device.is_empty() {
            banner.output_device = DEFAULT_DEVICE_LABEL.to_owned();
        }
    }

    /// The running layout, or the request with the last real values for what it leaves open.
    /// The settings send these values back on a retry, so they must be values the device took.
    fn effective_config(&self) -> DeviceConfig {
        if let Some(running) = &self.running {
            return running.negotiated.worker_config();
        }
        let last = self.last_negotiated.as_ref();
        DeviceConfig {
            sample_rate: self
                .config
                .sample_rate
                .or_else(|| last.map(|stream| stream.sample_rate)),
            buffer_frames: self
                .config
                .buffer_frames
                .or_else(|| last.map(|stream| stream.max_callback_frames)),
            ..self.config.clone()
        }
    }

    /// Maps a settings request to a stream request. The settings send back the values that they
    /// show. A shown value means "unchanged", so that an automatic choice stays automatic.
    fn requested_config(&self, requested: &AudioDeviceConfig) -> DeviceConfig {
        let keep = |asked: &Option<String>, current: &Option<String>, shown: &str| {
            if asked.as_deref() == Some(shown) {
                current.clone()
            } else {
                asked.clone()
            }
        };
        let input_device = keep(
            &requested.input,
            &self.config.input_device,
            &self.device.selected_input,
        );
        let output_device = keep(
            &requested.output,
            &self.config.output_device,
            &self.device.selected_output,
        )
        // The automatic output is the system default, so that these are one choice.
        .filter(|id| id != SYSTEM_DEFAULT_DEVICE);
        let label = |id: &Option<String>, current: &Option<String>, old: &Option<String>| {
            if id == current {
                return old.clone();
            }
            let id = id.as_deref()?;
            self.inventory
                .devices
                .iter()
                .find(|device| device.id == id)
                .map(|device| device.label.clone())
        };
        let shown_rate = self.device.banner.sample_rate;
        let shown_buffer = self.device.banner.buffer_size;
        let input_changed = input_device != self.config.input_device;
        let output_changed = output_device != self.config.output_device;
        DeviceConfig {
            backend: self.kind,
            input_label: label(
                &input_device,
                &self.config.input_device,
                &self.config.input_label,
            ),
            output_label: label(
                &output_device,
                &self.config.output_device,
                &self.config.output_label,
            ),
            input_device,
            output_device,
            sample_rate: match requested.sample_rate {
                Some(rate) if (rate - shown_rate).abs() < 0.5 => self.config.sample_rate,
                other => other.map(|rate| rate as u32),
            },
            buffer_frames: match requested.buffer_frames {
                Some(frames) if frames == shown_buffer => self.config.buffer_frames,
                other => other,
            },
            input_channels: self.config.input_channels,
            output_channels: self.config.output_channels,
            // A channel of the old device means nothing on the new device.
            input_route: if input_changed {
                [0, 1]
            } else {
                self.config.input_route
            },
            output_first: if output_changed {
                0
            } else {
                self.config.output_first
            },
        }
    }
}

/// Everything that device work finds out. Only [`EngineState::finish_device_work`] installs it.
#[derive(Default)]
struct DeviceInventory {
    devices: Vec<DeviceDescriptor>,
    route: Route,
    resolved: Option<ResolvedRoute>,
    input: Option<DeviceCapabilities>,
    output: Option<DeviceCapabilities>,
    midi: Option<Vec<MidiInputDevice>>,
}

/// A started stream, with what the engine must know about it.
struct OpenedStream {
    stream: AudioStream,
    negotiated: NegotiatedStream,
    format: StreamFormat,
    mirror: AudioSessionState,
    generation: u64,
    unavailable: Vec<(BlockId, ChainFault)>,
    spectrum_taps: Vec<(BlockId, Arc<SpectrumTap>)>,
}

/// One piece of device work. It holds no engine lock, so that a frame does not wait for it.
pub(super) struct DeviceJob {
    serial: u64,
    config: DeviceConfig,
    open: bool,
    list: bool,
    save: bool,
    /// Whether to save `hear_yourself`. The newest choice goes in the next job, so the jobs
    /// save the choices in their order.
    save_hear: bool,
    retired: Vec<AudioStream>,
    backend: SharedBackend,
    session: ProcessorBackend,
    settings: Arc<StandaloneAudioSettings>,
    list_midi: bool,
    /// Whether the new stream plays the processed input from its first callback.
    hear_yourself: bool,
    /// Whether OPTIMIZE is on from the first callback of the new stream.
    optimize: bool,
    /// The oversampling of the chain of the new stream.
    oversampling: u8,
    /// How OPTIMIZE chooses the buffer of the new stream, or `None` for the buffer of the user.
    buffer: Option<BufferRequest>,
    /// The buffers that OPTIMIZE found, to save.
    save_buffers: Option<BTreeMap<String, u32>>,
}

/// What a job needs to choose the buffer for OPTIMIZE, once it knows the devices.
struct BufferRequest {
    stored: BTreeMap<String, u32>,
    probe: bool,
    /// A step up or a choice of the user, which wins over a stored result and a probe.
    forced: Option<u32>,
}

impl BufferRequest {
    fn choose(
        &self,
        kind: AudioBackendKind,
        route: &ResolvedRoute,
        input: Option<&DeviceCapabilities>,
        output: Option<&DeviceCapabilities>,
    ) -> BufferChoice {
        let device = buffer_device(kind, route);
        let range = common_buffer_range(input, output);
        let frames = self.forced.unwrap_or_else(|| {
            chosen_frames(
                self.stored.get(&device).copied(),
                self.probe,
                range.as_ref(),
            )
        });
        BufferChoice {
            device,
            frames,
            range,
        }
    }
}

/// The identity of a device pair for a stored buffer. A default device has no identity of its
/// own, so that it is "default".
fn buffer_device(kind: AudioBackendKind, route: &ResolvedRoute) -> String {
    let id = |device: &ResolvedDevice| device.open.clone().unwrap_or_else(|| "default".to_owned());
    format!("{}|{}|{}", kind.name(), id(&route.input), id(&route.output))
}

/// The buffer sizes that both devices take, when at least one says.
fn common_buffer_range(
    input: Option<&DeviceCapabilities>,
    output: Option<&DeviceCapabilities>,
) -> Option<RangeInclusive<u32>> {
    let ranges = [input, output]
        .into_iter()
        .flatten()
        .filter_map(|capabilities| capabilities.buffer_frames.clone());
    let (start, end) = ranges.fold(None, |both: Option<(u32, u32)>, range| {
        Some(both.map_or((*range.start(), *range.end()), |(start, end)| {
            (start.max(*range.start()), end.min(*range.end()))
        }))
    })?;
    (start <= end).then_some(start..=end)
}

/// The result of one [`DeviceJob`].
pub(super) struct DeviceOutcome {
    serial: u64,
    inventory: Option<DeviceInventory>,
    opened: Option<Result<OpenedStream, AudioError>>,
    other_error: Option<String>,
    /// The buffer that OPTIMIZE chose for the stream of this job.
    buffer: Option<BufferChoice>,
}

impl DeviceJob {
    /// Does the slow work. This closes streams, talks to the sound server, opens devices, and
    /// prepares a chain.
    #[must_use]
    pub(super) fn run(self) -> DeviceOutcome {
        let mut other_error = None;
        for stream in self.retired {
            if let Err(error) = stream.close() {
                other_error.get_or_insert(error.to_string());
            }
        }
        if self.save
            && let Err(error) = self.settings.save_config(&self.config)
        {
            other_error.get_or_insert(error.to_string());
        }
        if self.save_hear
            && let Err(error) = self.settings.set_hear_yourself(self.hear_yourself)
        {
            other_error.get_or_insert(error.to_string());
        }
        if let Some(buffers) = &self.save_buffers
            && let Err(error) = self.settings.save_optimized_buffers(buffers)
        {
            other_error.get_or_insert(error.to_string());
        }
        let mut outcome = DeviceOutcome {
            serial: self.serial,
            inventory: None,
            opened: None,
            other_error,
            buffer: None,
        };
        if !self.list && !self.open {
            return outcome;
        }
        let mut backend = self
            .backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let route = backend.connect();
        let mut open_config = self.config;
        // A host that cannot list devices cannot open one, and the open below reports why.
        if let Ok(devices) = backend.enumerate() {
            let resolved = resolve_route(&open_config, &devices);
            open_config.input_device.clone_from(&resolved.input.open);
            open_config.output_device.clone_from(&resolved.output.open);
            let midi = self.list_midi.then(|| midi_rows(&self.settings)).flatten();
            let input = backend
                .capabilities(resolved.input.open.as_deref(), true)
                .ok();
            let output = backend
                .capabilities(resolved.output.open.as_deref(), false)
                .ok();
            if self.open
                && let Some(request) = &self.buffer
            {
                let choice = request.choose(
                    open_config.backend,
                    &resolved,
                    input.as_ref(),
                    output.as_ref(),
                );
                open_config.buffer_frames = Some(choice.frames);
                outcome.buffer = Some(choice);
            }
            outcome.inventory = Some(DeviceInventory {
                input,
                output,
                resolved: Some(resolved),
                devices,
                route,
                midi,
            });
        }
        if self.open {
            outcome.opened = Some(start_stream(
                &mut **backend,
                &self.session,
                &open_config,
                self.hear_yourself,
                self.optimize,
                self.oversampling,
            ));
        }
        outcome
    }
}

fn midi_rows(settings: &StandaloneAudioSettings) -> Option<Vec<MidiInputDevice>> {
    let ports = MidiInputManager::ports().ok()?;
    Some(
        ports
            .into_iter()
            .map(|port| MidiInputDevice {
                enabled: settings.midi_enabled(&port.id),
                id: port.id,
                name: port.name,
            })
            .collect(),
    )
}

/// Negotiates, prepares the chain for the final layout, and starts one stream.
fn start_stream(
    backend: &mut (dyn AudioBackend + Send),
    session: &ProcessorBackend,
    config: &DeviceConfig,
    hear_yourself: bool,
    optimize: bool,
    oversampling: u8,
) -> Result<OpenedStream, AudioError> {
    let negotiated = backend.negotiate(config)?;
    let worker_config = negotiated.worker_config();
    let format = StreamFormat {
        sample_rate: f64::from(negotiated.sample_rate),
        max_frames: negotiated.max_callback_frames as usize,
        stereo_input: worker_config.input_channels == 2,
        stereo_output: worker_config.output_channels == 2,
        oversampling,
    };
    let control_error = |error: UiError| AudioError::Backend(error.to_string());
    // The new stream gets a new chain of the session, so it answers each waiting restore.
    session.take_restore().map_err(control_error)?;
    // The mirror and the chain come from one plan, so an edit and its undo during the compile
    // cannot put a chain of the edit behind the mirror.
    let plan = session.chain_plan(format).map_err(control_error)?;
    let chain = plan.prepare().map_err(control_error)?;
    let mirror = plan.into_state();
    let generation = chain.change.generation;
    let unavailable = chain.unavailable;
    let spectrum_taps = chain.change.spectrum_taps();
    let worker = AudioWorker::with_chain(&worker_config, chain.change, mirror.controls)?;
    worker.telemetry().set_hear_yourself(hear_yourself);
    worker.telemetry().set_optimize(optimize);
    let mut stream = backend.open(&negotiated.config, worker)?;
    stream.start()?;
    Ok(OpenedStream {
        stream,
        negotiated,
        format,
        mirror,
        generation,
        unavailable,
        spectrum_taps,
    })
}

fn spec_matches(change: &PreparedChainChange, format: StreamFormat) -> bool {
    change.spec.host_sample_rate == format.sample_rate
        && change.spec.max_host_frames == format.max_frames
        && change.spec.stereo_input == format.stereo_input
        && change.spec.stereo_output == format.stereo_output
        && change.spec.oversampling == format.oversampling
}

/// The oversampling of the chain. OPTIMIZE turns it off, because it costs processing time and
/// its filters add delay. Without OPTIMIZE, the oversampling setting chooses.
fn stream_oversampling(
    optimize: bool,
    enabled: Option<ParameterValue>,
    factor: Option<ParameterValue>,
) -> u8 {
    if optimize || !enabled.is_some_and(|enabled| enabled.0 >= 0.5) {
        return 1;
    }
    let choice = factor.map_or(0.0, |factor| factor.0.round()).max(0.0) as usize;
    OVERSAMPLING_FACTORS[choice.min(OVERSAMPLING_FACTORS.len() - 1)]
}

/// The backend of one driver. A system route finds its server later, on the worker.
#[must_use]
pub fn new_backend(kind: AudioBackendKind) -> Box<dyn AudioBackend + Send> {
    match kind {
        AudioBackendKind::Dummy => Box::new(DummyBackend::connected()),
        AudioBackendKind::System | AudioBackendKind::Alsa | AudioBackendKind::Jack => {
            Box::new(CpalBackend::new(kind))
        }
    }
}

/// The rows of one list: the system default first, then each real endpoint.
fn device_rows(devices: &[DeviceDescriptor], input: bool) -> Vec<AudioDeviceOption> {
    let default = system_default_label(devices, input).map(|label| AudioDeviceOption {
        id: SYSTEM_DEFAULT_DEVICE.to_owned(),
        label: format!("System default: {label}"),
    });
    default
        .into_iter()
        .chain(
            devices
                .iter()
                .filter(|device| device.listed && if input { device.input } else { device.output })
                .map(|device| AudioDeviceOption {
                    id: device.id.clone(),
                    label: device.label.clone(),
                }),
        )
        .collect()
}

/// The buffer rows that the device gives, with the running size.
fn buffer_rows(inventory: &DeviceInventory, current: u32) -> Vec<u32> {
    let accepts = |frames: u32, capabilities: &Option<DeviceCapabilities>| {
        capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.buffer_frames.as_ref())
            .is_none_or(|range| range.contains(&frames))
    };
    let mut rows: Vec<u32> = COMMON_BUFFER_FRAMES
        .into_iter()
        .filter(|frames| accepts(*frames, &inventory.input) && accepts(*frames, &inventory.output))
        .collect();
    if !rows.contains(&current) {
        rows.push(current);
        rows.sort_unstable();
    }
    rows
}

/// The rates that both devices accept, with the running rate.
fn rate_rows(inventory: &DeviceInventory, current: u32) -> Vec<f64> {
    let accepts = |rate: u32, capabilities: &Option<DeviceCapabilities>| {
        capabilities
            .as_ref()
            .is_none_or(|capabilities| capabilities.sample_rates.contains(&rate))
    };
    let mut rows: Vec<u32> = COMMON_SAMPLE_RATES
        .into_iter()
        .filter(|rate| accepts(*rate, &inventory.input) && accepts(*rate, &inventory.output))
        .collect();
    if !rows.contains(&current) {
        rows.push(current);
        rows.sort_unstable();
    }
    rows.into_iter().map(f64::from).collect()
}

/// The output pairs of a device with more than two channels.
fn output_pair_rows(channels: usize) -> Vec<String> {
    if channels <= 2 {
        return Vec::new();
    }
    (0..channels / 2)
        .map(|pair| format!("Output {}-{}", pair * 2 + 1, pair * 2 + 2))
        .collect()
}

/// What the page says about a rate that the sound server must convert.
fn rate_note(active: AudioBackendKind, inventory: &DeviceInventory, rate: u32) -> Option<String> {
    if active != AudioBackendKind::System {
        return None;
    }
    let device_rate = inventory.output.as_ref()?.device_rate?;
    (device_rate != rate).then(|| {
        format!(
            "The sound server runs at {device_rate} Hz and converts this rate, which adds a small delay."
        )
    })
}

/// Everything that one snapshot of the device state reads.
struct SnapshotInput<'a> {
    kind: AudioBackendKind,
    /// The running layout, or the request with the last real values.
    effective: &'a DeviceConfig,
    inventory: &'a DeviceInventory,
    previous: Option<&'a SystemDeviceState>,
    open: bool,
    /// The channels of the running devices, when a stream runs.
    channels: Option<(usize, usize)>,
}

fn snapshot(input: SnapshotInput<'_>) -> SystemDeviceState {
    let SnapshotInput {
        kind,
        effective,
        inventory,
        previous,
        open,
        channels,
    } = input;
    let capture_channels = channels
        .map(|(capture, _)| capture)
        .or_else(|| inventory.input.as_ref().map(|input| input.channels))
        .unwrap_or_else(|| {
            if kind == AudioBackendKind::Dummy {
                effective.input_channels.max(2)
            } else {
                effective.input_channels.max(1)
            }
        });
    let playback_channels = channels
        .map(|(_, playback)| playback)
        .or_else(|| inventory.output.as_ref().map(|output| output.channels))
        .unwrap_or(2);
    let mut fitted = effective.clone();
    fit_layout(&mut fitted, capture_channels, playback_channels);
    let active_channels: &[usize] = if fitted.input_channels == 2 {
        &fitted.input_route
    } else {
        &fitted.input_route[..1]
    };
    let input_channels: Vec<AudioInputChannel> = (0..capture_channels)
        .map(|index| AudioInputChannel {
            index,
            name: format!("Input {}", index + 1),
            active: active_channels.contains(&index),
        })
        .collect();
    let midi_inputs = inventory
        .midi
        .clone()
        .or_else(|| previous.map(|device| device.midi_inputs.clone()))
        .unwrap_or_default();
    let hear_yourself = previous.is_none_or(|device| device.banner.hear_yourself);
    let has_active_input = input_channels.iter().any(|channel| channel.active);
    let active = inventory.route.active.unwrap_or(kind);
    let resolved = inventory.resolved.as_ref();
    let label = |device: fn(&ResolvedRoute) -> &ResolvedDevice| {
        resolved.map(|route| device(route).label.clone())
    };
    let row = |device: fn(&ResolvedRoute) -> &ResolvedDevice,
               keep: fn(&SystemDeviceState) -> &str| {
        resolved.map_or_else(
            || previous.map_or_else(String::new, |state| keep(state).to_owned()),
            |route| device(route).row.clone(),
        )
    };
    let sample_rate = effective.sample_rate.unwrap_or(48_000);
    let buffer_size = effective.buffer_frames.unwrap_or(DEFAULT_BUFFER_FRAMES);
    let notice = [
        inventory.route.notice.clone(),
        resolved.and_then(|route| route.input.notice.clone()),
        resolved.and_then(|route| route.output.notice.clone()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let output_pairs = output_pair_rows(playback_channels);
    SystemDeviceState {
        banner: AudioDeviceState {
            mic_denied: false,
            device_open: open,
            input_device: label(|route| &route.input).unwrap_or_default(),
            output_device: label(|route| &route.output).unwrap_or_default(),
            has_active_input,
            hear_yourself,
            feedback_risk: false,
            asio_available: false,
            current_type: active.name().to_owned(),
            buffer_size,
            buffer_sizes: buffer_rows(inventory, buffer_size),
            sample_rate: f64::from(sample_rate),
            // The next poll reads the running stream.
            delay_ms: None,
        },
        device_types: if kind == AudioBackendKind::Dummy {
            vec![AudioBackendKind::Dummy.name().to_owned()]
        } else {
            AudioBackendKind::real_drivers()
                .iter()
                .map(|driver| driver.name().to_owned())
                .collect()
        },
        input_devices: device_rows(&inventory.devices, true),
        output_devices: device_rows(&inventory.devices, false),
        selected_input: row(|route| &route.input, |state| &state.selected_input),
        selected_output: row(|route| &route.output, |state| &state.selected_output),
        input_channels,
        midi_inputs,
        sample_rates: rate_rows(inventory, sample_rate),
        active_output_pair: if output_pairs.is_empty() {
            previous.map_or(-1, |device| device.active_output_pair)
        } else {
            i32::try_from(fitted.output_first / 2).unwrap_or(0)
        },
        output_pairs,
        separate_io: true,
        has_control_panel: false,
        bt_midi_available: false,
        error: None,
        status: None,
        notice: (!notice.is_empty()).then(|| notice.join(" ")),
        rate_note: rate_note(active, inventory, sample_rate),
    }
}

#[cfg(test)]
mod tests {
    use ronie_core::InputMode;
    use ronie_dsp::{PrepareSpec, rt_chain::FailedBlocks};
    use ronie_plugin::RonieProcessor;

    use ronie_ui::backend::{UiAction, UiBackend};

    use super::*;

    /// A state on a dummy stream, with no chain worker, so that each new structure compiles on
    /// this thread.
    fn start(
        session: ProcessorBackend,
        settings: StandaloneAudioSettings,
    ) -> Result<EngineState, UiError> {
        EngineState::started(
            session,
            settings,
            Arc::new(new_backend),
            AudioBackendKind::Dummy,
            None,
        )
    }

    fn session() -> ProcessorBackend {
        ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: DEFAULT_BUFFER_FRAMES as usize,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        }))
    }

    fn ir_tone() -> ronie_core::Tone {
        ronie_core::Tone::from_value(serde_json::json!({
            "id": 6,
            "title": "Remote",
            "format": "ir",
            "models": [{"id": 8, "name": "Cab", "model_url": "https://models.example/8.wav"}],
        }))
        .expect("tone")
    }

    fn ir_bytes() -> ronie_core::ModelBytes {
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

    fn engine_with_deferred_load(session: &ProcessorBackend) -> (EngineState, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("tempdir");
        let settings = StandaloneAudioSettings::load(directory.path().join("audio.settings"))
            .expect("settings");
        let mut engine = start(session.clone(), settings).expect("engine");
        let target = session.chain_state(0).chain.left[0].id.clone();
        session
            .dispatch(UiAction::LoadToneWithBytes {
                target_insert_id: target,
                tone: ir_tone(),
                model_id: ronie_core::ModelId(8),
                bytes: ir_bytes(),
            })
            .expect("load");
        // A chain worker takes the new structure, and this test plays that worker.
        engine.chain_worker = true;
        engine.sync().expect("sync");
        (engine, directory)
    }

    #[test]
    fn a_block_edit_during_a_deferred_preparation_reaches_the_new_chain() {
        let session = session();
        let (mut engine, _directory) = engine_with_deferred_load(&session);
        let first = engine.status().published_generation;
        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("a new structure needs a chain");
        let prepared = deferred.plan().prepare();

        let block = session.chain_state(0).chain.left[0].clone();
        let mut edited = block.params.clone();
        edited.mix = 0.25;
        session
            .dispatch(UiAction::SetBlockParams {
                block_id: block.id,
                params: edited,
            })
            .expect("edit");
        engine.sync().expect("sync");
        assert_eq!(engine.status().published_generation, first, "no compile");

        assert!(
            engine
                .finish_deferred_chain(deferred, prepared)
                .expect("finish")
        );

        assert!(engine.status().published_generation > first);
        let running = engine.running.as_ref().expect("stream");
        let state = session.audio_state().expect("state");
        assert_eq!(running.mirror.block_params, state.block_params);
        assert_eq!(running.mirror.fingerprint, state.fingerprint);
        assert!(
            engine.outbox.block_params.is_none(),
            "the update left for the new chain"
        );
        assert!(engine.status().unavailable_blocks.is_empty());
    }

    #[test]
    fn a_cab_blend_edit_reaches_the_running_chain_with_no_new_chain() {
        let session = session();
        let (mut engine, _directory) = plain_engine_of(&session);
        let target = session.chain_state(0).chain.left[0].id.clone();
        session
            .dispatch(UiAction::LoadToneWithBytes {
                target_insert_id: target,
                tone: ir_tone(),
                model_id: ronie_core::ModelId(8),
                bytes: ir_bytes(),
            })
            .expect("load");
        engine.sync().expect("sync");
        let first = engine.status().published_generation;

        let block = session.chain_state(0).chain.left[0].clone();
        let mut edited = block.params.clone();
        edited.ir_blend = 0.8;
        edited.ir_a_level = 0.4;
        edited.ir_b_level = 0.6;
        edited.ir_b_invert = true;
        session
            .dispatch(UiAction::SetBlockParams {
                block_id: block.id,
                params: edited,
            })
            .expect("edit");
        engine.sync().expect("sync");

        assert_eq!(engine.status().published_generation, first, "no new chain");
        let running = engine.running.as_ref().expect("stream");
        let state = session.audio_state().expect("state");
        assert_eq!(running.mirror.fingerprint, state.fingerprint);
        assert_eq!(running.mirror.block_params, state.block_params);
        assert!(engine.outbox.chain.is_none());
    }

    #[test]
    fn a_skin_change_needs_no_new_chain() {
        let session = session();
        let (mut engine, _directory) = plain_engine_of(&session);
        let target = session.chain_state(0).chain.left[0].id.clone();
        session
            .dispatch(UiAction::LoadToneWithBytes {
                target_insert_id: target.clone(),
                tone: ir_tone(),
                model_id: ronie_core::ModelId(8),
                bytes: ir_bytes(),
            })
            .expect("load");
        engine.sync().expect("sync");
        let first = engine.status().published_generation;

        session
            .dispatch(UiAction::SetBlockSkin {
                block_id: target,
                skin: Some("cab-112-clean".to_owned()),
            })
            .expect("skin");
        engine.sync().expect("sync");

        assert_eq!(engine.status().published_generation, first, "no new chain");
        assert!(engine.outbox.chain.is_none());
        assert!(engine.outbox.block_params.is_none());
    }

    #[test]
    fn a_structure_edit_during_a_deferred_preparation_asks_for_another_preparation() {
        let session = session();
        let (mut engine, _directory) = engine_with_deferred_load(&session);
        let first = engine.status().published_generation;
        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("a new structure needs a chain");
        let prepared = deferred.plan().prepare();

        session
            .dispatch(UiAction::SetStereoMode(
                !session.chain_state(0).chain.stereo_enabled,
            ))
            .expect("structure edit");
        engine.sync().expect("sync");

        assert!(
            !engine
                .finish_deferred_chain(deferred, prepared)
                .expect("finish")
        );
        assert_eq!(engine.status().published_generation, first);
        assert_eq!(
            engine.take_unused_chains().len(),
            1,
            "the caller drops the stale chain with no engine lock"
        );

        let again = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("the newest structure needs a chain");
        let prepared = again.plan().prepare();
        assert!(
            engine
                .finish_deferred_chain(again, prepared)
                .expect("finish")
        );
        assert!(engine.status().published_generation > first);
        assert!(engine.begin_deferred_chain().expect("begin").is_none());
    }

    #[test]
    fn a_deferral_that_an_undo_answered_takes_no_plan() {
        let session = session();
        let (mut engine, _directory) = engine_with_deferred_load(&session);
        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("a new structure needs a chain");
        let prepared = deferred.plan().prepare();
        assert!(
            engine
                .finish_deferred_chain(deferred, prepared)
                .expect("finish")
        );
        let format = engine.running.as_ref().expect("stream").format;
        // Each plan reserves a generation, so the generations count the plans.
        let plans_before = session.chain_plan(format).expect("plan").generation();

        // The chain worker wakes after an edit and its undo, when the stream plays the session.
        let stereo = session.chain_state(0).chain.stereo_enabled;
        for enabled in [!stereo, stereo] {
            session
                .dispatch(UiAction::SetStereoMode(enabled))
                .expect("structure edit");
            engine.sync().expect("sync");
        }
        assert!(engine.chain_work_wanted(), "the edit asked for a chain");
        assert!(engine.begin_deferred_chain().expect("begin").is_none());

        assert!(!engine.chain_work_wanted(), "the deferral did not end");
        assert_eq!(
            session.chain_plan(format).expect("plan").generation(),
            plans_before + 1,
            "the chain worker took a plan that it does not need"
        );
    }

    #[test]
    fn a_deferred_chain_does_not_replace_a_newer_chain() {
        let session = session();
        let (mut engine, _directory) = engine_with_deferred_load(&session);
        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("a new structure needs a chain");
        let prepared = deferred.plan().prepare();

        assert!(engine.queue_session_chain(), "a repair");
        engine.pump();
        let newer = engine.status().published_generation;
        assert!(
            engine
                .finish_deferred_chain(deferred, prepared)
                .expect("finish")
        );

        assert!(engine.status().published_generation >= newer);
    }

    #[test]
    fn a_block_edit_and_its_undo_during_a_deferred_preparation_reach_the_new_chain() {
        let session = session();
        let (mut engine, _directory) = engine_with_deferred_load(&session);
        // The callback takes no command here, so the next chain and its settings wait in the
        // outbox, where this test can read them.
        while engine.outbox.chain.is_none() {
            assert!(engine.queue_session_chain());
            engine.pump();
        }
        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("a new structure needs a chain");
        let planned = deferred.plan().state().block_params.clone();
        let block = session.chain_state(0).chain.left[0].clone();
        let set_mix = |mix: f64| {
            let mut params = block.params.clone();
            params.mix = mix;
            session
                .dispatch(UiAction::SetBlockParams {
                    block_id: block.id.clone(),
                    params,
                })
                .expect("edit");
        };

        // An edit and its undo come before the compile reads anything. It reads only its plan.
        set_mix(0.25);
        engine.sync().expect("sync");
        set_mix(block.params.mix);
        engine.sync().expect("sync");
        let prepared = deferred.plan().prepare();
        assert_eq!(
            planned,
            session.audio_state().expect("state").block_params,
            "the chain has the settings of the session"
        );
        // A later edit with no undo is not in the chain.
        set_mix(0.5);
        engine.sync().expect("sync");

        assert!(
            engine
                .finish_deferred_chain(deferred, prepared)
                .expect("finish")
        );

        let state = session.audio_state().expect("state");
        assert_eq!(
            engine.running.as_ref().expect("stream").mirror.block_params,
            state.block_params
        );
        assert_eq!(
            engine.outbox.block_params.as_ref(),
            Some(&state.block_params),
            "the settings of the session follow the chain"
        );
        assert_eq!(
            engine.take_unused_chains().len(),
            1,
            "the caller drops the replaced chain with no engine lock"
        );
    }

    #[test]
    fn an_edit_and_its_undo_during_an_open_leave_a_chain_of_the_session() {
        let session = session();
        let (mut engine, _directory) = plain_engine_of(&session);
        engine.chain_worker = true;
        // Each edit syncs, as an edit of the engine does. The job reads its plan in `run`, so the
        // edits after `run` come after the plan. The device worker syncs after each outcome.
        let reopen = |engine: &mut EngineState, before_plan: &[bool], after_plan: &[bool]| {
            engine
                .request_device(&DeviceAction::Restart)
                .expect("restart");
            let job = engine.next_job().expect("a job");
            let edit = |engine: &mut EngineState, enabled: bool| {
                session
                    .dispatch(UiAction::SetStereoMode(enabled))
                    .expect("structure edit");
                engine.sync().expect("sync");
            };
            for enabled in before_plan {
                edit(engine, *enabled);
            }
            let outcome = job.run();
            for enabled in after_plan {
                edit(engine, *enabled);
            }
            engine.finish_device_work(outcome);
            engine.sync().expect("sync");
            assert!(engine.running.is_some(), "the stream opened");
        };
        let stereo = session.chain_state(0).chain.stereo_enabled;

        reopen(&mut engine, &[], &[]);
        assert!(
            engine.begin_deferred_chain().expect("begin").is_none(),
            "a quiet open needs no new chain"
        );

        // The open reads the mirror and the chain from one plan, so an edit and its undo during
        // the open cannot put a chain of the edit behind the mirror.
        for (before_plan, after_plan) in
            [(&[!stereo, stereo][..], &[][..]), (&[], &[!stereo, stereo])]
        {
            reopen(&mut engine, before_plan, after_plan);
            let state = session.audio_state().expect("state");
            assert_eq!(engine.running.as_ref().expect("stream").mirror, state);
            assert!(
                engine.begin_deferred_chain().expect("begin").is_none(),
                "the chain of the new stream is a chain of the session"
            );
        }

        // An undo after the plan of an edit before it reaches the stream through a new chain.
        reopen(&mut engine, &[!stereo], &[stereo]);
        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("the new stream gets a chain of the session");
        assert_eq!(
            deferred.plan().state(),
            &session.audio_state().expect("state")
        );
    }

    #[test]
    fn a_failed_chain_preparation_is_retried_on_the_next_sync() {
        let directory = tempfile::tempdir().expect("tempdir");
        let settings = StandaloneAudioSettings::load(directory.path().join("audio.settings"))
            .expect("settings");
        let session = session();
        let mut engine = start(session.clone(), settings).expect("engine");
        let first = engine.status().published_generation;

        // A chain needs at least one frame, so this format makes the preparation fail.
        let running = engine.running.as_mut().expect("stream");
        let format = running.format;
        running.format.max_frames = 0;
        session
            .dispatch(UiAction::SetStereoMode(
                !session.chain_state(0).chain.stereo_enabled,
            ))
            .expect("structure edit");
        engine.sync().expect("sync");
        let failed = engine.status();
        assert!(failed.chain_error.is_some());
        assert_eq!(failed.published_generation, first);

        engine.running.as_mut().expect("stream").format = format;
        engine.sync().expect("sync");

        let retried = engine.status();
        assert_eq!(retried.chain_error, None);
        assert!(retried.published_generation > first, "the edit must arrive");
    }

    fn plain_engine() -> (EngineState, tempfile::TempDir) {
        plain_engine_of(&session())
    }

    fn plain_engine_of(session: &ProcessorBackend) -> (EngineState, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("tempdir");
        let settings = StandaloneAudioSettings::load(directory.path().join("audio.settings"))
            .expect("settings");
        let engine = start(session.clone(), settings).expect("engine");
        (engine, directory)
    }

    fn publish_non_finite(engine: &EngineState) {
        publish_report(engine, engine.status().published_generation, None);
    }

    fn publish_report(
        engine: &EngineState,
        generation: u64,
        block: Option<(ronie_core::ChainSide, usize)>,
    ) {
        engine
            .running
            .as_ref()
            .expect("stream")
            .stream
            .telemetry()
            .publish_non_finite(NonFiniteReport { generation, block });
    }

    fn fault_kinds(engine: &EngineState) -> Vec<ChainFaultKind> {
        engine
            .chain_faults()
            .into_iter()
            .map(|fault| fault.kind)
            .collect()
    }

    fn optimize_of(engine: &EngineState) -> bool {
        engine
            .running
            .as_ref()
            .expect("stream")
            .stream
            .telemetry()
            .optimize()
    }

    fn set_optimize(session: &ProcessorBackend, enabled: bool) {
        session
            .dispatch(UiAction::SetPreference {
                key: OPTIMIZE_PREFERENCE.to_owned(),
                value: enabled,
            })
            .expect("OPTIMIZE");
    }

    #[test]
    fn the_callback_follows_the_optimize_setting_with_no_new_stream() {
        let session = session();
        let (mut engine, _directory) = plain_engine_of(&session);
        assert!(optimize_of(&engine), "OPTIMIZE is on with no stored value");
        let generation = engine.status().published_generation;

        set_optimize(&session, false);
        engine.sync().expect("sync");
        assert!(!optimize_of(&engine));
        set_optimize(&session, true);
        engine.sync().expect("sync");
        assert!(optimize_of(&engine));
        assert_eq!(engine.status().published_generation, generation);
    }

    #[test]
    fn a_new_stream_starts_with_the_optimize_setting() {
        let session = session();
        set_optimize(&session, false);
        let (mut engine, _directory) = plain_engine_of(&session);
        assert!(!optimize_of(&engine));
        engine
            .apply_device(&DeviceAction::Restart)
            .expect("restart");
        assert!(!optimize_of(&engine));
    }

    /// A state on a dummy device that takes 100 to 2048 frames, whose buffer OPTIMIZE chooses
    /// as it does for a real device. The settings file is at `path`.
    fn optimizing_engine(session: &ProcessorBackend, path: &std::path::Path) -> EngineState {
        let device = DummyBackend::with_limits(crate::audio::DummyLimits {
            buffer_frames: 100..=2048,
            ..crate::audio::DummyLimits::default()
        });
        let settings = StandaloneAudioSettings::load(path).expect("settings");
        let mut engine = EngineState::deferred(
            session.clone(),
            settings,
            Arc::new(move |_| Box::new(device.clone())),
            AudioBackendKind::Dummy,
            None,
        );
        engine.optimize_dummy_buffers = true;
        engine.run_device_work_here().expect("open");
        engine
    }

    fn running_frames(engine: &EngineState) -> u32 {
        let frames = engine
            .running
            .as_ref()
            .expect("stream")
            .negotiated
            .max_callback_frames;
        assert_eq!(
            engine.device.banner.buffer_size, frames,
            "the page shows it"
        );
        frames
    }

    fn stored_buffers(path: &std::path::Path) -> Vec<u32> {
        StandaloneAudioSettings::load(path)
            .expect("settings")
            .optimized_buffers()
            .values()
            .copied()
            .collect()
    }

    /// Three output underruns after the cool-down, as three polls of the page see them.
    fn underruns(engine: &mut EngineState) {
        engine.optimizer.end_cool_down();
        for _ in 0..3 {
            engine
                .running
                .as_ref()
                .expect("stream")
                .stream
                .telemetry()
                .publish_event(AudioEvent::OutputUnderrun);
            engine.poll();
        }
    }

    #[test]
    fn optimize_probes_the_smallest_buffer_and_steps_up_after_underruns_to_512() {
        let session = session();
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("audio.settings");
        let mut engine = optimizing_engine(&session, &path);
        assert_eq!(
            running_frames(&engine),
            128,
            "64 is too small for this device"
        );
        assert_eq!(stored_buffers(&path), [128], "the probe result is stored");

        for expected in [256, 512] {
            underruns(&mut engine);
            assert!(engine.open_wanted, "a restart at {expected}");
            engine.run_device_work_here().expect("restart");
            assert_eq!(running_frames(&engine), expected);
            assert_eq!(stored_buffers(&path), [expected]);
        }

        // 512 is the cap, so that more underruns restart nothing.
        underruns(&mut engine);
        assert!(!engine.open_wanted);
        assert_eq!(running_frames(&engine), 512);
    }

    #[test]
    fn a_stored_result_is_used_at_the_next_start_with_no_probe() {
        let session = session();
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("audio.settings");
        let mut first = optimizing_engine(&session, &path);
        underruns(&mut first);
        first.run_device_work_here().expect("restart");
        assert_eq!(running_frames(&first), 256);
        drop(first);

        let next = optimizing_engine(&session, &path);
        assert_eq!(running_frames(&next), 256, "no new probe");
    }

    #[test]
    fn optimize_off_uses_the_buffer_of_the_user_and_on_probes_again() {
        let session = session();
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("audio.settings");
        let mut engine = optimizing_engine(&session, &path);
        for _ in 0..2 {
            underruns(&mut engine);
            engine.run_device_work_here().expect("restart");
        }
        assert_eq!(running_frames(&engine), 512);

        set_optimize(&session, false);
        engine.sync().expect("sync");
        engine.run_device_work_here().expect("reopen");
        assert_eq!(
            running_frames(&engine),
            DEFAULT_BUFFER_FRAMES,
            "the user buffer"
        );
        assert_eq!(engine.config.buffer_frames, None, "the user chose nothing");
        underruns(&mut engine);
        assert!(!engine.open_wanted, "with OPTIMIZE off, no step");

        // Only a new probe gives a smaller buffer again.
        set_optimize(&session, true);
        engine.sync().expect("sync");
        engine.run_device_work_here().expect("reopen");
        assert_eq!(running_frames(&engine), 128);
    }

    #[test]
    fn a_buffer_that_the_user_chooses_with_optimize_on_is_the_result_for_the_device() {
        let session = session();
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("audio.settings");
        let mut engine = optimizing_engine(&session, &path);
        engine
            .apply_device(&DeviceAction::Configure(AudioDeviceConfig {
                sample_rate: None,
                buffer_frames: Some(1024),
                input: None,
                output: None,
                midi_inputs: Vec::new(),
            }))
            .expect("configure");
        assert_eq!(running_frames(&engine), 1024);
        assert_eq!(stored_buffers(&path), [1024]);
        assert_eq!(engine.config.buffer_frames, Some(1024));
    }

    #[test]
    fn the_device_state_shows_the_delay_of_the_running_stream() {
        let session = session();
        let (mut engine, _directory) = plain_engine_of(&session);
        engine.poll();
        let telemetry = engine
            .running
            .as_ref()
            .expect("stream")
            .stream
            .shared_telemetry();
        // The dummy stream has no queue: one input and one output buffer of 256 frames.
        let buffers = engine.device.banner.delay_ms.expect("a delay");
        assert!((buffers - 512.0 / 48.0).abs() < 1e-9, "{buffers}");

        telemetry.publish_queue(256, 256);
        telemetry.publish_chain_latency(480);
        engine.poll();
        let delay = engine.device.banner.delay_ms.expect("a delay");
        assert!((delay - (768.0 + 480.0) / 48.0).abs() < 1e-9, "{delay}");

        engine.stop_stream();
        engine.poll();
        assert_eq!(engine.device.banner.delay_ms, None, "no stream, no delay");
    }

    #[test]
    fn a_dummy_stream_keeps_its_buffer_with_optimize_on() {
        let (engine, _directory) = plain_engine();
        assert!(engine.optimize);
        assert_eq!(running_frames(&engine), DEFAULT_BUFFER_FRAMES);
    }

    /// The oversampling of the chain that the session prepares for the running stream.
    fn prepared_oversampling(engine: &EngineState, session: &ProcessorBackend) -> u8 {
        let format = engine.running.as_ref().expect("stream").format;
        let chain = session
            .chain_plan(format)
            .and_then(|plan| plan.prepare())
            .expect("chain");
        chain.change.spec.oversampling
    }

    fn set_machine(session: &ProcessorBackend, id: &'static str, value: f32) {
        session
            .set_parameter(
                ParameterId(id),
                ParameterValue(value),
                ronie_ui::backend::GesturePhase::End,
            )
            .expect(id);
    }

    #[test]
    fn optimize_off_connects_oversampling_and_optimize_on_turns_it_off() {
        let session = session();
        let (mut engine, _directory) = plain_engine_of(&session);
        assert_eq!(prepared_oversampling(&engine, &session), 1);

        set_machine(&session, "osEnabled", 1.0);
        set_machine(&session, "osFactor", 0.0);
        engine.sync().expect("sync");
        assert_eq!(
            prepared_oversampling(&engine, &session),
            1,
            "OPTIMIZE is on"
        );

        let before = engine.status().published_generation;
        set_optimize(&session, false);
        engine.sync().expect("sync");
        assert_eq!(prepared_oversampling(&engine, &session), 2, "the 2x choice");
        assert!(
            engine.status().published_generation > before,
            "a new chain for the same structure"
        );

        set_machine(&session, "osFactor", 1.0);
        engine.sync().expect("sync");
        assert_eq!(prepared_oversampling(&engine, &session), 4);
        engine
            .apply_device(&DeviceAction::Restart)
            .expect("restart");
        assert_eq!(
            engine.running.as_ref().expect("stream").format.oversampling,
            4,
            "a new stream keeps it"
        );
        let output = engine
            .render_dummy(&[0.25; DEFAULT_BUFFER_FRAMES as usize])
            .expect("an oversampled callback");
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert_eq!(engine.status().chain_error, None);

        set_optimize(&session, true);
        engine.sync().expect("sync");
        assert_eq!(prepared_oversampling(&engine, &session), 1);
    }

    #[test]
    fn oversampling_follows_its_setting_only_with_optimize_off() {
        let on = Some(ParameterValue(1.0));
        let off = Some(ParameterValue(0.0));
        for (choice, factor) in [(0.0, 2), (1.0, 4), (2.0, 8)] {
            let choice = Some(ParameterValue(choice));
            assert_eq!(stream_oversampling(false, on, choice), factor);
            assert_eq!(stream_oversampling(true, on, choice), 1);
            assert_eq!(stream_oversampling(false, off, choice), 1);
        }
        assert_eq!(stream_oversampling(false, on, None), 2);
        assert_eq!(stream_oversampling(false, None, None), 1);
    }

    #[test]
    fn a_non_finite_event_with_no_block_is_a_fault_of_the_chain() {
        let (mut engine, _directory) = plain_engine();
        assert!(engine.chain_faults().is_empty());

        publish_non_finite(&engine);
        engine.poll();

        assert_eq!(
            engine.chain_faults(),
            [ChainFault::for_chain(ChainFaultKind::NonFiniteOutput)]
        );
    }

    /// Does what the chain worker of the engine does, on this thread.
    fn run_repair(engine: &mut EngineState) {
        while let Some(deferred) = engine.begin_deferred_chain().expect("begin") {
            let prepared = deferred.plan().prepare();
            if engine
                .finish_deferred_chain(deferred, prepared)
                .expect("finish")
            {
                return;
            }
        }
    }

    #[test]
    fn a_non_finite_event_asks_for_a_repair_and_the_poll_compiles_nothing() {
        let (mut engine, _directory) = plain_engine();
        let first = engine.status().published_generation;

        publish_non_finite(&engine);
        engine.poll();

        assert_eq!(
            engine.status().published_generation,
            first,
            "the poll compiled under the engine lock"
        );
        assert!(engine.outbox.chain.is_none(), "the poll queued a chain");
        assert!(engine.repair_wanted());

        run_repair(&mut engine);

        assert!(!engine.repair_wanted());
        assert!(engine.status().published_generation > first);
    }

    #[test]
    fn repairs_keep_the_rate_limit_and_one_request_at_a_time() {
        let (mut engine, _directory) = plain_engine();
        publish_non_finite(&engine);
        engine.poll();
        assert!(engine.repair_wanted());
        let asked = engine.last_repair;

        // A slow compile must not collect requests.
        publish_non_finite(&engine);
        engine.poll();
        assert_eq!(engine.last_repair, asked, "one request waits");

        run_repair(&mut engine);
        assert!(!engine.repair_wanted());

        publish_non_finite(&engine);
        engine.poll();
        assert!(!engine.repair_wanted(), "under the rate limit");

        engine.last_repair = engine
            .last_repair
            .map(|repair| repair - MIN_REPAIR_INTERVAL);
        publish_non_finite(&engine);
        engine.poll();
        assert!(engine.repair_wanted(), "after the rate limit");
    }

    #[test]
    fn a_deferred_chain_answers_a_repair_with_no_second_compile() {
        let session = session();
        let (mut engine, _directory) = engine_with_deferred_load(&session);

        publish_non_finite(&engine);
        engine.poll();

        assert!(
            !engine.repair_wanted(),
            "the chain that the load prepares replaces the running chain"
        );
    }

    #[test]
    fn a_chain_for_a_new_structure_answers_a_repair() {
        let session = session();
        let (mut engine, _directory) = plain_engine_of(&session);
        publish_non_finite(&engine);
        engine.poll();
        assert!(engine.repair_wanted());

        session
            .dispatch(UiAction::SetStereoMode(
                !session.chain_state(0).chain.stereo_enabled,
            ))
            .expect("structure edit");
        engine.sync().expect("sync");

        assert!(!engine.repair_wanted(), "this chain is also clean");
        assert!(engine.begin_deferred_chain().expect("begin").is_none());
    }

    #[test]
    fn a_new_stream_answers_a_repair() {
        let (mut engine, _directory) = plain_engine();
        publish_non_finite(&engine);
        engine.poll();
        assert!(engine.repair_wanted());

        engine
            .apply_device(&DeviceAction::Restart)
            .expect("restart");

        assert!(!engine.repair_wanted());
    }

    #[test]
    fn a_failed_repair_shows_a_fault_and_ends_the_request() {
        let (mut engine, _directory) = plain_engine();
        publish_non_finite(&engine);
        engine.poll();
        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("a repair needs a chain");

        assert!(
            engine
                .finish_deferred_chain(deferred, Err(UiError::Control("no chain".to_owned())))
                .expect("finish")
        );

        assert!(!engine.repair_wanted());
        assert!(engine.status().chain_error.is_some());
    }

    #[test]
    fn an_abandoned_repair_can_be_asked_for_again() {
        let (mut engine, _directory) = plain_engine();
        publish_non_finite(&engine);
        engine.poll();
        assert!(engine.repair_wanted());

        engine.abandon_deferred_chain();

        assert!(!engine.repair_wanted());
        assert!(engine.begin_deferred_chain().expect("begin").is_none());
    }

    #[test]
    fn a_non_finite_fault_stays_through_its_repair_and_ends_after_a_quiet_period() {
        let (mut engine, _directory) = plain_engine();
        publish_non_finite(&engine);
        engine.poll();
        run_repair(&mut engine);
        assert_eq!(
            fault_kinds(&engine),
            [ChainFaultKind::NonFiniteOutput],
            "a repair of the same structure does not end the fault"
        );
        let repaired = engine.status().published_generation;

        engine.render_dummy(&[0.0; 256]).expect("render");
        engine.poll();
        assert_eq!(engine.status().active_generation, repaired);
        assert_eq!(
            fault_kinds(&engine),
            [ChainFaultKind::NonFiniteOutput],
            "one clean callback must not hide a model that fails on the next note"
        );

        let (_, seen) = engine.non_finite.as_mut().expect("held fault");
        *seen -= NON_FINITE_HOLD;
        engine.poll();

        assert!(engine.chain_faults().is_empty());
    }

    #[test]
    fn a_repeated_non_finite_event_renews_the_quiet_period() {
        let (mut engine, _directory) = plain_engine();
        publish_non_finite(&engine);
        engine.poll();
        let (_, seen) = engine.non_finite.as_mut().expect("held fault");
        *seen -= NON_FINITE_HOLD;

        publish_non_finite(&engine);
        engine.poll();
        engine.poll();

        assert_eq!(fault_kinds(&engine), [ChainFaultKind::NonFiniteOutput]);
    }

    #[test]
    fn a_non_finite_event_names_the_block_at_its_chain_position() {
        let session = session();
        let (mut engine, _directory) = engine_with_deferred_load(&session);
        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("chain");
        let prepared = deferred.plan().prepare();
        assert!(
            engine
                .finish_deferred_chain(deferred, prepared)
                .expect("finish")
        );
        engine.render_dummy(&[0.0; 256]).expect("render");
        engine.poll();

        let generation = engine.status().active_generation;
        publish_report(&engine, generation, Some((ronie_core::ChainSide::Left, 0)));
        engine.poll();

        let block = session.chain_state(0).chain.left[0].clone();
        assert_eq!(
            engine.chain_faults(),
            [ChainFault::for_block(
                ChainFaultKind::NonFiniteOutput,
                &block
            )]
        );
    }

    #[test]
    fn an_event_from_the_old_chain_does_not_blame_the_new_one() {
        let session = session();
        let (mut engine, _directory) = engine_with_deferred_load(&session);
        publish_non_finite(&engine);
        engine.poll();
        assert_eq!(fault_kinds(&engine), [ChainFaultKind::NonFiniteOutput]);

        let deferred = engine
            .begin_deferred_chain()
            .expect("begin")
            .expect("chain");
        let prepared = deferred.plan().prepare();
        assert!(
            engine
                .finish_deferred_chain(deferred, prepared)
                .expect("finish")
        );
        assert!(
            engine.chain_faults().is_empty(),
            "the new chain has no fault"
        );

        let new_generation = engine.status().published_generation;

        // The old chain made this report before the callback took the new chain.
        publish_report(&engine, new_generation - 1, None);
        engine.poll();
        assert!(engine.chain_faults().is_empty());

        // This report is from the new chain.
        publish_report(&engine, new_generation, None);
        engine.poll();
        assert_eq!(fault_kinds(&engine), [ChainFaultKind::NonFiniteOutput]);
    }

    #[test]
    fn a_failed_chain_preparation_is_a_fault_until_a_preparation_succeeds() {
        let (mut engine, _directory) = plain_engine();
        let session = engine.session.clone();
        let running = engine.running.as_mut().expect("stream");
        let format = running.format;
        running.format.max_frames = 0;
        session
            .dispatch(UiAction::SetStereoMode(
                !session.chain_state(0).chain.stereo_enabled,
            ))
            .expect("structure edit");
        engine.sync().expect("sync");
        assert_eq!(
            engine.chain_faults(),
            [ChainFault::for_chain(ChainFaultKind::ChainNotPrepared)]
        );

        engine.running.as_mut().expect("stream").format = format;
        engine.sync().expect("sync");

        assert!(engine.chain_faults().is_empty());
    }

    #[test]
    fn a_non_finite_fault_precedes_a_bypassed_block() {
        let session = session();
        let directory = tempfile::tempdir().expect("tempdir");
        let settings = StandaloneAudioSettings::load(directory.path().join("audio.settings"))
            .expect("settings");
        let mut engine = start(session.clone(), settings).expect("engine");
        let target = session.chain_state(0).chain.left[0].id.clone();
        session
            .dispatch(UiAction::LoadTone {
                target_insert_id: target,
                tone: ir_tone(),
                model_id: ronie_core::ModelId(8),
            })
            .expect("load");
        engine.sync().expect("sync");
        engine.render_dummy(&[0.0; 256]).expect("render");
        engine.poll();
        assert_eq!(fault_kinds(&engine), [ChainFaultKind::MissingModel]);

        publish_non_finite(&engine);
        engine.poll();

        assert_eq!(
            fault_kinds(&engine),
            [
                ChainFaultKind::NonFiniteOutput,
                ChainFaultKind::MissingModel
            ]
        );
    }

    /// An engine with a tone in the first and second place of the left lane, on its running chain.
    fn engine_with_two_tones(session: &ProcessorBackend) -> (EngineState, tempfile::TempDir) {
        let (mut engine, directory) = engine_with_deferred_load(session);
        engine.abandon_deferred_chain();
        engine.chain_worker = false;
        let target = session.chain_state(0).chain.left[1].id.clone();
        session
            .dispatch(UiAction::LoadToneWithBytes {
                target_insert_id: target,
                tone: ir_tone(),
                model_id: ronie_core::ModelId(8),
                bytes: ir_bytes(),
            })
            .expect("second load");
        engine.sync().expect("sync");
        engine.render_dummy(&[0.0; 256]).expect("render");
        assert_eq!(
            engine.status().active_generation,
            engine.status().published_generation
        );
        (engine, directory)
    }

    fn publish_failure(engine: &EngineState, generation: u64, left: u64) {
        engine
            .running
            .as_ref()
            .expect("stream")
            .stream
            .telemetry()
            .publish_failed_blocks(generation, FailedBlocks { left, right: 0 });
    }

    #[test]
    fn a_failed_block_is_named_until_a_new_chain_replaces_it() {
        let session = session();
        let (mut engine, _directory) = engine_with_two_tones(&session);
        let block = session.chain_state(0).chain.left[0].clone();

        publish_failure(&engine, engine.status().active_generation, 1);
        engine.poll();
        let expected = [ChainFault::for_block(
            ChainFaultKind::ProcessingFailed,
            &block,
        )];
        assert_eq!(engine.chain_faults(), expected);

        // Time and callbacks do not repair a block, because the chain keeps it bypassed.
        engine.render_dummy(&[0.0; 256]).expect("render");
        engine.poll();
        assert_eq!(engine.chain_faults(), expected);

        // New settings reach the running chain, so that the block stays bypassed.
        let mut edited = block.params.clone();
        edited.mix = 0.25;
        session
            .dispatch(UiAction::SetBlockParams {
                block_id: block.id.clone(),
                params: edited,
            })
            .expect("edit");
        engine.sync().expect("sync");
        engine.render_dummy(&[0.0; 256]).expect("render");
        engine.poll();
        assert_eq!(engine.chain_faults(), expected, "no new chain came");

        // A queued chain does not end the fault. The callback must take it first.
        assert!(engine.queue_session_chain());
        engine.poll();
        assert_eq!(engine.chain_faults(), expected, "the old chain still runs");

        engine.render_dummy(&[0.0; 256]).expect("render");
        assert!(engine.chain_faults().is_empty(), "the block is new");
    }

    #[test]
    fn two_failed_blocks_are_two_faults_and_an_open_slot_is_no_fault() {
        let session = session();
        let (mut engine, _directory) = engine_with_two_tones(&session);
        let lane = session.chain_state(0).chain.left.clone();
        let open = lane
            .iter()
            .position(|block| block.is_insert)
            .expect("open slot");

        let bits = 0b11 | 1 << open;
        publish_failure(&engine, engine.status().active_generation, bits);
        engine.poll();

        assert_eq!(
            engine.chain_faults(),
            [
                ChainFault::for_block(ChainFaultKind::ProcessingFailed, &lane[0]),
                ChainFault::for_block(ChainFaultKind::ProcessingFailed, &lane[1]),
            ]
        );
    }

    #[test]
    fn a_failure_of_an_old_chain_does_not_blame_the_new_one() {
        let session = session();
        let (mut engine, _directory) = engine_with_two_tones(&session);
        let old = engine.status().active_generation;

        session
            .dispatch(UiAction::SetStereoMode(
                !session.chain_state(0).chain.stereo_enabled,
            ))
            .expect("structure edit");
        engine.sync().expect("sync");
        engine.render_dummy(&[0.0; 256]).expect("render");
        assert!(engine.status().active_generation > old);

        publish_failure(&engine, old, 1);
        engine.poll();

        assert!(engine.chain_faults().is_empty());
    }

    #[test]
    fn a_failed_block_is_not_named_from_a_structure_that_it_is_not_in() {
        let session = session();
        let (mut engine, _directory) = engine_with_two_tones(&session);
        publish_failure(&engine, engine.status().active_generation, 1);
        engine.poll();
        assert_eq!(fault_kinds(&engine), [ChainFaultKind::ProcessingFailed]);

        // The callback has not taken the chain for this structure, and the positions changed.
        let first = session.chain_state(0).chain.left[0].id.clone();
        session
            .dispatch(UiAction::RemoveBlock(first))
            .expect("remove");
        engine.sync().expect("sync");
        engine.poll();

        assert!(engine.chain_faults().is_empty());
    }

    #[test]
    fn a_new_stream_ends_a_failed_block() {
        let session = session();
        let (mut engine, _directory) = engine_with_two_tones(&session);
        publish_failure(&engine, engine.status().active_generation, 1);
        engine.poll();
        assert_eq!(fault_kinds(&engine), [ChainFaultKind::ProcessingFailed]);

        engine
            .apply_device(&DeviceAction::Restart)
            .expect("restart");

        assert!(engine.chain_faults().is_empty());
        engine.poll();
        assert!(engine.chain_faults().is_empty());
    }

    #[test]
    fn faults_keep_their_order_with_a_failed_block() {
        let session = session();
        let directory = tempfile::tempdir().expect("tempdir");
        let settings = StandaloneAudioSettings::load(directory.path().join("audio.settings"))
            .expect("settings");
        let mut engine = start(session.clone(), settings).expect("engine");
        let target = session.chain_state(0).chain.left[0].id.clone();
        session
            .dispatch(UiAction::LoadTone {
                target_insert_id: target,
                tone: ir_tone(),
                model_id: ronie_core::ModelId(8),
            })
            .expect("load");
        engine.sync().expect("sync");
        engine.render_dummy(&[0.0; 256]).expect("render");
        let generation = engine.status().active_generation;

        publish_failure(&engine, generation, 1);
        publish_report(&engine, generation, None);
        engine.poll();

        assert_eq!(
            fault_kinds(&engine),
            [
                ChainFaultKind::NonFiniteOutput,
                ChainFaultKind::ProcessingFailed,
                ChainFaultKind::MissingModel,
            ]
        );
    }

    #[test]
    fn a_repaired_output_replaces_the_running_chain_with_a_clean_one() {
        let directory = tempfile::tempdir().expect("tempdir");
        let settings = StandaloneAudioSettings::load(directory.path().join("audio.settings"))
            .expect("settings");
        let mut engine = start(session(), settings).expect("engine");
        let first = engine.status().published_generation;

        // Engine parameters are always finite, so that only telemetry can start this path here.
        engine
            .running
            .as_ref()
            .expect("stream")
            .stream
            .telemetry()
            .publish_event(AudioEvent::NonFiniteOutput);
        engine.poll();
        run_repair(&mut engine);

        let replaced = engine.status().published_generation;
        assert!(replaced > first);
        engine.render_dummy(&[0.0; 256]).expect("render");
        assert_eq!(engine.status().active_generation, replaced);

        engine.poll();
        assert_eq!(
            engine.status().published_generation,
            replaced,
            "the event must trigger one replacement only"
        );
    }
}
