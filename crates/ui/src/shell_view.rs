//! The Ronnie view with no window, so that more than one shell can show it.

use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use eframe::egui;

use crate::{
    backend::UiBackend,
    clock::UiClock,
    default::{AudioDeviceState, ChromeController, ModalCapture, ModalScrim},
    fonts::Fonts,
    shell::{
        Chrome, ChromeScript, MAX_ZOOM, MIN_ZOOM, NativeShellOptions, NoScript, apply_zoom,
        at_least_design_box, backend_device, backend_faults, build_chrome, responsive_zoom,
        sync_chain_faults, sync_device_banner,
    },
};

/// Longest time that a surface waits off the screen for its clean capture.
const CAPTURE_WAIT_MS: i64 = 1_000;

/// The Ronnie editor and the state that its passes keep.
///
/// A shell owns the window and gives this view its egui passes. The standalone window and a plugin
/// editor are two such shells. The view knows no window type, so it uses only egui.
pub struct ShellView {
    pub(crate) backend: Option<Arc<dyn UiBackend>>,
    pub(crate) chrome: Option<ChromeController>,
    pub(crate) started: Instant,
    pub(crate) clock: UiClock,
    was_visible: bool,
    /// Whether the shell can capture the window. Dialogs then wait for a blurred backdrop.
    screenshots: bool,
    pub(crate) modal_capture: ModalCapture,
    capture_wait_started_ms: Option<i64>,
    pub(crate) backdrop_count: u32,
    pub(crate) backdrop_tx: mpsc::Sender<Arc<egui::ColorImage>>,
    backdrop_rx: mpsc::Receiver<Arc<egui::ColorImage>>,
    blurred_tx: mpsc::Sender<egui::ColorImage>,
    blurred_rx: mpsc::Receiver<egui::ColorImage>,
    /// A blur stalls a frame for more than 100 ms at a scale of 2, so a worker does it. A self-test
    /// blurs in its frame, because it captures at a fixed time and its image must show the backdrop.
    pub(crate) blur_on_worker: bool,
    pub(crate) shown_device: Option<AudioDeviceState>,
    pub(crate) shown_faults: Vec<crate::backend::ChainFault>,
    /// The zoom and the design height that [`Self::show`] last gave the window.
    window_shape: Option<(f32, i32)>,
}

impl ShellView {
    /// Builds the view that `options` describe.
    ///
    /// Set `screenshots` when the shell carries out [`egui::ViewportCommand::Screenshot`]. With no
    /// screenshots, dialogs paint at once on the plain scrim.
    ///
    /// The view takes the sign-in flow and the community service out of `options`. A second view
    /// from the same options has neither, so build a new `options` for each view.
    #[must_use]
    pub fn new(options: &mut NativeShellOptions, screenshots: bool) -> Self {
        let (backdrop_tx, backdrop_rx) = mpsc::channel();
        let (blurred_tx, blurred_rx) = mpsc::channel();
        let mut clock = UiClock::default();
        clock.add_listener(|| {});
        let shown_device = backend_device(options);
        let shown_faults = backend_faults(options);
        let mut chrome = build_chrome(options);
        if screenshots && let Some(chrome) = &mut chrome {
            chrome.wait_for_dialog_backdrops();
        }
        Self {
            backend: options.backend.clone(),
            chrome,
            started: Instant::now(),
            clock,
            was_visible: true,
            screenshots,
            modal_capture: ModalCapture::default(),
            capture_wait_started_ms: None,
            backdrop_count: 0,
            backdrop_tx,
            backdrop_rx,
            blurred_tx,
            blurred_rx,
            blur_on_worker: !options.selftest,
            shown_device,
            shown_faults,
            window_shape: None,
        }
    }

    /// Sets the fonts, the colours, and the zoom rules that the view needs on `context`.
    pub fn prepare_context(context: &egui::Context) {
        Fonts::install(context);
        // egui's default selection and cursor colours are blue.
        context.all_styles_mut(|style| {
            let visuals = &mut style.visuals;
            visuals.selection.bg_fill = crate::theme::palette::EMBER;
            visuals.selection.stroke.color = crate::theme::palette::CHROME;
            visuals.text_cursor.stroke.color = crate::theme::palette::BLAZE;
            visuals.hyperlink_color = crate::theme::palette::BLAZE;
        });
        // The window size owns the zoom, so keyboard zoom must not fight it.
        context.options_mut(|egui_options| egui_options.zoom_with_keyboard = false);
    }

    /// The controller, when the options carried a backend.
    #[must_use]
    pub fn chrome(&self) -> Option<&ChromeController> {
        self.chrome.as_ref()
    }

    /// The controller, when the options carried a backend.
    pub fn chrome_mut(&mut self) -> Option<&mut ChromeController> {
        self.chrome.as_mut()
    }

    /// The chrome that sets the design height of the window.
    #[must_use]
    pub fn window_chrome(&self) -> Chrome {
        self.chrome
            .as_ref()
            .map(ChromeController::window_chrome)
            .unwrap_or_default()
    }

    /// Time until the next pass that the view needs.
    #[must_use]
    pub fn repaint_period(&self) -> Duration {
        Duration::from_millis(self.clock.period_ms())
    }

    /// Work that must continue while no pass runs, for example while another window covers this one.
    pub fn poll(&mut self) {
        let Some(chrome) = &mut self.chrome else {
            return;
        };
        chrome.poll_auth();
        chrome.poll_community();
        chrome.poll_updates(crate::services::auth::now_ms());
        if let Some(backend) = &self.backend {
            let now_ms = crate::services::auth::now_ms();
            sync_device_banner(backend.as_ref(), chrome, &mut self.shown_device, now_ms);
            sync_chain_faults(backend.as_ref(), chrome, &mut self.shown_faults, now_ms);
        }
    }

    /// One complete pass for a shell with no script, whose window size is the egui size times
    /// the zoom, as in egui-baseview.
    ///
    /// Such a shell resizes its window when the zoom changes. So when the zoom or the design height
    /// changes, the view asks for the design box. The window then keeps the size that the host gave
    /// it, snapped to the aspect of the design.
    ///
    /// `window` is the window size in display points: the physical size over the display scale.
    /// With no size yet, the zoom stays as it is.
    ///
    /// This also does the work of [`Self::poll`] and asks for the next pass, so the shell needs to
    /// call nothing else.
    pub fn show(&mut self, ui: &mut egui::Ui, window: Option<egui::Vec2>) {
        let context = ui.ctx().clone();
        self.poll();
        if let Some(window) = window {
            self.fit_zoom_to(&context, window);
        }
        let mut script = NoScript;
        let now_ms = self.begin_pass(&context, &script);
        self.paint(ui);
        self.finish_pass(&context, &mut script, now_ms);
        if let Some(chrome) = &mut self.chrome {
            // The design height below includes the chrome, so the steps are not necessary.
            let _ = chrome.take_window_steps(1.0);
        }
        let design_height = self.window_chrome().design_height();
        let shape = (context.zoom_factor(), design_height);
        if self.window_shape != Some(shape) {
            self.window_shape = Some(shape);
            context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                crate::shell::DESIGN_WIDTH as f32,
                design_height as f32,
            )));
        }
        context.request_repaint_after(self.repaint_period());
    }

    /// The first part of a pass. Returns the time of the pass in milliseconds since the start.
    pub(crate) fn begin_pass(&mut self, context: &egui::Context, script: &dyn ChromeScript) -> i64 {
        let visible = !context.input(|input| input.viewport().minimized.unwrap_or(false));
        if visible && !self.was_visible {
            self.clock.wake();
        }
        self.was_visible = visible;
        self.clock.tick(visible);

        let now_ms = self.started.elapsed().as_millis() as i64;
        self.request_scripted_backdrop(context, script, now_ms);
        now_ms
    }

    /// Sets the zoom that fits the design box into a window of `window` display points.
    ///
    /// This does not read the egui content rectangle, because egui rescales that rectangle when it
    /// applies a zoom, and a shell such as egui-baseview gives it at the new zoom already.
    ///
    /// A hidden or empty window keeps the zoom, because its size says nothing about the design.
    ///
    /// The window of a zoom has a whole number of pixels, so it can be up to one pixel smaller
    /// than the design box at that zoom. The fit allows that pixel. Without it, each resize
    /// lowers the zoom one step.
    fn fit_zoom_to(&self, context: &egui::Context, window: egui::Vec2) {
        let (minimized, native) = context.input(|input| {
            let viewport = input.viewport();
            (
                viewport.minimized.unwrap_or(false),
                viewport.native_pixels_per_point.unwrap_or(1.0),
            )
        });
        if minimized || !(window.x > 0.0 && window.y > 0.0) {
            return;
        }
        let pixel = 1.0 / if native > 0.0 { native } else { 1.0 };
        apply_zoom(
            context,
            responsive_zoom(
                window.x + pixel,
                window.y + pixel,
                native,
                self.window_chrome(),
            ),
        );
    }

    /// Sets the zoom to `zoom`, in the zoom limits, and returns the zoom that it set. A zoom that
    /// is not a number gives 1.
    ///
    /// egui applies the zoom at the start of the next pass. That pass of [`Self::show`] then asks
    /// the window for the design box at this zoom. Give it no window size until the window has
    /// that size, or it fits the zoom to the old window again.
    pub fn ask_zoom(context: &egui::Context, zoom: f32) -> f32 {
        let zoom = Self::clamp_zoom(zoom);
        apply_zoom(context, zoom);
        zoom
    }

    /// `zoom` in the zoom limits. A zoom that is not finite gives 1.
    #[must_use]
    pub fn clamp_zoom(zoom: f32) -> f32 {
        if zoom.is_finite() {
            f64::from(zoom).clamp(MIN_ZOOM, MAX_ZOOM) as f32
        } else {
            1.0
        }
    }

    /// Paints the editor in `ui`.
    pub(crate) fn paint(&mut self, ui: &mut egui::Ui) {
        // A pass with a stale zoom can be smaller than the design box. That pass must still
        // lay out every widget, or egui clears keyboard focus and any held click.
        let layout_rect = at_least_design_box(ui.max_rect(), self.window_chrome());
        let mut layout_ui = ui.new_child(egui::UiBuilder::new().max_rect(layout_rect));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(&mut layout_ui, |ui| {
                if let Some(chrome) = &mut self.chrome {
                    crate::default::show(ui, chrome);
                } else {
                    paint_placeholder(ui);
                }
            });
    }

    /// The last part of a pass: the backdrops that dialogs paint on.
    pub(crate) fn finish_pass(
        &mut self,
        context: &egui::Context,
        script: &mut dyn ChromeScript,
        now_ms: i64,
    ) {
        self.receive_modal_backdrop(context, script);
        self.request_surface_backdrop(context, now_ms);
    }

    /// Asks for a capture, because the script waits for clean backdrops.
    fn request_scripted_backdrop(
        &mut self,
        context: &egui::Context,
        script: &dyn ChromeScript,
        now_ms: i64,
    ) {
        let Some(chrome) = self.chrome.as_ref() else {
            return;
        };
        if script.wants_clean_backdrops()
            && self
                .modal_capture
                .request_due(now_ms, chrome.dialog_on_screen())
        {
            self.request_screenshot(context);
        }
    }

    /// Asks for a capture, when a surface that needs a backdrop opened.
    ///
    /// The screenshot shows the next pass, and the surface stays out of that pass, so the image is
    /// clean.
    fn request_surface_backdrop(&mut self, context: &egui::Context, now_ms: i64) {
        if !self.screenshots {
            return;
        }
        let Some(chrome) = self.chrome.as_mut() else {
            return;
        };
        if self
            .modal_capture
            .request_for(chrome.backdrop_user(), now_ms)
        {
            chrome.begin_backdrop_capture();
            self.request_screenshot(context);
        }
        let Some(chrome) = self.chrome.as_mut() else {
            return;
        };
        if !chrome.capture_held() {
            self.capture_wait_started_ms = None;
            return;
        }
        // This reads the clock again, because an inline blur can take a long time in this frame.
        let now_ms = self.started.elapsed().as_millis() as i64;
        let started = *self.capture_wait_started_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(started) >= CAPTURE_WAIT_MS {
            chrome.release_dialogs();
        }
    }

    pub(crate) fn request_screenshot(&self, context: &egui::Context) {
        let sender = self.backdrop_tx.clone();
        context.request_screenshot(move |image| {
            let _ = sender.send(image);
        });
    }

    fn receive_modal_backdrop(&mut self, context: &egui::Context, script: &mut dyn ChromeScript) {
        if let Ok(image) = self.backdrop_rx.try_recv() {
            let clean = self
                .chrome
                .as_ref()
                .is_some_and(|chrome| !chrome.dialog_on_screen());
            if self.modal_capture.complete(!clean) {
                self.start_blur(image, context);
            }
        }
        let Ok(blurred) = self.blurred_rx.try_recv() else {
            return;
        };
        let Some(chrome) = self.chrome.as_mut() else {
            return;
        };
        self.backdrop_count = self.backdrop_count.saturating_add(1);
        let texture = context.load_texture("modal-backdrop", blurred, egui::TextureOptions::LINEAR);
        chrome.set_modal_backdrop(Some(texture));
        script.backdrop_ready(chrome, self.backdrop_count);
    }

    fn start_blur(&self, image: Arc<egui::ColorImage>, context: &egui::Context) {
        let sender = self.blurred_tx.clone();
        if !self.blur_on_worker {
            let _ = sender.send(ModalScrim::blur_snapshot(&image));
            return;
        }
        let context = context.clone();
        // With no thread, a held dialog stops its wait and paints on the plain scrim.
        let _ = std::thread::Builder::new()
            .name("ronie-backdrop-blur".to_owned())
            .spawn(move || {
                let _ = sender.send(ModalScrim::blur_snapshot(&image));
                context.request_repaint();
            });
    }
}

// A plugin editor must be `Send`, so a change that makes the view `!Send` fails here.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<ShellView>();
};

/// The neutral baseline that a view with no backend shows.
fn paint_placeholder(ui: &mut egui::Ui) {
    ui.allocate_ui_with_layout(
        egui::vec2(1024.0, 45.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(24.0);
            ui.label(
                egui::RichText::new("Ronnie")
                    .family(egui::FontFamily::Monospace)
                    .size(18.0)
                    .color(egui::Color32::WHITE),
            );
            ui.add_space(16.0);
            ui.label(
                egui::RichText::new("Native UI foundation")
                    .size(13.0)
                    .color(crate::theme::palette::TEXT_MUTED),
            );
        },
    );
    ui.separator();
    ui.centered_and_justified(|ui| {
        ui.label(
            egui::RichText::new("Reference Default")
                .size(14.0)
                .color(crate::theme::palette::TEXT_MUTED),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backend::MockBackend, testbed::load_scenarios};

    /// A plugin view on the main scenario, with no screenshots.
    fn plugin_view() -> ShellView {
        let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "main-mono")
            .expect("main scenario");
        let mut options = NativeShellOptions {
            backend: Some(Arc::new(MockBackend::from_scenario(&scenario))),
            standalone: false,
            ..NativeShellOptions::default()
        };
        ShellView::new(&mut options, false)
    }

    /// Runs one frame of `view` in a window of `window` points and returns its window commands.
    fn frame(
        context: &egui::Context,
        view: &mut ShellView,
        window: Option<egui::Vec2>,
    ) -> Vec<egui::ViewportCommand> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 614.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| view.show(ui, window));
        output.textures_delta.clear();
        output
            .viewport_output
            .values()
            .flat_map(|viewport| viewport.commands.clone())
            .collect()
    }

    fn inner_sizes(commands: &[egui::ViewportCommand]) -> Vec<egui::Vec2> {
        commands
            .iter()
            .filter_map(|command| match command {
                egui::ViewportCommand::InnerSize(size) => Some(*size),
                _ => None,
            })
            .collect()
    }

    fn context() -> egui::Context {
        let context = egui::Context::default();
        Fonts::install(&context);
        context
    }

    #[test]
    fn a_plugin_window_takes_its_zoom_from_the_window_and_asks_for_the_design_box() {
        let context = context();
        let mut view = plugin_view();
        let design = egui::vec2(
            crate::shell::DESIGN_WIDTH as f32,
            view.window_chrome().design_height() as f32,
        );

        let sizes = inner_sizes(&frame(&context, &mut view, Some(egui::vec2(1280.0, 768.0))));

        assert!(
            (context.zoom_factor() - 1.25).abs() < 1e-4,
            "{}",
            context.zoom_factor()
        );
        assert!(!sizes.is_empty(), "the window must keep the design aspect");
        assert!(sizes.iter().all(|size| *size == design), "{sizes:?}");
        let again = frame(&context, &mut view, Some(egui::vec2(1280.0, 768.0)));
        assert!(
            inner_sizes(&again).is_empty(),
            "an unchanged window asks for nothing"
        );
    }

    #[test]
    fn a_window_of_whole_pixels_keeps_the_zoom_that_asked_for_it() {
        let context = context();
        let mut view = plugin_view();
        let design = egui::vec2(
            crate::shell::DESIGN_WIDTH as f32,
            view.window_chrome().design_height() as f32,
        );
        for step in 8..=48_u8 {
            let zoom = f32::from(step) / 16.0;
            // The platform rounds the asked window to whole pixels.
            let window = (design * zoom).round();

            let _ = frame(&context, &mut view, Some(window));

            assert!(
                (context.zoom_factor() - zoom).abs() < 1e-4,
                "zoom {zoom} in a {window:?} window gave {}",
                context.zoom_factor()
            );
        }
    }

    #[test]
    fn a_hidden_or_empty_plugin_window_keeps_its_zoom() {
        let context = context();
        let mut view = plugin_view();
        let _ = frame(&context, &mut view, Some(egui::vec2(1280.0, 768.0)));

        let _ = frame(&context, &mut view, Some(egui::Vec2::ZERO));

        assert!(
            (context.zoom_factor() - 1.25).abs() < 1e-4,
            "{}",
            context.zoom_factor()
        );
    }

    #[test]
    fn a_plugin_view_with_no_screenshots_paints_a_dialog_at_once() {
        let context = context();
        let mut view = plugin_view();
        let chrome = view.chrome_mut().expect("chrome");
        chrome.set_online(false);
        assert!(!chrome.require_connection("browse"));

        let commands = frame(&context, &mut view, None);

        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Screenshot(_))),
            "{commands:?}"
        );
        let chrome = view.chrome().expect("chrome");
        assert!(!chrome.dialogs_held());
        assert!(chrome.dialog_on_screen());
    }
}
