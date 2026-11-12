//! Native design-space layout, preferences, and shared UI services.

use std::sync::Arc;

use crate::{backend::UiBackend, clock::UiClock};

/// Fixed design-space width.
pub const DESIGN_WIDTH: u32 = 1024;
/// Fixed design-space core height before optional chrome.
pub const DESIGN_HEIGHT: u32 = 578;
/// Maximum total banner and hint height.
pub const MAX_EXTRA_HEIGHT: i32 = 160;

/// Available product experiences. Reference Default remains the compatibility default.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Reference-compatible layout and interaction flow.
    #[default]
    ReferenceDefault,
    /// Optional Ronnie-specific flow, used only after explicit selection.
    RonieFlow,
}

/// Machine and session preferences consumed by the native shell.
#[derive(Debug, Clone)]
pub struct UiPreferences {
    /// Selected product experience.
    pub view_mode: ViewMode,
    /// Whether the hint strip is visible.
    pub hints_enabled: bool,
    /// Persisted editor scale.
    pub editor_scale: f64,
    /// Persisted session chrome height.
    pub editor_extra_height: i32,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            view_mode: ViewMode::ReferenceDefault,
            hints_enabled: true,
            editor_scale: 1.0,
            editor_extra_height: 36,
        }
    }
}

/// Optional strips that grow the design box without compressing its core.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Chrome {
    /// Whether the 44 px application banner is present.
    pub banner: bool,
    /// Whether the 36 px hint strip is present.
    pub hints: bool,
}

impl Chrome {
    /// Total additional design pixels.
    #[must_use]
    pub const fn extra_height(self) -> i32 {
        (if self.banner { 44 } else { 0 }) + (if self.hints { 36 } else { 0 })
    }

    /// Complete design height including chrome.
    #[must_use]
    pub const fn design_height(self) -> i32 {
        DESIGN_HEIGHT as i32 + self.extra_height()
    }

    /// Rounded native window size at a user scale.
    #[must_use]
    pub fn window_size(self, scale: f64) -> (i32, i32) {
        (
            (DESIGN_WIDTH as f64 * scale).round() as i32,
            (self.design_height() as f64 * scale).round() as i32,
        )
    }
}

impl From<&UiPreferences> for Chrome {
    fn from(preferences: &UiPreferences) -> Self {
        Self {
            banner: false,
            hints: preferences.hints_enabled,
        }
    }
}

/// Uniform root transform in native pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutTransform {
    /// Scale applied to the complete design-space root.
    pub scale: f64,
    /// Horizontal root origin after letterboxing.
    pub x: i32,
    /// Vertical root origin, always top-anchored on desktop.
    pub y: i32,
}

impl LayoutTransform {
    /// Fits the design box into the actual host box without changing its aspect ratio.
    #[must_use]
    pub fn fit(width: i32, height: i32, chrome: Chrome) -> Self {
        let by_width = width.max(1) as f64 / DESIGN_WIDTH as f64;
        let by_height = height.max(1) as f64 / chrome.design_height() as f64;
        let scale = by_width.min(by_height).max(0.05);
        let x = ((width as f64 - DESIGN_WIDTH as f64 * scale) / 2.0).round() as i32;
        Self { scale, x, y: 0 }
    }
}

/// Rendered-pixel density steps per native pixel for the live zoom.
const ZOOM_STEPS_PER_PIXEL: f64 = 16.0;
/// Smallest live zoom, below the zoom that the minimum window size gives.
pub(crate) const MIN_ZOOM: f64 = 0.25;
/// Largest live zoom, above the zoom of a 16K display.
pub(crate) const MAX_ZOOM: f64 = 32.0;

/// Computes the egui zoom that fits the design box into a window.
///
/// `width` and `height` are window sizes in native points. They must not include
/// the current zoom, or the result feeds back into itself. The display scale is not
/// part of the zoom, because egui multiplies the two together.
///
/// The result rounds down to a fixed density grid. The layout then always has at
/// least the design size, and a window drag does not rebuild the font atlas each frame.
#[must_use]
pub fn responsive_zoom(
    width: f32,
    height: f32,
    native_pixels_per_point: f32,
    chrome: Chrome,
) -> f32 {
    let native = if native_pixels_per_point.is_finite() && native_pixels_per_point > 0.0 {
        f64::from(native_pixels_per_point)
    } else {
        1.0
    };
    let fit = (f64::from(width) / f64::from(DESIGN_WIDTH))
        .min(f64::from(height) / f64::from(chrome.design_height()));
    if fit.is_nan() {
        return 1.0;
    }
    // The small offset keeps an exact fit such as 2.0 from rounding down one step.
    let density =
        ((fit * native * ZOOM_STEPS_PER_PIXEL + 1e-4).floor() / ZOOM_STEPS_PER_PIXEL) / native;
    density.clamp(MIN_ZOOM, MAX_ZOOM) as f32
}

/// Fits the zoom to the window. A change repeats this pass with the new zoom.
pub fn refit_zoom(context: &egui::Context, chrome: Chrome) {
    // `Context::content_rect` rounds to whole points, and that rounding would move an
    // exact fit down one zoom step. The input rectangle is exact.
    let (content, minimized, native) = context.input(|input| {
        let viewport = input.viewport();
        (
            input.content_rect(),
            viewport.minimized.unwrap_or(false),
            viewport.native_pixels_per_point.unwrap_or(1.0),
        )
    });
    if minimized || !content.is_positive() {
        return;
    }
    // egui rescales the content rectangle when it applies a new zoom, so this product
    // stays correct on the frame of the change. `ViewportInfo::inner_rect` does not.
    let window = content.size() * context.zoom_factor();
    apply_zoom(context, responsive_zoom(window.x, window.y, native, chrome));
}

/// Sets `zoom` when it differs from the current zoom. A change repeats this pass.
pub(crate) fn apply_zoom(context: &egui::Context, zoom: f32) {
    if (context.zoom_factor() - zoom).abs() > 1e-4 {
        context.set_zoom_factor(zoom);
        context.request_discard("window size changed the zoom");
    }
}

pub(crate) fn at_least_design_box(rect: egui::Rect, chrome: Chrome) -> egui::Rect {
    let design = egui::vec2(DESIGN_WIDTH as f32, chrome.design_height() as f32);
    egui::Rect::from_min_size(rect.min, rect.size().max(design))
}

/// Largest startup scale that fits the usable display and native title bar.
#[must_use]
pub fn max_start_scale(usable_width: i32, usable_height: i32, chrome: Chrome) -> f64 {
    let fit_width = usable_width as f64 / DESIGN_WIDTH as f64;
    let fit_height = (usable_height - 40) as f64 / chrome.design_height() as f64;
    fit_width.min(fit_height).clamp(1.0, 2.0)
}

/// Counter-scale and logical content size for the tone browser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrowserLayout {
    /// Transform that keeps browser cards at 1x on screen.
    pub scale: f64,
    /// Logical content width before the inverse transform.
    pub content_width: i32,
    /// Logical content height before the inverse transform.
    pub content_height: i32,
}

impl BrowserLayout {
    /// Sizes browser content inversely to the window zoom.
    #[must_use]
    pub fn new(screen_width: i32, screen_height: i32, zoom: f64) -> Self {
        let zoom = if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        };
        Self {
            scale: 1.0 / zoom,
            content_width: (screen_width as f64 * zoom).round() as i32,
            content_height: (screen_height as f64 * zoom).round() as i32,
        }
    }
}

/// Shared services owned by one native editor.
pub struct UiServices {
    /// Host-neutral processor and catalog boundary.
    pub backend: Arc<dyn UiBackend>,
    /// One scheduler for all UI polling.
    pub clock: UiClock,
    /// Current machine and session preferences.
    pub preferences: UiPreferences,
}

impl UiServices {
    /// Creates shared services around an existing backend.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>) -> Self {
        Self {
            backend,
            clock: UiClock::default(),
            preferences: UiPreferences::default(),
        }
    }
}

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use eframe::egui::{self, ViewportBuilder, ViewportCommand};
use serde_json::json;

use crate::{
    fonts::Fonts,
    scroller::{ScrollKey, Scroller},
    shell_view::ShellView,
};

/// Everything that the production shell needs in order to run.
///
/// Anything that a test or a scenario needs on top of this goes through a [`ChromeScript`].
pub struct NativeShellOptions {
    /// Close automatically after the smoke run and write a report.
    pub selftest: bool,
    /// Optional PNG capture path.
    pub capture: Option<PathBuf>,
    /// Optional JSON report path.
    pub report: Option<PathBuf>,
    /// Optional initial window size as a multiple of the design size.
    pub scale: Option<f32>,
    /// Optional initial window size in native points. This overrides `scale`.
    pub window_size: Option<[f32; 2]>,
    /// Real backend for interactive chrome, or `None` for the neutral baseline.
    pub backend: Option<Arc<dyn UiBackend>>,
    /// Whether the editor exposes standalone-only controls and settings tabs.
    pub standalone: bool,
    /// Preset metadata displayed by the preset browser.
    pub presets: Vec<ronie_core::PresetInfo>,
    /// Initial account state.
    pub authenticated: bool,
    /// Initial hint-strip state.
    pub hints_enabled: bool,
    /// Configured system-browser authentication flow, or `None` when sign-in is unavailable.
    /// A scenario supplies this. The production application supplies `community` instead.
    pub auth: Option<Box<dyn crate::services::session::AuthFlowAdapter>>,
    /// Production community service, with its own sign-in flow. This replaces `auth`.
    pub community:
        Option<crate::services::community::CommunityService<ronie_core::ReqwestTransport>>,
    /// What the host does with a key that the user pastes. With no service, the sign-in
    /// controls then open the key page, and a saved key starts the service with no restart.
    pub key_setup: Option<Box<dyn crate::services::key_setup::KeySetup>>,
    /// UI preferences used to restore update snoozes and dismissed banners.
    pub update_settings: Option<ronie_core::SettingsStore>,
    /// The daily release check. Only the standalone app gives one, so other builds stay offline.
    pub release_feed: Option<Arc<dyn crate::services::release_check::ReleaseFeed>>,
}

impl NativeShellOptions {
    /// The chrome of the first window of these options.
    #[must_use]
    pub fn initial_chrome(&self) -> Chrome {
        Chrome {
            banner: initial_window_has_banner(self),
            hints: self.hints_enabled,
        }
    }
}

impl Default for NativeShellOptions {
    fn default() -> Self {
        Self {
            selftest: false,
            capture: None,
            report: None,
            scale: None,
            window_size: None,
            backend: None,
            standalone: true,
            presets: Vec::new(),
            authenticated: false,
            hints_enabled: true,
            auth: None,
            community: None,
            key_setup: None,
            update_settings: None,
            release_feed: None,
        }
    }
}

/// Extra work on a running controller, at the points that only the shell can time.
///
/// This is the seam between the production shell and anything that must drive it to a
/// known state. The shell knows no scenario, and it calls these points in a fixed order. The
/// production run uses no script. [`crate::native_scenario`] is the other adapter.
pub trait ChromeScript {
    /// Runs once on the new controller, before the first frame.
    fn start(&mut self, _chrome: &mut crate::default::ChromeController, _context: &egui::Context) {}

    /// Runs after each frame. `frame` counts the frames that finished before this one.
    fn after_frame(&mut self, _chrome: &mut crate::default::ChromeController, _frame: u64) {}

    /// Runs when the shell holds a new clean backdrop. `count` counts the backdrops so far.
    ///
    /// A system modal paints over this backdrop, so a script must open that modal here and
    /// not earlier.
    fn backdrop_ready(&mut self, _chrome: &mut crate::default::ChromeController, _count: u32) {}

    /// Whether the shell must take clean backdrops before any surface needs one.
    ///
    /// A capture stalls the window, so the production script answers no. The shell then captures
    /// only when a dialog or a busy tone card opens.
    fn wants_clean_backdrops(&self) -> bool {
        false
    }

    /// Whether a capture must wait until a system modal is on the screen.
    fn capture_waits_for_modal(&self) -> bool {
        false
    }

    /// What the script adds to the JSON report.
    fn report(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// Whether the script did all of its work. A report does not pass when this is false.
    fn passed(&self) -> bool {
        true
    }
}

/// The production script, which does nothing.
pub(crate) struct NoScript;

impl ChromeScript for NoScript {}

/// The device state that the backend reports for the banners.
pub(crate) fn backend_device(
    options: &NativeShellOptions,
) -> Option<crate::default::AudioDeviceState> {
    options
        .backend
        .as_ref()?
        .device_settings()
        .map(|device| device.banner)
}

/// Whether an initial device state selects a visible banner rule.
#[must_use]
pub fn initial_banner_visible(device: &crate::default::AudioDeviceState) -> bool {
    let mut store = crate::default::BannerStore::new();
    store.evaluate(Some(device), 0);
    store.active().is_some()
}

/// Shows the backend's latest device state in the banners when it differs from `shown`.
pub fn sync_device_banner(
    backend: &dyn UiBackend,
    chrome: &mut crate::default::ChromeController,
    shown: &mut Option<crate::default::AudioDeviceState>,
    now_ms: i64,
) {
    let latest = backend.device_settings().map(|device| device.banner);
    if latest != *shown {
        shown.clone_from(&latest);
        chrome.set_device_state(latest, now_ms);
    }
}

/// Shows the backend's latest audio chain faults in the banners when they differ from `shown`.
///
/// Call this after [`sync_device_banner`], because a device read also polls the audio engine.
pub fn sync_chain_faults(
    backend: &dyn UiBackend,
    chrome: &mut crate::default::ChromeController,
    shown: &mut Vec<crate::backend::ChainFault>,
    now_ms: i64,
) {
    let latest = backend.chain_faults();
    if latest != *shown {
        shown.clone_from(&latest);
        chrome.set_chain_faults(latest, now_ms);
    }
}

/// Whether the first window must have room for a banner.
#[must_use]
pub fn initial_window_has_banner(options: &NativeShellOptions) -> bool {
    let device = backend_device(options);
    if device.as_ref().is_some_and(initial_banner_visible) {
        return true;
    }
    let faults = backend_faults(options);
    if faults.is_empty() {
        return false;
    }
    // A fault that the user has hidden must not make the window taller at each launch.
    let mut store = options.update_settings.clone().map_or_else(
        crate::default::BannerStore::new,
        crate::default::BannerStore::with_settings,
    );
    store.set_chain_faults(faults);
    store.evaluate(device.as_ref(), crate::services::auth::now_ms());
    store.active().is_some()
}

/// The audio chain faults that the backend reports for the banners.
pub(crate) fn backend_faults(options: &NativeShellOptions) -> Vec<crate::backend::ChainFault> {
    options
        .backend
        .as_ref()
        .map_or_else(Vec::new, |backend| backend.chain_faults())
}

/// Runs the real eframe/Glow native foundation window.
///
/// # Errors
/// Returns native window creation, rendering, or event-loop errors.
pub fn run_native(options: NativeShellOptions) -> eframe::Result<()> {
    run_window(options, NoScript, false)
}

/// Runs the native window with a script on its controller.
///
/// A capture test runs on a thread that is not the main thread, so this window accepts any
/// thread. The production window does not.
///
/// # Errors
/// Returns native window creation, rendering, or event-loop errors.
pub fn run_scripted(
    options: NativeShellOptions,
    script: impl ChromeScript + 'static,
) -> eframe::Result<()> {
    run_window(options, script, true)
}

fn run_window(
    options: NativeShellOptions,
    script: impl ChromeScript + 'static,
    allow_background: bool,
) -> eframe::Result<()> {
    let window_scale = options.scale.unwrap_or(1.0).max(0.1);
    let initial_chrome = options.initial_chrome();
    let (window_width, window_height) = initial_chrome.window_size(f64::from(window_scale));
    let initial_window_size = options
        .window_size
        .unwrap_or([window_width as f32, window_height as f32]);
    let mut native_options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Ronnie")
            .with_inner_size(initial_window_size)
            .with_min_inner_size([512.0, 289.0]),
        renderer: eframe::Renderer::Glow,
        event_loop_builder: Some(Box::new(move |builder| {
            #[cfg(target_os = "linux")]
            {
                use winit::platform::x11::EventLoopBuilderExtX11;
                builder.with_x11();
                if allow_background {
                    builder.with_any_thread(true);
                }
            }
            // Another operating system keeps the default event loop.
            #[cfg(not(target_os = "linux"))]
            let _ = builder;
        })),
        ..Default::default()
    };
    // A scripted window has no compositor to pace it on the private display of a capture. A wait
    // for a vertical blank there can take one second, which a timed toast does not survive.
    native_options.glow_options.vsync = !allow_background;
    eframe::run_native(
        "Ronnie",
        native_options,
        Box::new(move |creation| {
            ShellView::prepare_context(&creation.egui_ctx);
            creation.egui_ctx.enable_accesskit();
            Ok(Box::new(NativeShellApp::new(
                options,
                Box::new(script),
                &creation.egui_ctx,
            )))
        }),
    )
}

/// Builds the Default chrome that `options` describe, without a window.
///
/// The native shell and the real-input driver share this function, so a driven
/// scenario starts from the same state as a captured one. Returns `None` when the
/// options carry no backend.
pub(crate) fn build_chrome(
    options: &mut NativeShellOptions,
) -> Option<crate::default::ChromeController> {
    let backend = options.backend.clone()?;
    let device = backend_device(options);
    let faults = backend_faults(options);
    let mut chrome = if let Some(community) = options.community.take() {
        crate::default::ChromeController::with_community(
            backend,
            options.presets.clone(),
            options.authenticated,
            options.hints_enabled,
            community,
        )
    } else if let Some(auth) = options.auth.take() {
        crate::default::ChromeController::with_auth_flow(
            backend,
            options.presets.clone(),
            options.authenticated,
            options.hints_enabled,
            auth,
        )
    } else {
        crate::default::ChromeController::new(
            backend,
            options.presets.clone(),
            options.authenticated,
            options.hints_enabled,
        )
    };
    chrome.set_standalone(options.standalone);
    if let Some(setup) = options.key_setup.take() {
        chrome.use_key_setup(setup);
    }
    if let Some(feed) = options.release_feed.take() {
        chrome.use_release_feed(feed, options.update_settings.clone());
    }
    if let Some(settings) = options.update_settings.take() {
        chrome.use_settings(settings);
    }
    if let Some(device) = device {
        chrome.set_device_state(Some(device), 0);
    }
    if !faults.is_empty() {
        // The real time keeps this choice equal to the height of the first window.
        chrome.set_chain_faults(faults, crate::services::auth::now_ms());
    }
    Some(chrome)
}

struct NativeShellApp {
    options: NativeShellOptions,
    view: ShellView,
    script: Box<dyn ChromeScript>,
    frame_count: u64,
    max_frame_ms: f64,
    capture_requested: bool,
    capture_after_ms: u64,
    capture_requires_modal: bool,
    capture_saved: bool,
    capture_error: Option<String>,
    screenshot_tx: mpsc::Sender<Arc<egui::ColorImage>>,
    screenshot_rx: mpsc::Receiver<Arc<egui::ColorImage>>,
    report_written: bool,
    applied_design_height: i32,
}

impl NativeShellApp {
    fn new(
        mut options: NativeShellOptions,
        mut script: Box<dyn ChromeScript>,
        context: &egui::Context,
    ) -> Self {
        let (screenshot_tx, screenshot_rx) = mpsc::channel();
        let mut view = ShellView::new(&mut options, true);
        if let Some(chrome) = view.chrome_mut() {
            script.start(chrome, context);
        }
        let capture_requires_modal = script.capture_waits_for_modal();
        let capture_after_ms = if capture_requires_modal { 1_800 } else { 900 };
        let applied_design_height = view.window_chrome().design_height();
        Self {
            options,
            view,
            script,
            frame_count: 0,
            max_frame_ms: 0.0,
            capture_requested: false,
            capture_after_ms,
            capture_requires_modal,
            capture_saved: false,
            capture_error: None,
            screenshot_tx,
            screenshot_rx,
            report_written: false,
            applied_design_height,
        }
    }

    /// Moves the window edge by the chrome change, so the zoom returns to its value
    /// when the resize arrives. Until then the zoom fits the new chrome in the old window.
    fn apply_window_steps(&mut self, context: &egui::Context) {
        // The zoom in this pass is stale, and the size below depends on it.
        if context.will_discard() {
            return;
        }
        let Some(chrome) = self.view.chrome_mut() else {
            return;
        };
        let design_height = chrome.window_chrome().design_height();
        let height_change = design_height - self.applied_design_height;
        self.applied_design_height = design_height;
        let resize_requested = chrome
            .take_window_steps(1.0)
            .iter()
            .any(|step| matches!(step, crate::default::WindowStep::Resize(_, _)));
        let (maximized, fullscreen) = context.input(|input| {
            let viewport = input.viewport();
            (
                viewport.maximized.unwrap_or(false),
                viewport.fullscreen.unwrap_or(false),
            )
        });
        if resize_requested && height_change != 0 && !maximized && !fullscreen {
            // InnerSize uses zoomed points, so one design pixel stays one point here.
            let size = context.input(egui::InputState::content_rect).size()
                + egui::vec2(0.0, height_change as f32);
            context.send_viewport_cmd(ViewportCommand::InnerSize(size));
        }
    }

    fn receive_capture(&mut self) {
        let Ok(image) = self.screenshot_rx.try_recv() else {
            return;
        };
        let Some(path) = self.options.capture.as_deref() else {
            return;
        };
        match save_screenshot(path, &image) {
            Ok(()) => self.capture_saved = true,
            Err(error) => self.capture_error = Some(error),
        }
    }

    /// Whether this pass shows the final state of a self-test image, backdrop included.
    fn image_ready(&self) -> bool {
        if self.script.wants_clean_backdrops() || self.view.modal_capture.in_flight() {
            return false;
        }
        !self.capture_requires_modal
            || self
                .view
                .chrome()
                .is_some_and(crate::default::ChromeController::dialog_on_screen)
    }

    fn write_report(&mut self) {
        if self.report_written {
            return;
        }
        self.report_written = true;
        let Some(path) = self.options.report.as_deref() else {
            return;
        };
        let accessibility = self.view.chrome_mut().map_or_else(
            || json!({"passed": true, "node_count": 0, "focusable_count": 0, "unnamed_focusable": 0}),
            audit_chrome_accessibility,
        );
        let accessibility_passed = accessibility["passed"] == true;
        // A run that asked for a capture has not passed until the image is on the disk.
        let capture_passed =
            self.capture_error.is_none() && (self.options.capture.is_none() || self.capture_saved);
        let report = json!({
            "passed": capture_passed && accessibility_passed && self.script.passed(),
            "accessibility": accessibility,
            "renderer": "glow",
            "capture_saved": self.capture_saved,
            "capture_error": self.capture_error,
            "system_modal_visible": self.view.chrome().is_some_and(
                crate::default::ChromeController::system_modal_visible
            ),
            "script": self.script.report(),
            "frame_count": self.frame_count,
            "max_frame_ms": self.max_frame_ms,
            "hidden_period_ms": self.view.clock.period_ms(),
            "rss_kib": current_rss_kib(),
            "logic": logic_report(&accessibility),
            "platform_accessibility_activation": "unverified",
        });
        if let Err(error) =
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap_or_default())
        {
            self.capture_error = Some(error.to_string());
        }
    }
}

/// Whether an event comes from the pointer of the display.
const fn is_pointer_event(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::PointerMoved(_)
            | egui::Event::MouseMoved(_)
            | egui::Event::PointerButton { .. }
            | egui::Event::PointerGone
            | egui::Event::MouseWheel { .. }
            | egui::Event::Touch { .. }
            | egui::Event::Zoom(_)
            | egui::Event::Rotate(_)
    )
}

/// Keeps the pointer of the display out of a self-test window.
///
/// A self-test window opens wherever the display puts it, and the pointer stays where it was.
/// A pointer on a tile or a menu row puts a hover state into the capture. The script sets every
/// state through controller operations, so no pointer event is necessary. When a pointer event
/// arrives, this removes it and tells egui that the pointer left the window.
fn remove_display_pointer(raw_input: &mut egui::RawInput) {
    let count = raw_input.events.len();
    raw_input.events.retain(|event| !is_pointer_event(event));
    if raw_input.events.len() != count {
        raw_input.events.push(egui::Event::PointerGone);
    }
}

impl eframe::App for NativeShellApp {
    fn raw_input_hook(&mut self, _context: &egui::Context, raw_input: &mut egui::RawInput) {
        if self.options.selftest {
            remove_display_pointer(raw_input);
        }
    }

    /// Work that must continue while another window covers this one.
    ///
    /// eframe does not call `ui` for an occluded window, and a maximized browser
    /// during sign-in occludes Ronnie. A self-test must also finish in that state.
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.view.poll();
        self.receive_capture();
        let elapsed = self.view.started.elapsed();
        let capture_finished = self.options.capture.is_none()
            || self.capture_saved
            || self.capture_error.is_some()
            || elapsed >= Duration::from_secs(6);
        if self.options.selftest && capture_finished && elapsed >= Duration::from_secs(3) {
            self.write_report();
            context.send_viewport_cmd(ViewportCommand::Close);
        }
        context.request_repaint_after(self.view.repaint_period());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let frame_started = Instant::now();
        let context = ui.ctx().clone();
        refit_zoom(&context, self.view.window_chrome());
        let now_ms = self.view.begin_pass(&context, self.script.as_ref());
        let image_ready = self.image_ready();

        self.view.paint(ui);

        if let Some(chrome) = self.view.chrome_mut() {
            self.script.after_frame(chrome, self.frame_count);
        }
        self.view
            .finish_pass(&context, self.script.as_mut(), now_ms);
        self.apply_window_steps(&context);

        let elapsed = self.view.started.elapsed();
        if self.options.selftest
            && elapsed >= Duration::from_millis(self.capture_after_ms)
            && image_ready
            && !self.capture_requested
        {
            self.capture_requested = true;
            if self.options.capture.is_some() {
                let sender = self.screenshot_tx.clone();
                context.request_screenshot(move |image| {
                    let _ = sender.send(image);
                });
            }
        }
        self.frame_count += 1;
        self.max_frame_ms = self
            .max_frame_ms
            .max(frame_started.elapsed().as_secs_f64() * 1_000.0);
    }
}

/// Audits visible focus-capable nodes in one real Default scene.
#[must_use]
pub fn audit_chrome_accessibility(
    chrome: &mut crate::default::ChromeController,
) -> serde_json::Value {
    let context = egui::Context::default();
    context.enable_accesskit();
    Fonts::install(&context);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 658.0),
        )),
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ui, |ui| crate::default::show(ui, chrome));
    });
    let update = output.platform_output.accesskit_update.take();
    output.textures_delta.clear();
    let mut focusable = 0_usize;
    let mut focusables = Vec::new();
    let mut unnamed = Vec::new();
    for (id, node) in update.as_ref().into_iter().flat_map(|update| &update.nodes) {
        if node.is_hidden() || !node.supports_action(accesskit::Action::Focus) {
            continue;
        }
        focusable += 1;
        focusables.push(json!({
            "id": format!("{id:?}"),
            "role": format!("{:?}", node.role()),
            "label": node.label(),
        }));
        let labelled = node.label().is_some_and(|label| !label.trim().is_empty())
            || (node.role() == accesskit::Role::Label
                && node.value().is_some_and(|value| !value.trim().is_empty()));
        if !labelled {
            unnamed.push(format!("{id:?}: {:?}", node.role()));
        }
    }
    let nodes = update.as_ref().map_or(0, |update| update.nodes.len());
    json!({
        "passed": nodes > 0 && focusable > 0 && unnamed.is_empty(),
        "node_count": nodes,
        "focusable_count": focusable,
        "focusables": focusables,
        "unnamed_focusable": unnamed.len(),
        "unnamed": unnamed,
    })
}

/// Generates real egui accessibility nodes and exercises common scrolling without a window.
///
/// `accessibility` is the result of [`audit_chrome_accessibility`] on the chrome to report on.
pub(crate) fn logic_report(accessibility: &serde_json::Value) -> serde_json::Value {
    let context = egui::Context::default();
    context.enable_accesskit();
    Fonts::install(&context);
    let mut output = context.run_ui(egui::RawInput::default(), |ui| {
        let _ = ui.button("Save preset");
    });
    let update = output.platform_output.accesskit_update.take();
    let named_save = update
        .as_ref()
        .is_some_and(|update| format!("{update:?}").contains("Save preset"));
    output.textures_delta.clear();

    let mut scroller = Scroller::new(crate::scroller::Axis::Vertical, 200, 1_000);
    let key_handled = scroller.scroll_by_key(ScrollKey::Forward);
    json!({
        "passed": named_save && key_handled && scroller.position() == 40 && accessibility["passed"] == true,
        "accesskit_nodes": accessibility["node_count"],
        "accesskit_focusable": accessibility["focusable_count"],
        "accesskit_unnamed_focusable": accessibility["unnamed_focusable"],
        "accesskit_unnamed": accessibility["unnamed"],
        "accesskit_passed": accessibility["passed"],
        "accesskit_named_save": named_save,
        "scroll_position": scroller.position(),
    })
}

fn current_rss_kib() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        line.split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn save_screenshot(path: &Path, image: &egui::ColorImage) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&pixel.to_array());
    }
    let buffer = image::RgbaImage::from_raw(image.width() as u32, image.height() as u32, bytes)
        .ok_or_else(|| "invalid screenshot dimensions".to_owned())?;
    buffer.save(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backend::MockBackend,
        services::session::{AuthFlowAdapter, AuthPhase, AuthSnapshot, FlowEvent},
        testbed::load_scenarios,
    };

    /// A flow whose work finished while no UI pass ran.
    struct FinishedFlow(Option<FlowEvent>);

    impl AuthFlowAdapter for FinishedFlow {
        fn snapshot(&self) -> AuthSnapshot {
            AuthSnapshot {
                phase: AuthPhase::BrowserWaiting,
                copied: false,
                user_code: None,
                verification_uri: None,
                browser_problem: None,
                pending_intent: "browse".to_owned(),
                error: None,
            }
        }

        fn start_browser(&mut self, _runtime: &tokio::runtime::Handle) {}

        fn start_device(&mut self, _runtime: &tokio::runtime::Handle) {}

        fn poll(&mut self) -> Option<FlowEvent> {
            self.0.take()
        }

        fn copy_link(&mut self) -> Option<String> {
            None
        }

        fn sign_out(&mut self) -> Result<(), ronie_core::OAuthError> {
            Ok(())
        }

        fn close(&mut self) {}
    }

    /// Records the points at which the shell calls its script.
    #[derive(Clone, Default)]
    struct Recorder(Arc<std::sync::Mutex<Vec<String>>>);

    impl Recorder {
        fn push(&self, point: String) {
            self.0.lock().expect("recorder").push(point);
        }

        fn points(&self) -> Vec<String> {
            self.0.lock().expect("recorder").clone()
        }
    }

    impl ChromeScript for Recorder {
        fn start(&mut self, _chrome: &mut crate::default::ChromeController, _: &egui::Context) {
            self.push("start".to_owned());
        }

        fn after_frame(&mut self, _chrome: &mut crate::default::ChromeController, frame: u64) {
            self.push(format!("frame {frame}"));
        }

        fn backdrop_ready(&mut self, _chrome: &mut crate::default::ChromeController, count: u32) {
            self.push(format!("backdrop {count}"));
        }

        fn wants_clean_backdrops(&self) -> bool {
            true
        }

        fn capture_waits_for_modal(&self) -> bool {
            true
        }
    }

    fn fixture(id: &str) -> crate::testbed::UiScenario {
        load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == id)
            .expect("scenario")
    }

    fn run_frame(app: &mut NativeShellApp, context: &egui::Context) {
        let mut frame = eframe::Frame::_new_kittest();
        // The design size keeps the zoom, so that one run is one pass.
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 614.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            eframe::App::ui(app, ui, &mut frame);
        });
        output.textures_delta.clear();
    }

    fn backdrop() -> Arc<egui::ColorImage> {
        Arc::new(egui::ColorImage::new(
            [4, 4],
            vec![egui::Color32::BLACK; 16],
        ))
    }

    /// Runs one frame and counts the screenshots that it asks for.
    fn screenshots_of_frame(app: &mut NativeShellApp, context: &egui::Context) -> usize {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 614.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            eframe::App::ui(app, ui, &mut frame);
        });
        output.textures_delta.clear();
        output
            .viewport_output
            .values()
            .flat_map(|viewport| &viewport.commands)
            .filter(|command| matches!(command, ViewportCommand::Screenshot(_)))
            .count()
    }

    fn production_app(context: &egui::Context) -> NativeShellApp {
        let scenario = fixture("main-mono");
        let options = NativeShellOptions {
            backend: Some(Arc::new(MockBackend::from_scenario(&scenario))),
            ..NativeShellOptions::default()
        };
        Fonts::install(context);
        let mut app = NativeShellApp::new(options, Box::new(NoScript), context);
        app.view.blur_on_worker = false;
        app
    }

    #[test]
    fn a_window_with_no_dialog_takes_no_screenshot() {
        let context = egui::Context::default();
        let mut app = production_app(&context);
        let mut screenshots = 0;
        for _ in 0..4 {
            screenshots += screenshots_of_frame(&mut app, &context);
            // The old refresh period was 125 ms.
            app.view.started -= Duration::from_millis(130);
        }
        assert_eq!(screenshots, 0);
    }

    /// Runs one frame with `events` through the input hook of the window.
    fn hover_after(app: &mut NativeShellApp, context: &egui::Context, events: Vec<egui::Event>) {
        let mut frame = eframe::Frame::_new_kittest();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 614.0),
            )),
            events,
            ..Default::default()
        };
        eframe::App::raw_input_hook(app, context, &mut input);
        let mut output = context.run_ui(input, |ui| {
            eframe::App::ui(app, ui, &mut frame);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn a_self_test_window_keeps_the_display_pointer_out() {
        let context = egui::Context::default();
        let mut app = production_app(&context);
        app.options.selftest = true;
        let on_a_tile = egui::Event::PointerMoved(egui::pos2(300.0, 300.0));
        hover_after(&mut app, &context, vec![on_a_tile.clone()]);
        assert_eq!(context.input(|input| input.pointer.hover_pos()), None);

        // A pointer that was already in the window leaves it.
        let mut production = production_app(&context);
        hover_after(&mut production, &context, vec![on_a_tile.clone()]);
        assert!(context.input(|input| input.pointer.hover_pos()).is_some());
        hover_after(&mut app, &context, vec![on_a_tile]);
        assert_eq!(context.input(|input| input.pointer.hover_pos()), None);
    }

    #[test]
    fn a_self_test_window_keeps_input_that_is_not_the_pointer() {
        let mut input = egui::RawInput {
            events: vec![egui::Event::Text("a".to_owned())],
            ..Default::default()
        };
        remove_display_pointer(&mut input);
        assert_eq!(input.events, vec![egui::Event::Text("a".to_owned())]);
    }

    #[test]
    fn a_new_dialog_paints_after_one_clean_screenshot() {
        let context = egui::Context::default();
        let mut app = production_app(&context);
        assert_eq!(screenshots_of_frame(&mut app, &context), 0);

        let chrome = app.view.chrome.as_mut().expect("chrome");
        chrome.set_online(false);
        assert!(!chrome.require_connection("browse"));
        assert!(chrome.system_modal_visible());
        assert!(
            app.view.chrome.as_ref().expect("chrome").dialogs_held(),
            "the next frame must stay clean"
        );
        assert_eq!(screenshots_of_frame(&mut app, &context), 1);
        assert_eq!(screenshots_of_frame(&mut app, &context), 0, "one is enough");
        assert!(app.view.chrome.as_ref().expect("chrome").dialogs_held());

        app.view.backdrop_tx.send(backdrop()).expect("backdrop");
        assert_eq!(screenshots_of_frame(&mut app, &context), 0);
        let chrome = app.view.chrome.as_ref().expect("chrome");
        assert!(!chrome.dialogs_held(), "the backdrop has arrived");
        assert!(chrome.modal_backdrop().is_some());
        assert_eq!(screenshots_of_frame(&mut app, &context), 0);
    }

    #[test]
    fn a_dialog_waits_for_its_own_image_after_an_older_backdrop() {
        let context = egui::Context::default();
        let mut app = production_app(&context);
        let older = context.load_texture(
            "older",
            egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]),
            egui::TextureOptions::LINEAR,
        );
        let chrome = app.view.chrome.as_mut().expect("chrome");
        chrome.set_modal_backdrop(Some(older));
        chrome.set_online(false);
        assert!(!chrome.require_connection("browse"));

        assert_eq!(screenshots_of_frame(&mut app, &context), 1);

        let chrome = app.view.chrome.as_ref().expect("chrome");
        assert!(chrome.dialogs_held());
        assert!(chrome.modal_backdrop().is_none());
    }

    #[test]
    fn a_blur_on_a_worker_reaches_the_held_dialog() {
        let context = egui::Context::default();
        let mut app = production_app(&context);
        app.view.blur_on_worker = true;
        let chrome = app.view.chrome.as_mut().expect("chrome");
        chrome.set_online(false);
        assert!(!chrome.require_connection("browse"));
        assert_eq!(screenshots_of_frame(&mut app, &context), 1);

        app.view.backdrop_tx.send(backdrop()).expect("backdrop");
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.view.chrome.as_ref().expect("chrome").dialogs_held() {
            assert!(Instant::now() < deadline, "the blur did not arrive");
            let _ = screenshots_of_frame(&mut app, &context);
            std::thread::sleep(Duration::from_millis(1));
        }

        assert!(
            app.view
                .chrome
                .as_ref()
                .expect("chrome")
                .modal_backdrop()
                .is_some()
        );
    }

    #[test]
    fn a_screenshot_after_the_wait_still_gives_the_dialog_its_backdrop() {
        let context = egui::Context::default();
        let mut app = production_app(&context);
        let chrome = app.view.chrome.as_mut().expect("chrome");
        chrome.set_online(false);
        assert!(!chrome.require_connection("browse"));
        assert_eq!(screenshots_of_frame(&mut app, &context), 1);
        let _ = screenshots_of_frame(&mut app, &context);
        app.view.started -= Duration::from_millis(1_100);
        let _ = screenshots_of_frame(&mut app, &context);
        assert!(!app.view.chrome.as_ref().expect("chrome").dialogs_held());
        assert!(
            app.view
                .chrome
                .as_ref()
                .expect("chrome")
                .modal_backdrop()
                .is_none()
        );

        app.view.backdrop_tx.send(backdrop()).expect("backdrop");
        let _ = screenshots_of_frame(&mut app, &context);

        assert!(
            app.view
                .chrome
                .as_ref()
                .expect("chrome")
                .modal_backdrop()
                .is_some()
        );
    }

    #[test]
    fn a_dialog_stops_waiting_when_no_screenshot_arrives() {
        let context = egui::Context::default();
        let mut app = production_app(&context);
        let chrome = app.view.chrome.as_mut().expect("chrome");
        chrome.set_online(false);
        assert!(!chrome.require_connection("browse"));
        assert_eq!(screenshots_of_frame(&mut app, &context), 1);
        assert!(app.view.chrome.as_ref().expect("chrome").dialogs_held());

        let _ = screenshots_of_frame(&mut app, &context);
        app.view.started -= Duration::from_millis(1_100);
        let _ = screenshots_of_frame(&mut app, &context);

        assert!(!app.view.chrome.as_ref().expect("chrome").dialogs_held());
    }

    #[test]
    fn the_shell_calls_a_script_at_fixed_points_and_only_with_a_clean_backdrop() {
        let scenario = fixture("main-mono");
        let options = NativeShellOptions {
            backend: Some(Arc::new(MockBackend::from_scenario(&scenario))),
            ..NativeShellOptions::default()
        };
        let context = egui::Context::default();
        Fonts::install(&context);
        let recorder = Recorder::default();
        let mut app = NativeShellApp::new(options, Box::new(recorder.clone()), &context);
        app.view.blur_on_worker = false;
        assert!(app.capture_requires_modal);
        assert_eq!(app.capture_after_ms, 1_800);

        run_frame(&mut app, &context);
        run_frame(&mut app, &context);
        assert_eq!(recorder.points(), ["start", "frame 0", "frame 1"]);

        // The first frame asked for this backdrop.
        app.view.backdrop_tx.send(backdrop()).expect("backdrop");
        run_frame(&mut app, &context);
        assert_eq!(
            recorder.points(),
            ["start", "frame 0", "frame 1", "frame 2", "backdrop 1"]
        );

        // A backdrop that a modal paints over is not clean, so the script does not hear of it.
        let chrome = app.view.chrome.as_mut().expect("chrome");
        chrome.set_online(false);
        assert!(!chrome.require_connection("browse"));
        chrome.release_dialogs();
        assert!(app.view.modal_capture.request_due(i64::MAX, false));
        app.view.backdrop_tx.send(backdrop()).expect("backdrop");
        run_frame(&mut app, &context);
        assert_eq!(
            recorder.points(),
            [
                "start",
                "frame 0",
                "frame 1",
                "frame 2",
                "backdrop 1",
                "frame 3"
            ]
        );
    }

    #[test]
    fn the_shell_gives_a_real_scenario_script_every_turn() {
        for id in [
            "load-offline-modal",
            "load-update-notice",
            "settings-plugin",
            "browser-picking",
            "browser-signed-out-card",
            "browser-scrolled",
            "browser-return",
            "browser-signed-out-footer",
            "chrome-tile-menu",
        ] {
            let run = crate::native_scenario::NativeScenario::new(&fixture(id), None, None)
                .expect("scenario");
            let context = egui::Context::default();
            Fonts::install(&context);
            let mut app = NativeShellApp::new(run.options, Box::new(run.script), &context);
            app.view.blur_on_worker = false;
            assert!(!app.script.passed(), "{id} starts with steps that wait");
            for frame in 0..6 {
                // A screenshot arrives after the frame that asks for it. The fresh capture
                // state removes the delay between two screenshots.
                let modal = app
                    .view
                    .chrome
                    .as_ref()
                    .is_some_and(crate::default::ChromeController::system_modal_visible);
                if frame >= 2 && !modal {
                    app.view.modal_capture = crate::default::ModalCapture::default();
                    app.view.backdrop_tx.send(backdrop()).expect("backdrop");
                }
                run_frame(&mut app, &context);
            }
            assert!(app.script.passed(), "{id}: {}", app.script.report());
            assert_eq!(
                app.view
                    .chrome
                    .as_ref()
                    .is_some_and(crate::default::ChromeController::system_modal_visible),
                app.capture_requires_modal,
                "{id}"
            );
        }
    }

    /// Writes the report of a shell that ran `steps` on the main scenario.
    fn report_after(steps: Vec<crate::native_scenario::Step>) -> serde_json::Value {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("report.json");
        let options = NativeShellOptions {
            report: Some(path.clone()),
            backend: Some(Arc::new(MockBackend::from_scenario(&fixture("main-mono")))),
            ..NativeShellOptions::default()
        };
        let context = egui::Context::default();
        let script = crate::native_scenario::ScenarioScript::new(steps);
        let mut app = NativeShellApp::new(options, Box::new(script), &context);
        app.write_report();
        serde_json::from_slice(&std::fs::read(path).expect("report")).expect("JSON report")
    }

    #[test]
    fn a_report_does_not_pass_when_a_script_step_failed_or_got_no_turn() {
        use crate::native_scenario::Step;

        let report = report_after(vec![Step::Hint("Fine.".to_owned())]);
        assert_eq!(report["passed"], true, "{report}");

        // No tone browser is open.
        let report = report_after(vec![Step::BrowserZoom(2.0)]);
        assert_eq!(report["passed"], false, "{report}");

        // No clean backdrop arrived.
        let report = report_after(vec![Step::Offline]);
        assert_eq!(report["passed"], false, "{report}");
    }

    #[test]
    fn the_logic_report_describes_the_chrome_of_the_run_and_not_a_scenario() {
        use crate::{default::SettingsTab, native_scenario::Step};

        let report = report_after(vec![Step::Settings(SettingsTab::Plugin)]);
        assert_eq!(
            report["logic"]["accesskit_nodes"],
            report["accessibility"]["node_count"]
        );
        let main = report_after(Vec::new());
        assert_ne!(
            report["logic"]["accesskit_nodes"],
            main["logic"]["accesskit_nodes"]
        );
    }

    #[test]
    fn a_report_with_a_requested_capture_passes_only_when_the_image_is_saved() {
        let report_of = |saved: bool| {
            let directory = tempfile::tempdir().expect("tempdir");
            let path = directory.path().join("report.json");
            let options = NativeShellOptions {
                capture: Some(directory.path().join("capture.png")),
                report: Some(path.clone()),
                backend: Some(Arc::new(MockBackend::from_scenario(&fixture("main-mono")))),
                ..NativeShellOptions::default()
            };
            let context = egui::Context::default();
            let mut app = NativeShellApp::new(options, Box::new(NoScript), &context);
            app.capture_saved = saved;
            app.write_report();
            let report: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).expect("report")).expect("JSON");
            report
        };
        assert_eq!(report_of(false)["passed"], false);
        let saved = report_of(true);
        assert_eq!(saved["passed"], true, "{saved}");
    }

    #[test]
    fn logic_compares_a_banner_dismissal_with_unix_time() {
        let options = NativeShellOptions {
            backend: Some(Arc::new(MockBackend::from_scenario(&fixture(
                "banner-rate",
            )))),
            ..NativeShellOptions::default()
        };
        let context = egui::Context::default();
        let mut app = NativeShellApp::new(options, Box::new(NoScript), &context);
        let chrome = app.view.chrome.as_mut().expect("chrome");
        assert!(chrome.banners().active().is_some());

        // This dismissal ended on the second day of the Unix epoch.
        chrome
            .handle_banner_view_action(
                crate::default::BannerViewAction::Dismiss("rate-not-48k".to_owned()),
                0,
            )
            .expect("dismiss");
        assert!(chrome.banners().active().is_none());

        // The time since the start of the app is small, and it would keep the banner hidden.
        app.view.shown_device = None;
        eframe::App::logic(&mut app, &context, &mut eframe::Frame::_new_kittest());

        let chrome = app.view.chrome.as_ref().expect("chrome");
        assert!(chrome.banners().active().is_some(), "the dismissal ended");
    }

    fn fault() -> crate::backend::ChainFault {
        crate::backend::ChainFault {
            kind: crate::backend::ChainFaultKind::ModelRejected,
            title: Some("Green Muff".to_owned()),
            identity: "block-1:2:3".to_owned(),
        }
    }

    #[test]
    fn a_fault_at_launch_reaches_the_first_chrome_with_no_window_resize() {
        let backend = MockBackend::from_scenario(&fixture("main-mono"));
        backend.set_chain_faults(vec![fault()]);
        let options = NativeShellOptions {
            backend: Some(Arc::new(backend)),
            ..NativeShellOptions::default()
        };
        assert!(initial_window_has_banner(&options));
        let context = egui::Context::default();
        let app = NativeShellApp::new(options, Box::new(NoScript), &context);

        let chrome = app.view.chrome.as_ref().expect("chrome");
        assert_eq!(
            chrome.banners().active().expect("banner").headline,
            "Green Muff cannot run."
        );
        assert_eq!(app.view.shown_faults, [fault()]);
        assert_eq!(
            app.applied_design_height,
            Chrome {
                banner: true,
                hints: true
            }
            .design_height(),
            "the first window already has the height"
        );
    }

    #[test]
    fn logic_shows_a_live_fault_and_removes_it_when_it_clears() {
        let backend = MockBackend::from_scenario(&fixture("main-mono"));
        let options = NativeShellOptions {
            backend: Some(Arc::new(backend.clone())),
            ..NativeShellOptions::default()
        };
        let context = egui::Context::default();
        let mut app = NativeShellApp::new(options, Box::new(NoScript), &context);
        let banner = |app: &NativeShellApp| {
            app.view
                .chrome
                .as_ref()
                .and_then(|chrome| chrome.banners().active().cloned())
        };
        assert!(banner(&app).is_none());

        // eframe calls only this for an occluded window.
        backend.set_chain_faults(vec![fault()]);
        eframe::App::logic(&mut app, &context, &mut eframe::Frame::_new_kittest());
        assert!(banner(&app).is_some());

        backend.set_chain_faults(Vec::new());
        eframe::App::logic(&mut app, &context, &mut eframe::Frame::_new_kittest());
        assert!(banner(&app).is_none());
    }

    #[test]
    fn options_with_a_community_service_build_a_chrome_that_holds_it() {
        let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "main-mono")
            .expect("main scenario");
        let directory = tempfile::tempdir().expect("settings root");
        let origin = url::Url::parse("https://api.example").expect("origin");
        let policy = ronie_core::OriginPolicy::new(origin.clone(), Vec::new());
        let community = crate::services::community::CommunityService::new(
            ronie_core::OAuthConfig::new(origin, "public-client"),
            policy.clone(),
            ronie_core::ReqwestTransport::new(policy).expect("transport"),
            ronie_core::SettingsStore::standalone(
                directory.path(),
                ronie_core::UI_PREFERENCES_SETTINGS,
            ),
        );
        let mut options = NativeShellOptions {
            backend: Some(Arc::new(MockBackend::from_scenario(&scenario))),
            community: Some(community),
            ..NativeShellOptions::default()
        };

        let chrome = build_chrome(&mut options).expect("chrome");

        assert!(chrome.community().is_some());
        assert!(!chrome.session().signed_in());
        assert!(
            options.community.is_none(),
            "the controller owns the service"
        );
    }

    #[test]
    fn logic_finishes_a_sign_in_for_a_window_that_runs_no_ui_pass() {
        let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "main-mono")
            .expect("main scenario");
        let options = NativeShellOptions {
            backend: Some(Arc::new(MockBackend::from_scenario(&scenario))),
            auth: Some(Box::new(FinishedFlow(Some(FlowEvent::Finished(Ok(())))))),
            ..NativeShellOptions::default()
        };
        let context = egui::Context::default();
        let mut app = NativeShellApp::new(options, Box::new(NoScript), &context);
        let signed_in = |app: &NativeShellApp| {
            app.view
                .chrome
                .as_ref()
                .is_some_and(|chrome| chrome.session().signed_in())
        };
        assert!(!signed_in(&app));

        // eframe calls only this for an occluded window.
        eframe::App::logic(&mut app, &context, &mut eframe::Frame::_new_kittest());

        assert!(signed_in(&app));
    }
}
