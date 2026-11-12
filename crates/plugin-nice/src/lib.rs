//! Ronnie 3000 as a CLAP and VST3 plugin.
//!
//! The audio thread runs the Ronnie chain through [`RtChain`]. The [`engine`] keeps that chain in
//! step with the session that the editor changes.

mod controls;
mod engine;
mod params;

use std::{
    num::NonZeroU32,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use nice_plug::{
    context::gui::GuiContext,
    editor::{
        HostMethods, SpawnedEditor,
        dpi::{LogicalSize, NativeSize},
    },
    prelude::*,
};
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, NiceEguiApp, RepaintNotifier, create_egui_editor,
};
use ronie_core::InputMode;
use ronie_dsp::{PlanarBuffer, PrepareSpec, rt_chain::RtChain};
use ronie_plugin::{ProcessorBackend, RonieProcessor};
use ronie_ui::{
    UiBackend,
    shell::{DESIGN_HEIGHT, DESIGN_WIDTH, NativeShellOptions},
    shell_view::ShellView,
};

/// The smallest editor, which is half of the design size.
const MIN_SIZE: nice_plug::editor::dpi::LogicalSize<f32> =
    nice_plug::editor::dpi::LogicalSize::new(DESIGN_WIDTH as f32 / 2.0, DESIGN_HEIGHT as f32 / 2.0);

pub use params::RonnieParams;

/// One plugin instance.
pub struct Ronnie {
    params: Arc<RonnieParams>,
    repaint: RepaintNotifier,
    backend: Arc<ProcessorBackend>,
    engine: Arc<engine::Engine>,
    /// The real-time chain, while the host keeps the plugin active.
    chain: Option<RtChain>,
    /// Planar left and right samples for one chunk of at most `max_frames`.
    scratch: Vec<f32>,
    max_frames: usize,
    /// Whether the host gives two input channels. With one, the left input feeds both.
    stereo_input: bool,
    /// The chain controls that the audio thread makes from the host values.
    host_controls: controls::HostControls,
    sample_rate: f32,
    /// The latency in samples that the host knows.
    reported_latency: u32,
}

/// What the last editor window of one plugin instance leaves for the next window.
#[derive(Debug)]
struct WindowMemory {
    /// The display scale of the last window, as `f32` bits. Zero before the first window.
    scale: AtomicU32,
    /// The project zoom that the last window had, as `f32` bits. NaN before the first window.
    zoom: AtomicU32,
    /// The design height of the chrome of the last window. Zero before the first window.
    design_height: AtomicU32,
    /// Whether the open window had a pass. Before it, the egui state does not know the display
    /// scale, so its size is wrong.
    passed: AtomicBool,
}

impl WindowMemory {
    fn new() -> Self {
        Self {
            scale: AtomicU32::new(0),
            zoom: AtomicU32::new(f32::NAN.to_bits()),
            design_height: AtomicU32::new(0),
            passed: AtomicBool::new(false),
        }
    }

    /// The display scale of the last window, if a window had one.
    fn scale(&self) -> Option<f32> {
        let scale = f32::from_bits(self.scale.load(Ordering::Acquire));
        (scale.is_finite() && scale > 0.0).then_some(scale)
    }

    /// Whether the last window had the project zoom `zoom`.
    fn had_zoom(&self, zoom: f32) -> bool {
        self.zoom.load(Ordering::Acquire) == zoom.to_bits()
    }
}

/// The display scale that baseview gives a new window on X11: `Xft.dpi` divided by 96.
///
/// The host asks for the editor size before the first window exists. So the plugin reads the
/// scale itself, one time for each process.
#[cfg(target_os = "linux")]
fn display_scale() -> f32 {
    static SCALE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| {
        let Ok((connection, _)) = x11rb::connect(None) else {
            return 1.0;
        };
        x11rb::resource_manager::new_from_default(&connection)
            .ok()
            .and_then(|database| database.get_value::<u32>("Xft.dpi", "").ok().flatten())
            .filter(|dpi| *dpi > 0)
            .map_or(1.0, |dpi| dpi as f32 / 96.0)
    })
}

/// The display scale of a new window. macOS gives the host size in points, so it needs no scale.
/// On Windows, the host gives the scale only to an open window, so a first window at a high scale
/// can still change its size once.
#[cfg(not(target_os = "linux"))]
fn display_scale() -> f32 {
    1.0
}

/// The editor of the plugin. It opens each new window at the size of the project zoom.
///
/// The egui editor opens a window at the size of its last window, and it tells the host that size
/// before the first window exists. A project can hold another zoom, and the display scale is not
/// known before the first window. So a window would open at one size and then change to another.
/// This editor makes a new egui editor at the project zoom when the zoom changed since the last
/// window, and it tells the host the size of that zoom.
pub struct RonnieWindow {
    /// The egui editor of the last window, with its state. `None` before the first window.
    editor: Mutex<Option<EguiParts>>,
    backend: Arc<ProcessorBackend>,
    params: Arc<RonnieParams>,
    repaint: RepaintNotifier,
    memory: Arc<WindowMemory>,
}

impl RonnieWindow {
    fn new(
        backend: Arc<ProcessorBackend>,
        params: Arc<RonnieParams>,
        repaint: RepaintNotifier,
    ) -> Self {
        Self {
            editor: Mutex::new(None),
            backend,
            params,
            repaint,
            memory: Arc::new(WindowMemory::new()),
        }
    }

    /// The logical size of the design box at `zoom`, with the chrome of the last window, or with
    /// the chrome that a new view starts with.
    fn design_size(&self, zoom: f32) -> LogicalSize<f32> {
        let height = match self.memory.design_height.load(Ordering::Acquire) {
            0 => shell_options(self.backend.clone())
                .initial_chrome()
                .design_height() as f32,
            known => known as f32,
        };
        LogicalSize::new(DESIGN_WIDTH as f32 * zoom, height * zoom)
    }

    /// A new egui editor that opens its window at `zoom`.
    fn egui_editor(&self, zoom: f32) -> Option<EguiParts> {
        let backend: Arc<dyn UiBackend> = self.backend.clone();
        let state = EguiEditorState::from_size(self.design_size(zoom), zoom);
        let editor = create_egui_editor(
            state.clone(),
            self.repaint.clone(),
            editor_settings(),
            RonnieEditor::new(backend, self.params.clone(), self.memory.clone()),
        )?;
        Some(EguiParts { editor, state })
    }
}

/// An egui editor and the state that it shares with its window.
struct EguiParts {
    editor: EguiEditor<RonnieEditor>,
    state: Arc<EguiEditorState>,
}

/// The options of the view in the plugin.
fn shell_options(backend: Arc<dyn UiBackend>) -> NativeShellOptions {
    NativeShellOptions {
        backend: Some(backend),
        standalone: false,
        ..NativeShellOptions::default()
    }
}

fn editor_settings() -> EguiNiceSettings {
    EguiNiceSettings::new()
        .with_tile("Ronnie 3000")
        .with_resize_hint(editor_resize_hint())
}

fn editor_resize_hint() -> ResizeHint {
    ResizeHint::resizable().with_min_logical_size(MIN_SIZE)
}

impl Editor for RonnieWindow {
    type Handle = <EguiEditor<RonnieEditor> as Editor>::Handle;

    fn spawn(
        &self,
        parent: Option<ParentWindowHandle>,
        wait_for_parent: bool,
        fallback_scale_factor: Option<f64>,
        gui_context: GuiContext,
        host: Option<HostMethods>,
    ) -> Result<SpawnedEditor<Self::Handle>, Box<dyn std::error::Error>> {
        let zoom = self.params.editor_zoom();
        let mut editor = params::lock(&self.editor);
        if editor.is_none() || !self.memory.had_zoom(zoom) {
            *editor = self.egui_editor(zoom);
            // The new window has the zoom only after its first settled pass. A window that closes
            // before that pass must not leave the zoom of an older window.
            self.memory
                .zoom
                .store(f32::NAN.to_bits(), Ordering::Release);
        }
        let Some(parts) = editor.as_ref() else {
            return Err("the egui editor is not available".into());
        };
        self.memory.passed.store(false, Ordering::Release);
        parts.editor.spawn(
            parent,
            wait_for_parent,
            fallback_scale_factor,
            gui_context,
            host,
        )
    }

    fn size(&self) -> NativeSize<u32> {
        let zoom = self.params.editor_zoom();
        let passed = self.memory.passed.load(Ordering::Acquire);
        if let Some(parts) = params::lock(&self.editor).as_ref()
            && (if parts.state.is_open() {
                passed
            } else {
                self.memory.had_zoom(zoom)
            })
        {
            // An open window has its own size, after its first pass. The next window opens at the
            // size of the last one, which the user can have changed.
            return parts.editor.size();
        }
        let scale = self.memory.scale().unwrap_or_else(display_scale);
        NativeSize::from_size(self.design_size(zoom).into(), f64::from(scale))
    }

    fn resize_hint(&self) -> ResizeHint {
        editor_resize_hint()
    }

    fn track_info_updated(&self, info: TrackInfo) {
        if let Some(parts) = params::lock(&self.editor).as_ref() {
            parts.editor.track_info_updated(info);
        }
    }
}

impl Default for Ronnie {
    fn default() -> Self {
        let backend = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 1_024,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        }));
        let backend = Arc::new(backend);
        let params = Arc::new(RonnieParams::new(backend.clone()));
        let engine = engine::Engine::new(backend.clone(), params.clone());
        Self {
            host_controls: controls::HostControls::new(&params, engine.extras()),
            engine,
            params,
            repaint: RepaintNotifier::new(),
            backend,
            chain: None,
            scratch: Vec::new(),
            max_frames: 0,
            stereo_input: true,
            sample_rate: 48_000.0,
            reported_latency: 0,
        }
    }
}

/// How long the editor waits for the host to give the window the size of an asked zoom.
const ASK_TIMEOUT: Duration = Duration::from_millis(1_500);

/// The editor. It keeps a view only while its window is open, because egui textures belong to
/// the context of one window.
pub struct RonnieEditor {
    backend: Arc<dyn UiBackend>,
    params: Arc<RonnieParams>,
    /// The host link of the open window, which tells the host about edits in the view.
    gui: Option<GuiContext>,
    view: Option<ShellView>,
    /// The physical window size that baseview reported last.
    physical: Option<(u32, u32)>,
    /// The project zoom that the editor knows. NaN before the first pass of a window.
    known_zoom: f32,
    /// The window that the editor asked the host for, while the host has not given it.
    asked: Option<AskedZoom>,
    /// What this window leaves for the next window.
    memory: Arc<WindowMemory>,
}

/// A zoom that the editor set, and when it asked the host for the window of that zoom.
#[derive(Debug, Clone, Copy)]
struct AskedZoom {
    zoom: f32,
    since: Instant,
}

impl RonnieEditor {
    fn new(
        backend: Arc<dyn UiBackend>,
        params: Arc<RonnieParams>,
        memory: Arc<WindowMemory>,
    ) -> Self {
        Self {
            backend,
            params,
            gui: None,
            view: None,
            physical: None,
            known_zoom: f32::NAN,
            asked: None,
            memory,
        }
    }

    /// Records the display scale, the chrome, and the project zoom of this window, for the next
    /// window.
    fn remember(&self, native: f32) {
        self.memory.passed.store(true, Ordering::Release);
        if let Some(view) = &self.view
            && let Ok(height) = u32::try_from(view.window_chrome().design_height())
        {
            self.memory.design_height.store(height, Ordering::Release);
        }
        if native.is_finite() && native > 0.0 {
            self.memory.scale.store(native.to_bits(), Ordering::Release);
        }
        // A window that waits for its size does not have the zoom yet.
        if self.asked.is_none() && self.known_zoom.is_finite() {
            self.memory
                .zoom
                .store(self.known_zoom.to_bits(), Ordering::Release);
        }
    }

    /// The window size in display points that fits the zoom to, or `None` while the host has
    /// not given the window that the editor asked for.
    ///
    /// A new window opens at the project zoom. A host that does not know the display scale yet
    /// gives a window of the wrong size, so the editor asks for its size and waits. A host that
    /// refuses the size gets a zoom that fits its window after [`ASK_TIMEOUT`].
    fn window(&mut self, context: &egui::Context, native: f32) -> Option<egui::Vec2> {
        let saved = self.params.editor_zoom();
        if saved.to_bits() != self.known_zoom.to_bits() {
            // A new window, or a project restore while the window is open.
            self.known_zoom = saved;
            self.asked = Some(AskedZoom {
                zoom: ShellView::ask_zoom(context, saved),
                since: Instant::now(),
            });
        }
        let (width, height) = self.physical?;
        if let Some(asked) = self.asked {
            let wanted = DESIGN_WIDTH as f32 * asked.zoom * native;
            if (width as f32 - wanted).abs() > 2.0 && asked.since.elapsed() < ASK_TIMEOUT {
                return None;
            }
            self.asked = None;
        }
        Some(egui::vec2(width as f32, height as f32) / native)
    }

    /// Records a zoom that the window size gave, for the next save of the project.
    fn record_zoom(&mut self, zoom: f32) {
        // The project holds only a zoom in the limits, so a zoom out of them is recorded clamped.
        let zoom = ShellView::clamp_zoom(zoom);
        if self.asked.is_none()
            && (zoom - self.known_zoom).abs() > 1e-4
            && self.params.replace_editor_zoom(self.known_zoom, zoom)
        {
            self.known_zoom = zoom;
        }
    }
}

impl NiceEguiApp for RonnieEditor {
    fn build(
        &mut self,
        egui_ctx: egui::Context,
        nice_gui_ctx: GuiContext,
        _frame: &mut nice_plug_egui::Frame,
    ) -> Result<(), nice_plug_egui::baseview::HandlerError> {
        ShellView::prepare_context(&egui_ctx);
        let mut options = shell_options(self.backend.clone());
        // A new window can come with no close of the last one, so this replaces any old view.
        self.view = Some(ShellView::new(&mut options, false));
        self.gui = Some(nice_gui_ctx);
        self.physical = None;
        self.known_zoom = f32::NAN;
        self.asked = None;
        Ok(())
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut nice_plug_egui::Frame) {
        if self.view.is_none() {
            return;
        }
        let context = ui.ctx().clone();
        // The zoom comes from the physical size, because baseview gives egui its size at the
        // zoom that it applies in this pass.
        let native = context.input(|input| input.viewport().native_pixels_per_point.unwrap_or(1.0));
        let window = self.window(&context, native);
        if let Some(view) = self.view.as_mut() {
            view.show(ui, window);
        }
        self.record_zoom(context.zoom_factor());
        self.remember(native);
        if let Some(gui) = &self.gui {
            params::publish_session_changes(self.backend.as_ref(), &self.params, gui);
        }
    }

    fn resized(&mut self, new_size: nice_plug_egui::baseview::WindowSize) {
        self.physical = Some((new_size.physical.width, new_size.physical.height));
    }

    fn editor_closed(&mut self) {
        self.view = None;
        self.gui = None;
    }
}

impl Plugin for Ronnie {
    const NAME: &'static str = "Ronnie 3000";
    const VENDOR: &'static str = "KonScanner";
    const URL: &'static str = "https://github.com/KonScanner/ronnie-3000";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
    ];

    type Editor = RonnieWindow;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        Some(RonnieWindow::new(
            self.backend.clone(),
            self.params.clone(),
            self.repaint.clone(),
        ))
    }

    fn activate(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl ActivateContext<Self>,
    ) -> bool {
        // The old chain drops here, on the main thread, and never on the audio thread.
        self.engine.deactivate();
        self.chain = None;
        let stereo_input = audio_io_layout.main_input_channels != NonZeroU32::new(1);
        let max_frames = buffer_config.max_buffer_size.max(1) as usize;
        let format = self.engine.stream_format(
            f64::from(buffer_config.sample_rate),
            max_frames,
            stereo_input,
        );
        match self.engine.activate(format) {
            Ok(chain) => {
                self.reported_latency = latency_of(&chain);
                context.set_latency_samples(self.reported_latency);
                self.chain = Some(chain);
                self.scratch = vec![0.0; 2 * max_frames];
                self.max_frames = max_frames;
                self.stereo_input = stereo_input;
                // The new chain takes its first controls from the host values.
                self.host_controls.reset();
                self.sample_rate = buffer_config.sample_rate;
                true
            }
            Err(error) => {
                nice_error!("Ronnie could not start its audio chain: {error}");
                false
            }
        }
    }

    fn deactivate(&mut self) {
        self.engine.deactivate();
        self.chain = None;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer<'_>,
        _aux: &mut AuxiliaryBuffers<'_>,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let Some(chain) = self.chain.as_mut() else {
            return ProcessStatus::Normal;
        };
        // Host automation applies at this block, also in a render that runs faster than real time.
        if let Some(controls) = self
            .host_controls
            .update(&self.params, self.engine.extras())
        {
            chain.set_controls(controls);
        }
        let frames = buffer.samples();
        let channels = buffer.as_slice();
        if channels.len() < 2 {
            return ProcessStatus::Normal;
        }
        // nice-plug copies the host input into the outputs. A mono input is only in the first.
        let right_input = usize::from(self.stereo_input);
        let mut start = 0;
        while start < frames {
            let count = (frames - start).min(self.max_frames);
            let end = start + count;
            let planar = &mut self.scratch[..2 * count];
            let (left, right) = planar.split_at_mut(count);
            left.copy_from_slice(&channels[0][start..end]);
            right.copy_from_slice(&channels[right_input][start..end]);
            let processed = PlanarBuffer::new(planar, 2, count)
                .and_then(|mut planar| chain.process(&mut planar));
            if processed.is_err() {
                // A chain that refuses a chunk gives silence, and the host keeps running.
                planar.fill(0.0);
            }
            let (left, right) = planar.split_at(count);
            channels[0][start..end].copy_from_slice(left);
            channels[1][start..end].copy_from_slice(right);
            start = end;
        }
        if chain.take_processing_failed() || chain.take_output_sanitized() {
            // A failed block runs as bypass, and a repaired output can leave bad state. The worker
            // compiles a fresh chain, as the standalone app does.
            self.engine.request_repair();
        }
        let latency = latency_of(chain);
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
        }
        let tail = chain.host_report().tail_seconds * f64::from(self.sample_rate);
        let tail = tail.ceil().clamp(0.0, f64::from(u32::MAX)) as u32;
        // With no tail, the host can stop the plugin on silence.
        if tail == 0 {
            ProcessStatus::Normal
        } else {
            ProcessStatus::Tail(tail)
        }
    }
}

impl Drop for Ronnie {
    fn drop(&mut self) {
        self.engine.deactivate();
        self.chain = None;
        self.engine.stop();
    }
}

/// The host latency of the active chain, in samples.
fn latency_of(chain: &RtChain) -> u32 {
    u32::try_from(chain.host_report().latency_samples).unwrap_or(u32::MAX)
}

impl ClapPlugin for Ronnie {
    const CLAP_ID: &'static str = "io.github.konscanner.ronnie-3000";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Guitar amp with NAM models");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Distortion,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Ronnie {
    const VST3_CLASS_ID: [u8; 16] = *b"Ronnie3000Amp001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nice_export_clap!(Ronnie);
nice_export_vst3!(Ronnie);

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> RonnieEditor {
        let plugin = Ronnie::default();
        let backend: Arc<dyn UiBackend> = plugin.backend.clone();
        RonnieEditor::new(
            backend,
            plugin.params.clone(),
            Arc::new(WindowMemory::new()),
        )
    }

    #[test]
    fn a_new_window_waits_for_the_size_of_the_project_zoom() {
        let mut editor = editor();
        editor.params.set_editor_zoom(1.25);
        let context = egui::Context::default();
        // A host that does not know the display scale gives the window of zoom 1.
        editor.physical = Some((DESIGN_WIDTH, DESIGN_HEIGHT));

        assert_eq!(editor.window(&context, 1.0), None, "the editor waits");
        // egui applies the asked zoom only in the next pass.
        editor.record_zoom(context.zoom_factor());
        assert!(
            (editor.params.editor_zoom() - 1.25).abs() < 1e-4,
            "the old zoom of this pass does not go into the project"
        );

        let width = (DESIGN_WIDTH as f32 * 1.25).round() as u32;
        editor.physical = Some((width, 900));
        assert_eq!(
            editor.window(&context, 1.0),
            Some(egui::vec2(width as f32, 900.0)),
            "the host gave the asked size"
        );
    }

    #[test]
    fn a_zoom_that_the_window_gives_goes_into_the_project() {
        let mut editor = editor();
        let context = egui::Context::default();
        editor.physical = Some((DESIGN_WIDTH, DESIGN_HEIGHT));
        assert!(
            editor.window(&context, 1.0).is_some(),
            "zoom 1 fits already"
        );

        editor.record_zoom(1.5);

        assert!((editor.params.editor_zoom() - 1.5).abs() < 1e-4);
        assert!(editor.asked.is_none(), "a recorded zoom asks for nothing");
        let _ = editor.window(&context, 1.0);
        assert!(editor.asked.is_none(), "the editor knows its own zoom");
    }

    #[test]
    fn a_window_that_the_host_refuses_gets_a_fitted_zoom_after_the_wait() {
        let mut editor = editor();
        editor.params.set_editor_zoom(1.5);
        let context = egui::Context::default();
        editor.physical = Some((DESIGN_WIDTH, DESIGN_HEIGHT));
        assert_eq!(editor.window(&context, 1.0), None, "the editor waits");

        let asked = editor.asked.as_mut().expect("asked");
        asked.since = Instant::now()
            .checked_sub(ASK_TIMEOUT * 2)
            .expect("a time in the past");

        assert!(
            editor.window(&context, 1.0).is_some(),
            "the zoom fits the window that the host gave"
        );
        assert!(editor.asked.is_none());
    }

    #[test]
    fn a_project_restore_while_the_window_is_open_asks_for_its_zoom() {
        let mut editor = editor();
        let context = egui::Context::default();
        editor.physical = Some((DESIGN_WIDTH, DESIGN_HEIGHT));
        assert!(editor.window(&context, 1.0).is_some());

        // The restore writes the zoom after the pass read it, and before the pass records its own.
        editor.params.set_editor_zoom(1.5);
        editor.record_zoom(1.25);
        assert!(
            (editor.params.editor_zoom() - 1.5).abs() < 1e-4,
            "the restored zoom stays"
        );

        assert_eq!(editor.window(&context, 1.0), None, "the editor asks again");
        let asked = editor.asked.expect("asked");
        assert!((asked.zoom - 1.5).abs() < 1e-4);
    }

    fn window() -> RonnieWindow {
        let plugin = Ronnie::default();
        RonnieWindow::new(
            plugin.backend.clone(),
            plugin.params.clone(),
            RepaintNotifier::new(),
        )
    }

    #[test]
    fn the_host_gets_the_size_of_the_project_zoom_before_a_window_opens() {
        let window = window();
        window.params.set_editor_zoom(1.25);
        // The tests set the scale, so that no test reads the display of the machine.
        window
            .memory
            .scale
            .store(2.0_f32.to_bits(), Ordering::Release);

        let size = window.size();

        let height = shell_options(window.backend.clone())
            .initial_chrome()
            .design_height() as f32;
        // macOS gives the host a size in points, and the other systems give it in pixels.
        let factor = if cfg!(target_os = "macos") { 1.25 } else { 2.5 };
        assert_eq!(
            (size.width, size.height),
            (
                (DESIGN_WIDTH as f32 * factor).round() as u32,
                (height * factor).round() as u32
            )
        );
    }

    #[test]
    fn a_window_at_the_project_zoom_keeps_the_size_of_the_last_window() {
        let window = window();
        window
            .memory
            .scale
            .store(1.0_f32.to_bits(), Ordering::Release);
        let zoom = window.params.editor_zoom();
        *params::lock(&window.editor) = window.egui_editor(zoom);
        let last = params::lock(&window.editor)
            .as_ref()
            .expect("an editor")
            .editor
            .size();

        window.memory.zoom.store(zoom.to_bits(), Ordering::Release);
        assert_eq!(window.size(), last);

        // A project restore while the window is closed brings another zoom.
        window.params.set_editor_zoom(2.0);
        assert_ne!(window.size(), last);
    }

    #[test]
    fn a_window_remembers_its_scale_and_its_zoom_only_when_it_has_them() {
        let mut editor = editor();
        let context = egui::Context::default();
        editor.params.set_editor_zoom(1.5);
        editor.physical = Some((DESIGN_WIDTH, DESIGN_HEIGHT));
        assert_eq!(editor.window(&context, 1.0), None, "the editor waits");

        editor.remember(1.0);
        assert_eq!(editor.memory.scale(), Some(1.0));
        assert!(
            !editor.memory.had_zoom(1.5),
            "the window does not have it yet"
        );

        let width = (DESIGN_WIDTH as f32 * 1.5).round() as u32;
        editor.physical = Some((width, 900));
        assert!(editor.window(&context, 1.0).is_some());
        editor.remember(1.0);
        assert!(editor.memory.had_zoom(1.5));
    }

    #[test]
    fn a_zoom_out_of_the_limits_opens_in_the_limits() {
        let editor = editor();
        editor.params.set_editor_zoom(-3.0);
        let zoom = editor.params.editor_zoom();
        assert!(zoom > 0.0, "{zoom}");
        assert!(editor.params.replace_editor_zoom(zoom, 1.5));
        assert!((editor.params.editor_zoom() - 1.5).abs() < 1e-4);
    }

    #[test]
    fn a_zoom_that_is_not_a_number_opens_at_zoom_1() {
        let mut editor = editor();
        editor.params.set_editor_zoom(f32::NAN);
        let context = egui::Context::default();
        editor.physical = Some((DESIGN_WIDTH, DESIGN_HEIGHT));

        assert!(
            editor.window(&context, 1.0).is_some(),
            "zoom 1 fits already"
        );
        editor.record_zoom(1.5);

        assert!((editor.params.editor_zoom() - 1.5).abs() < 1e-4);
    }
}
