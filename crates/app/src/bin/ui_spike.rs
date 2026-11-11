//! Throwaway native UI capability spike. This is not product UI architecture.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, DroppedFileHandle, Event, FontData, FontDefinitions,
    FontFamily, FontId, Frame, Key, Modifiers, Pos2, Rect, RichText, Role, Sense, Stroke,
    TouchDeviceId, TouchId, TouchPhase, Vec2, ViewportBuilder, ViewportCommand, WidgetInfo,
};
use ronie_app::platform::{WindowProbe, embedded_parent, native_picker, probe_window};
use serde_json::json;

const BACKGROUND: Color32 = Color32::from_rgb(10, 10, 12);
const PANEL: Color32 = Color32::from_rgb(25, 25, 29);
const PANEL_ALT: Color32 = Color32::from_rgb(35, 35, 40);
const TEXT: Color32 = Color32::from_rgb(241, 241, 236);
const MUTED: Color32 = Color32::from_rgb(157, 157, 151);
const ACCENT: Color32 = Color32::from_rgb(230, 230, 40);

#[derive(Default)]
struct Config {
    report: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    scale: Option<f32>,
    selftest: bool,
    picker: bool,
}

#[derive(Default)]
struct LogicProbe {
    keyboard_saves: usize,
    mouse_saves: usize,
    touch_saves: usize,
    file_drops: usize,
    accesskit_updates: usize,
    accesskit_max_nodes: usize,
    accesskit_named_save: bool,
    accesskit_named_output: bool,
}

struct SpikeApp {
    config: Config,
    logic: LogicProbe,
    started: Instant,
    last_frame: Option<Instant>,
    was_visible: bool,
    window: WindowProbe,
    embedded_parent: bool,
    frames: usize,
    visible_frames: usize,
    hidden_frames: usize,
    visible_intervals_ms: Vec<f64>,
    hidden_intervals_ms: Vec<f64>,
    update_times_us: Vec<f64>,
    save_clicks: usize,
    dropped_files: usize,
    stereo: bool,
    tuner: bool,
    input_gain: f32,
    gate: f32,
    bass: f32,
    middle: f32,
    treble: f32,
    output_gain: f32,
    native_pixels_per_point: Option<f32>,
    effective_pixels_per_point: Option<f32>,
    logical_widths: Vec<f32>,
    minimized_observed: bool,
    picker_attempted: bool,
    picker_completed: bool,
    picker_selected: bool,
    screenshot_requested: bool,
    screenshot_state: String,
    screenshot_tx: mpsc::Sender<Arc<egui::ColorImage>>,
    screenshot_rx: mpsc::Receiver<Arc<egui::ColorImage>>,
    report_written: bool,
}

impl SpikeApp {
    fn new(config: Config, logic: LogicProbe) -> Self {
        let (screenshot_tx, screenshot_rx) = mpsc::channel();
        Self {
            config,
            logic,
            started: Instant::now(),
            last_frame: None,
            was_visible: true,
            window: WindowProbe::default(),
            embedded_parent: false,
            frames: 0,
            visible_frames: 0,
            hidden_frames: 0,
            visible_intervals_ms: Vec::new(),
            hidden_intervals_ms: Vec::new(),
            update_times_us: Vec::new(),
            save_clicks: 0,
            dropped_files: 0,
            stereo: true,
            tuner: false,
            input_gain: 0.0,
            gate: -48.0,
            bass: 0.0,
            middle: 0.0,
            treble: 0.0,
            output_gain: -3.0,
            native_pixels_per_point: None,
            effective_pixels_per_point: None,
            logical_widths: Vec::new(),
            minimized_observed: false,
            picker_attempted: false,
            picker_completed: false,
            picker_selected: false,
            screenshot_requested: false,
            screenshot_state: "not-requested".to_owned(),
            screenshot_tx,
            screenshot_rx,
            report_written: false,
        }
    }

    fn track_frame(&mut self, ctx: &egui::Context) {
        self.frames += 1;
        let now = Instant::now();
        if let Some(previous) = self.last_frame.replace(now) {
            let elapsed = previous.elapsed().as_secs_f64() * 1000.0;
            if self.was_visible {
                self.visible_intervals_ms.push(elapsed);
            } else {
                self.hidden_intervals_ms.push(elapsed);
            }
        }

        let visible = ctx.input(|input| input.viewport().visible().unwrap_or(true));
        self.was_visible = visible;
        if visible {
            self.visible_frames += 1;
        } else {
            self.hidden_frames += 1;
            self.minimized_observed = true;
        }
        let width = ctx.content_rect().width();
        if !self.logical_widths.contains(&width) {
            self.logical_widths.push(width);
        }
        self.native_pixels_per_point = ctx.native_pixels_per_point();
        self.effective_pixels_per_point = Some(ctx.pixels_per_point());
    }

    fn draw(&mut self, ui: &mut egui::Ui, native: Option<&eframe::Frame>) {
        let ctx = ui.ctx().clone();
        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(Frame::new().fill(PANEL_ALT))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(format!(
                            "UI spike  |  {}  |  {} frames  |  {:.2}x  |  {} dropped",
                            self.window.backend.unwrap_or("probing"),
                            self.frames,
                            self.effective_pixels_per_point.unwrap_or(1.0),
                            self.dropped_files
                        ))
                        .small()
                        .color(MUTED)
                        .family(mono_family()),
                    );
                });
            });

        egui::Panel::bottom("faceplate")
            .exact_size(156.0)
            .frame(Frame::new().fill(PANEL_ALT))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    knob(ui, "Input", &mut self.input_gain, -24.0..=24.0, 0.0);
                    knob(ui, "Gate", &mut self.gate, -96.0..=0.0, -48.0);
                    knob(ui, "Bass", &mut self.bass, -12.0..=12.0, 0.0);
                    knob(ui, "Middle", &mut self.middle, -12.0..=12.0, 0.0);
                    knob(ui, "Treble", &mut self.treble, -12.0..=12.0, 0.0);
                    knob(ui, "Output", &mut self.output_gain, -60.0..=12.0, -3.0);
                });
            });

        egui::Panel::top("chrome")
            .exact_size(62.0)
            .frame(Frame::new().fill(PANEL))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new("RONIE 3000")
                            .size(19.0)
                            .color(ACCENT)
                            .family(mono_family()),
                    );
                    ui.add_space(24.0);
                    if ui.button("Save preset").clicked() {
                        self.save_clicks += 1;
                    }
                    if ui.button("Open model").clicked() {
                        self.open_picker(native);
                    }
                    ui.menu_button("Presets", |ui| {
                        if ui.button("Factory clean").clicked() {
                            self.save_clicks += 1;
                        }
                        if ui.button("User preset").clicked() {
                            self.save_clicks += 1;
                        }
                    });
                    ui.checkbox(&mut self.stereo, "Stereo");
                    if ui.selectable_label(self.tuner, "Tuner").clicked() {
                        self.tuner = !self.tuner;
                    }
                    ui.menu_button("Account", |ui| {
                        ui.label("Signed out for the capability spike");
                        let _ = ui.button("Sign in");
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BACKGROUND))
            .show(ui, |ui| {
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.label(
                        RichText::new("SIGNAL CHAIN")
                            .size(18.0)
                            .color(TEXT)
                            .family(mono_family()),
                    );
                });
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    chain_card(ui, "Input", "Dual mono", 0.38);
                    chain_card(ui, "Amplifier", "Standard model", 0.62);
                    chain_card(ui, "Cabinet", "Prepared IR", 0.48);
                    chain_card(ui, "+", "Insert a block", 0.0);
                });
            });

        let dropped = ctx.input(|input| input.raw.dropped_files.len());
        if dropped > 0 {
            self.dropped_files += dropped;
            ctx.input_mut(|input| input.raw.dropped_files.clear());
        }
    }

    fn open_picker(&mut self, native: Option<&eframe::Frame>) {
        self.picker_attempted = true;
        if let Some(frame) = native {
            match embedded_parent(frame) {
                Ok(parent) => {
                    self.picker_selected = native_picker(&parent).pick_file().is_some();
                }
                Err(_) => {
                    self.picker_completed = false;
                    return;
                }
            }
        }
        self.picker_completed = true;
    }

    fn native_automation(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let elapsed = self.started.elapsed().as_secs_f32();
        if self.config.selftest && self.config.picker && elapsed >= 4.0 && !self.picker_attempted {
            self.open_picker(Some(frame));
        }
        if self.config.selftest && elapsed >= 8.0 && !self.screenshot_requested {
            if self.config.screenshot.is_some() {
                let sender = self.screenshot_tx.clone();
                ctx.request_screenshot(move |image| {
                    let _ = sender.send(image);
                });
                self.screenshot_state = "requested".to_owned();
            }
            self.screenshot_requested = true;
        }
        if self.config.selftest && elapsed >= 11.0 {
            self.write_report();
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn receive_screenshot(&mut self) {
        let Ok(image) = self.screenshot_rx.try_recv() else {
            return;
        };
        let Some(path) = self.config.screenshot.as_deref() else {
            return;
        };
        match save_screenshot(path, &image) {
            Ok(()) => {
                self.screenshot_state = format!("saved:{}x{}", image.width(), image.height());
            }
            Err(error) => {
                self.screenshot_state = format!("error:{error}");
            }
        }
    }

    fn write_report(&mut self) {
        if self.report_written {
            return;
        }
        self.report_written = true;
        let Some(path) = self.config.report.as_deref() else {
            return;
        };
        let (rss_kib, peak_rss_kib) = memory_kib();
        let report = json!({
            "schema_version": 1,
            "native": {
                "backend": self.window.backend,
                "raw_window_handle": self.window.has_window_handle,
                "raw_display_handle": self.window.has_display_handle,
                "embedded_parent_contract": self.embedded_parent,
                "frames": self.frames,
                "visible_frames": self.visible_frames,
                "hidden_frames": self.hidden_frames,
                "minimized_observed": self.minimized_observed,
                "logical_widths": self.logical_widths,
                "native_pixels_per_point": self.native_pixels_per_point,
                "effective_pixels_per_point": self.effective_pixels_per_point,
                "save_actions": self.save_clicks,
                "file_drops": self.dropped_files,
                "picker_attempted": self.picker_attempted,
                "picker_completed": self.picker_completed,
                "picker_selected": self.picker_selected,
                "screenshot": self.screenshot_state,
            },
            "synthetic": {
                "keyboard_saves": self.logic.keyboard_saves,
                "mouse_saves": self.logic.mouse_saves,
                "touch_saves": self.logic.touch_saves,
                "file_drops": self.logic.file_drops,
                "accesskit_updates": self.logic.accesskit_updates,
                "accesskit_max_nodes": self.logic.accesskit_max_nodes,
                "accesskit_named_save": self.logic.accesskit_named_save,
                "accesskit_named_output": self.logic.accesskit_named_output,
            },
            "resources": {
                "update_cpu_us": distribution(&self.update_times_us),
                "visible_frame_interval_ms": distribution(&self.visible_intervals_ms),
                "hidden_frame_interval_ms": distribution(&self.hidden_intervals_ms),
                "rss_kib": rss_kib,
                "peak_rss_kib": peak_rss_kib,
            }
        });
        match serde_json::to_vec_pretty(&report) {
            Ok(bytes) => {
                if let Err(error) = std::fs::write(path, bytes) {
                    eprintln!("Could not write the spike report: {error}");
                }
            }
            Err(error) => eprintln!("Could not encode the spike report: {error}"),
        }
    }
}

impl eframe::App for SpikeApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.frames == 0 {
            self.window = probe_window(frame);
            self.embedded_parent = embedded_parent(frame).is_ok();
        }
        self.track_frame(ctx);
        self.native_automation(ctx, frame);
        self.receive_screenshot();
        let repaint_delay = if self.was_visible { 33 } else { 250 };
        ctx.request_repaint_after(Duration::from_millis(repaint_delay));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let update_started = Instant::now();
        self.draw(ui, Some(frame));
        self.update_times_us
            .push(update_started.elapsed().as_secs_f64() * 1_000_000.0);
    }
}

impl Drop for SpikeApp {
    fn drop(&mut self) {
        self.write_report();
    }
}

fn chain_card(ui: &mut egui::Ui, title: &str, subtitle: &str, level: f32) {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, PANEL_ALT))
        .corner_radius(CornerRadius::same(8))
        .show(ui, |ui| {
            ui.set_min_size(Vec2::new(190.0, 205.0));
            ui.add_space(8.0);
            ui.label(RichText::new(title).size(17.0).color(TEXT));
            ui.label(RichText::new(subtitle).size(12.0).color(MUTED));
            ui.add_space(18.0);
            meter(ui, level);
            ui.add_space(12.0);
            ui.label(
                RichText::new("READY")
                    .small()
                    .color(ACCENT)
                    .family(mono_family()),
            );
        });
}

fn meter(ui: &mut egui::Ui, level: f32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(150.0, 74.0), Sense::hover());
    let painter = ui.painter_at(rect);
    for index in 0..12 {
        let enabled = (index as f32) < level * 12.0;
        let color = if !enabled {
            PANEL_ALT
        } else if index > 8 {
            Color32::from_rgb(210, 80, 65)
        } else {
            ACCENT
        };
        let segment = Rect::from_min_size(
            Pos2::new(rect.left() + index as f32 * 12.5, rect.top()),
            Vec2::new(8.0, 74.0),
        );
        painter.rect_filled(segment, 2.0, color);
    }
    response.widget_info(|| {
        WidgetInfo::labeled(
            Role::ProgressIndicator,
            true,
            format!("Signal level {:.0} percent", level * 100.0),
        )
    });
}

fn knob(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(116.0, 128.0), Sense::click_and_drag());
    if response.dragged() {
        let span = range.end() - range.start();
        *value =
            (*value - response.drag_delta().y * span * 0.003).clamp(*range.start(), *range.end());
    }
    if response.double_clicked() {
        *value = default;
    }
    if response.has_focus() {
        let step = (range.end() - range.start()) * 0.01;
        if ui.input(|input| input.key_pressed(Key::ArrowUp) || input.key_pressed(Key::ArrowRight)) {
            *value = (*value + step).min(*range.end());
        }
        if ui.input(|input| input.key_pressed(Key::ArrowDown) || input.key_pressed(Key::ArrowLeft))
        {
            *value = (*value - step).max(*range.start());
        }
    }

    let center = rect.center() - Vec2::new(0.0, 13.0);
    let normalized = (*value - range.start()) / (range.end() - range.start());
    let angle = std::f32::consts::PI * (0.75 + normalized * 1.5);
    let needle = center + Vec2::new(angle.cos(), angle.sin()) * 20.0;
    let painter = ui.painter_at(rect);
    painter.circle_filled(center, 30.0, Color32::from_rgb(72, 72, 76));
    painter.circle_stroke(
        center,
        30.0,
        Stroke::new(2.0, Color32::from_rgb(12, 12, 14)),
    );
    painter.line_segment([center, needle], Stroke::new(2.0, ACCENT));
    painter.circle_filled(center, 4.0, PANEL_ALT);
    painter.text(
        rect.center_bottom() - Vec2::new(0.0, 4.0),
        Align2::CENTER_BOTTOM,
        label,
        FontId::proportional(13.0),
        TEXT,
    );
    response
        .widget_info(|| WidgetInfo::labeled(Role::Slider, true, format!("{label}: {value:.1} dB")));
}

fn mono_family() -> FontFamily {
    FontFamily::Name("ronie-mono".into())
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        "arimo".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/Arimo-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "roboto-mono".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/RobotoMono-Regular.ttf"
        ))),
    );
    fonts
        .families
        .insert(FontFamily::Proportional, vec!["arimo".to_owned()]);
    fonts
        .families
        .insert(FontFamily::Monospace, vec!["roboto-mono".to_owned()]);
    fonts
        .families
        .insert(mono_family(), vec!["roboto-mono".to_owned()]);
    ctx.set_fonts(fonts);
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

fn distribution(values: &[f64]) -> serde_json::Value {
    if values.is_empty() {
        return json!({"samples": 0});
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let p99 = sorted[(sorted.len() - 1) * 99 / 100];
    let maximum = sorted[sorted.len() - 1];
    json!({
        "samples": sorted.len(),
        "median": median,
        "p99": p99,
        "max": maximum,
    })
}

fn memory_kib() -> (Option<u64>, Option<u64>) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return (None, None);
    };
    let parse = |name: &str| {
        status.lines().find_map(|line| {
            let value = line.strip_prefix(name)?;
            value.split_whitespace().next()?.parse().ok()
        })
    };
    (parse("VmRSS:"), parse("VmHWM:"))
}

fn run_logic_probe() -> LogicProbe {
    let mut app = SpikeApp::new(Config::default(), LogicProbe::default());
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    install_fonts(&ctx);
    let mut probe = LogicProbe::default();

    run_logic_frame(&ctx, &mut app, &mut probe, Vec::new(), Vec::new());
    for _ in 0..7 {
        run_logic_frame(
            &ctx,
            &mut app,
            &mut probe,
            vec![key(Key::Tab, true)],
            Vec::new(),
        );
        run_logic_frame(
            &ctx,
            &mut app,
            &mut probe,
            vec![key(Key::Tab, false)],
            Vec::new(),
        );
    }
    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        vec![key(Key::Enter, true)],
        Vec::new(),
    );
    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        vec![key(Key::Enter, false)],
        Vec::new(),
    );
    probe.keyboard_saves = app.save_clicks;

    let pointer = Pos2::new(220.0, 31.0);
    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        vec![Event::PointerMoved(pointer)],
        Vec::new(),
    );
    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        vec![Event::PointerButton {
            pos: pointer,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::default(),
        }],
        Vec::new(),
    );
    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        vec![Event::PointerButton {
            pos: pointer,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::default(),
        }],
        Vec::new(),
    );
    probe.mouse_saves = app.save_clicks - probe.keyboard_saves;

    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        vec![
            touch(TouchPhase::Start, pointer),
            Event::PointerMoved(pointer),
            Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::default(),
            },
        ],
        Vec::new(),
    );
    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        vec![
            touch(TouchPhase::Move, pointer),
            Event::PointerMoved(pointer),
        ],
        Vec::new(),
    );
    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        vec![
            touch(TouchPhase::End, pointer),
            Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::default(),
            },
            Event::PointerGone,
        ],
        Vec::new(),
    );
    probe.touch_saves = app.save_clicks - probe.keyboard_saves - probe.mouse_saves;

    run_logic_frame(
        &ctx,
        &mut app,
        &mut probe,
        Vec::new(),
        vec![Arc::new(SyntheticDrop {
            path: PathBuf::from("spike-model.nam"),
        })],
    );
    probe.file_drops = app.dropped_files;
    probe
}

#[derive(Debug)]
struct SyntheticDrop {
    path: PathBuf,
}

impl egui::DroppedFile for SyntheticDrop {
    fn path(&self) -> &Path {
        &self.path
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.path).map_err(|error| error.to_string())
    }
}

fn run_logic_frame(
    ctx: &egui::Context,
    app: &mut SpikeApp,
    probe: &mut LogicProbe,
    events: Vec<Event>,
    dropped_files: Vec<DroppedFileHandle>,
) {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 578.0))),
        focused: true,
        events,
        dropped_files,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        app.track_frame(ui.ctx());
        app.draw(ui, None);
    });
    if let Some(update) = output.platform_output.accesskit_update.take() {
        probe.accesskit_updates += 1;
        probe.accesskit_max_nodes = probe.accesskit_max_nodes.max(update.nodes.len());
        let debug = format!("{update:?}");
        probe.accesskit_named_save |= debug.contains("Save preset");
        probe.accesskit_named_output |= debug.contains("Output:");
    }
    output.drop_without_applying_deltas();
}

fn key(key: Key, pressed: bool) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::default(),
    }
}

fn touch(phase: TouchPhase, pos: Pos2) -> Event {
    Event::Touch {
        device_id: TouchDeviceId(0),
        id: TouchId(0),
        phase,
        pos,
        force: Some(1.0),
    }
}

fn parse_config() -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--report" => config.report = Some(PathBuf::from(next_value(&mut args, "--report")?)),
            "--screenshot" => {
                config.screenshot = Some(PathBuf::from(next_value(&mut args, "--screenshot")?));
            }
            "--scale" => {
                let value = next_value(&mut args, "--scale")?;
                let scale = value
                    .parse::<f32>()
                    .map_err(|_| format!("invalid scale: {value}"))?;
                if !(0.5..=4.0).contains(&scale) {
                    return Err("scale must be between 0.5 and 4.0".to_owned());
                }
                config.scale = Some(scale);
            }
            "--selftest" => config.selftest = true,
            "--picker" => config.picker = true,
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(config)
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value after {flag}"))
}

fn main() -> eframe::Result<()> {
    let config = match parse_config() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let logic = run_logic_probe();
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Ronnie UI toolkit spike")
            .with_inner_size([1024.0, 578.0])
            .with_min_inner_size([640.0, 400.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Ronnie UI toolkit spike",
        options,
        Box::new(move |creation| {
            install_fonts(&creation.egui_ctx);
            creation.egui_ctx.enable_accesskit();
            if let Some(scale) = config.scale {
                creation.egui_ctx.set_zoom_factor(scale);
            }
            Ok(Box::new(SpikeApp::new(config, logic)))
        }),
    )
}
