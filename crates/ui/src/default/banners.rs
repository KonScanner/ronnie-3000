//! Prioritized standalone audio banners and window growth choreography.

use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
};

use eframe::egui;
use ronie_core::{PropertiesError, SettingsStore};

use crate::{
    backend::{ChainFault, ChainFaultKind},
    shell::Chrome,
};

const DISMISSED_BANNERS_KEY: &str = "t3k.dismissedBanners";
const IGNORE_MS: i64 = 24 * 60 * 60 * 1000;
/// Start of the dismissal identity of an audio chain fault.
const FAULT_BANNER_PREFIX: &str = "chain-fault:";
/// Shortest time between two reads of the preferences file for a change from another process.
const DISMISSED_READ_INTERVAL_MS: i64 = 5_000;
/// Longest tone title in a fault banner, which leaves room for the fixed words on one line.
const MAX_BANNER_TITLE_CHARS: usize = 40;
/// Name of a block that has no tone title to show.
const NO_TONE_TITLE: &str = "A block";
/// What one read of the preferences file finds: the text of the dismissals, if the file has it.
type SavedDismissals = Result<Option<String>, PropertiesError>;
/// Banner strip height in design pixels.
pub const BANNER_HEIGHT: i32 = 44;
/// Above this estimated delay in milliseconds, the delay banner suggests a smaller buffer.
const DELAY_WARNING_MS: f64 = 20.0;

/// Minimal standalone audio-device state consumed by banner rules.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDeviceState {
    /// Microphone privacy is denied by the operating system.
    pub mic_denied: bool,
    /// Whether the selected device is open.
    pub device_open: bool,
    /// Selected input name, or empty when none is selected.
    pub input_device: String,
    /// Selected output name, or empty when none is selected.
    pub output_device: String,
    /// Whether at least one capture channel is active.
    pub has_active_input: bool,
    /// Whether dry input monitoring is enabled.
    pub hear_yourself: bool,
    /// Whether the current route can feed speakers back into a microphone.
    pub feedback_risk: bool,
    /// Whether an ASIO driver is available.
    pub asio_available: bool,
    /// Current backend type name.
    pub current_type: String,
    /// Current callback size.
    pub buffer_size: u32,
    /// Selectable callback sizes.
    pub buffer_sizes: Vec<u32>,
    /// Current stream sample rate.
    pub sample_rate: f64,
    /// The estimated delay from the input to the output in milliseconds, while a stream runs.
    pub delay_ms: Option<f64>,
}

impl AudioDeviceState {
    /// The estimated delay in milliseconds. With no estimate, the input and the output buffer
    /// are the least delay that the stream has.
    #[must_use]
    pub fn estimated_delay_ms(&self) -> f64 {
        self.delay_ms.unwrap_or_else(|| {
            let rate = if self.sample_rate > 0.0 {
                self.sample_rate
            } else {
                48_000.0
            };
            2.0 * f64::from(self.buffer_size) * 1_000.0 / rate
        })
    }

    /// Parses the neutral scenario representation without accepting partial state.
    #[must_use]
    pub fn from_ui_value(value: &serde_json::Value) -> Option<Self> {
        let object = value.as_object()?;
        let string = |key: &str| {
            object
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let boolean = |key: &str| {
            object
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        let buffer_sizes = object
            .get("bufferSizes")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_u64)
                    .filter_map(|value| u32::try_from(value).ok())
                    .collect()
            })
            .unwrap_or_default();
        let has_active_input = object
            .get("inputChannels")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|channels| {
                channels
                    .iter()
                    .any(|channel| channel["active"].as_bool().unwrap_or(false))
            });
        Some(Self {
            mic_denied: string("micPermission") == "denied",
            device_open: boolean("deviceOpen"),
            input_device: string("inputDevice"),
            output_device: string("outputDevice"),
            has_active_input,
            hear_yourself: boolean("hearYourself"),
            feedback_risk: boolean("feedbackRisk"),
            asio_available: boolean("asioAvailable"),
            current_type: string("currentType"),
            buffer_size: u32::try_from(object.get("bufferSize")?.as_u64()?).ok()?,
            buffer_sizes,
            sample_rate: object.get("sampleRate")?.as_f64()?,
            delay_ms: object.get("delayMs").and_then(serde_json::Value::as_f64),
        })
    }
}

/// Banner severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerVariant {
    /// Audio cannot be captured or heard.
    Error,
    /// Monitoring can damage hearing or equipment.
    Warning,
    /// Optional setup improvement.
    Info,
}

/// Action dispatched by a banner button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerAction {
    /// Open standalone audio settings.
    OpenSettings,
    /// Select the available ASIO backend.
    SwitchToAsio,
    /// Open the operating system's microphone privacy page.
    OpenMicSettings,
    /// No action exists, so the strip draws no action button.
    None,
}

/// Fully resolved active banner.
#[derive(Debug, Clone, PartialEq)]
pub struct BannerSpec {
    /// Stable rule identity used by dismissal preferences.
    pub id: String,
    /// Leading strong text.
    pub headline: String,
    /// Remaining plain explanation.
    pub body: String,
    /// Severity.
    pub variant: BannerVariant,
    /// Action button label.
    pub action_label: String,
    /// Action button behavior.
    pub action: BannerAction,
    /// Whether the banner can be hidden for 24 hours.
    pub dismissable: bool,
}

/// The preferences file behind the dismissals, and the one way that this module reads it.
///
/// A store reads the file when it starts and when the user dismisses a banner. For a change from
/// another process, a thread reads it at most one time in each [`DISMISSED_READ_INTERVAL_MS`], so
/// that no frame does file work.
struct DismissalFile {
    settings: SettingsStore,
    read: Arc<dyn Fn() -> SavedDismissals + Send + Sync>,
    /// Frame time of the latest read that started.
    last_read_ms: Option<i64>,
    /// The answer of the read on another thread.
    pending: Option<mpsc::Receiver<SavedDismissals>>,
}

impl DismissalFile {
    fn new(settings: SettingsStore) -> Self {
        let source = settings.clone();
        Self {
            settings,
            read: Arc::new(move || {
                let properties = source.read()?;
                Ok(properties.get(DISMISSED_BANNERS_KEY).map(str::to_owned))
            }),
            last_read_ms: None,
            pending: None,
        }
    }

    /// Takes the answer of a read that ended, or starts a read when one is due.
    fn poll(&mut self, now_ms: i64) -> Option<SavedDismissals> {
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(saved) => {
                    self.pending = None;
                    return Some(saved);
                }
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
            }
        }
        // The frame clock can start again, so the distance in either direction counts.
        let last = *self.last_read_ms.get_or_insert(now_ms);
        if now_ms.abs_diff(last) >= DISMISSED_READ_INTERVAL_MS.unsigned_abs() {
            self.last_read_ms = Some(now_ms);
            self.start_read();
        }
        None
    }

    fn start_read(&mut self) {
        let (answer, pending) = mpsc::sync_channel(1);
        let read = Arc::clone(&self.read);
        let started = std::thread::Builder::new()
            .name("ronie-banner-read".to_owned())
            .spawn(move || {
                let _ = answer.send(read());
            });
        // With no thread, the next read that is due tries again.
        self.pending = started.is_ok().then_some(pending);
    }

    /// Reads the file on this thread, and drops a read from before this one.
    fn read_now(&mut self, now_ms: i64) -> SavedDismissals {
        self.pending = None;
        self.last_read_ms = Some(now_ms);
        (self.read)()
    }
}

/// Priority evaluator with 24-hour per-rule dismissal memory.
pub struct BannerStore {
    active: Option<BannerSpec>,
    last_state: Option<AudioDeviceState>,
    dismissed: BTreeMap<String, i64>,
    file: Option<DismissalFile>,
    announcement: Option<String>,
    chain_faults: Vec<ChainFault>,
}

impl Default for BannerStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BannerStore {
    /// Creates an in-memory evaluator.
    #[must_use]
    pub fn new() -> Self {
        Self::from_settings(None)
    }

    /// Creates an evaluator backed by shared UI preferences.
    #[must_use]
    pub fn with_settings(settings: SettingsStore) -> Self {
        Self::from_settings(Some(settings))
    }

    fn from_settings(settings: Option<SettingsStore>) -> Self {
        let file = settings.map(DismissalFile::new);
        let dismissed = file
            .as_ref()
            .and_then(|file| (file.read)().ok())
            .and_then(parse_dismissals);
        Self {
            active: None,
            last_state: None,
            dismissed: dismissed.unwrap_or_default(),
            file,
            announcement: None,
            chain_faults: Vec::new(),
        }
    }

    /// Re-evaluates all rules in priority order.
    pub fn evaluate(&mut self, state: Option<&AudioDeviceState>, now_ms: i64) {
        self.last_state = state.cloned();
        if let Some(saved) = self.file.as_mut().and_then(|file| file.poll(now_ms)) {
            self.apply_saved(saved);
        }
        let device = state.and_then(|state| {
            [
                "mic-denied",
                "no-input",
                "no-output",
                "input-muted",
                "feedback-risk",
                "asio-nudge",
                "buffer-latency",
                "rate-not-48k",
            ]
            .into_iter()
            .find(|id| rule_matches(id, state))
            .filter(|id| {
                let dismissable = matches!(
                    id,
                    &"feedback-risk" | &"asio-nudge" | &"buffer-latency" | &"rate-not-48k"
                );
                !dismissable || self.dismissed.get(*id).is_none_or(|until| *until <= now_ms)
            })
            .and_then(|id| make_spec(id, state))
        });
        // With no audio path, a block fault means nothing yet. Each other device rule is a note
        // about a chain that runs, so a fault goes first.
        let fault = self.chain_faults.iter().map(make_fault_spec).find(|spec| {
            self.dismissed
                .get(&spec.id)
                .is_none_or(|until| *until <= now_ms)
        });
        let next = match (device, fault) {
            (Some(device), _) if device.variant == BannerVariant::Error => Some(device),
            (_, Some(fault)) => Some(fault),
            (device, None) => device,
        };
        if next != self.active {
            self.announcement = next
                .as_ref()
                .map(|spec| format!("{} {}", spec.headline, spec.body));
            self.active = next;
        }
    }

    /// Replaces the audio chain faults that the next evaluation reads.
    pub fn set_chain_faults(&mut self, faults: Vec<ChainFault>) {
        self.chain_faults = faults;
    }

    /// Current active banner.
    #[must_use]
    pub const fn active(&self) -> Option<&BannerSpec> {
        self.active.as_ref()
    }

    /// Takes the latest changed-banner announcement.
    pub fn take_announcement(&mut self) -> Option<String> {
        self.announcement.take()
    }

    /// Dismisses a dismissible rule for 24 hours.
    ///
    /// # Errors
    /// Returns a preferences error without hiding the banner when saving fails.
    pub fn dismiss(&mut self, id: &str, now_ms: i64) -> Result<bool, PropertiesError> {
        if self
            .active
            .as_ref()
            .is_none_or(|banner| banner.id != id || !banner.dismissable)
        {
            return Ok(false);
        }
        // The write replaces the whole value, so it must start from what the file has now.
        if let Some(saved) = self.file.as_mut().map(|file| file.read_now(now_ms)) {
            self.apply_saved(saved);
        }
        let mut dismissed = self.dismissed.clone();
        // Fault identities have no fixed set, so an ended dismissal must not stay in the file.
        dismissed.retain(|_, until| *until > now_ms);
        dismissed.insert(id.to_owned(), now_ms.saturating_add(IGNORE_MS));
        if let Some(file) = &self.file {
            let payload = serde_json::to_string(&dismissed).unwrap_or_else(|_| "{}".to_owned());
            file.settings.merge_key(DISMISSED_BANNERS_KEY, &payload)?;
        }
        self.dismissed = dismissed;
        let state = self.last_state.clone();
        self.evaluate(state.as_ref(), now_ms);
        Ok(true)
    }

    /// Keeps the dismissals that this store has when a read fails or finds a damaged value.
    fn apply_saved(&mut self, saved: SavedDismissals) {
        if let Ok(saved) = saved
            && let Some(dismissed) = parse_dismissals(saved)
        {
            self.dismissed = dismissed;
        }
    }

    /// Whether a read on another thread has not reported.
    #[cfg(test)]
    fn read_in_progress(&self) -> bool {
        self.file
            .as_ref()
            .is_some_and(|file| file.pending.is_some())
    }
}

/// The dismissals in a saved value. No value means no dismissals, and a damaged value gives `None`.
fn parse_dismissals(saved: Option<String>) -> Option<BTreeMap<String, i64>> {
    saved.map_or_else(
        || Some(BTreeMap::new()),
        |payload| serde_json::from_str(&payload).ok(),
    )
}

fn rule_matches(id: &str, state: &AudioDeviceState) -> bool {
    match id {
        "mic-denied" => state.mic_denied,
        "no-input" => {
            (!state.input_device.is_empty() || !state.output_device.is_empty())
                && (!state.device_open || state.input_device.is_empty() || !state.has_active_input)
        }
        "no-output" => state.device_open && state.output_device.is_empty(),
        "input-muted" => state.device_open && !state.hear_yourself && state.has_active_input,
        "feedback-risk" => state.feedback_risk && state.hear_yourself,
        "asio-nudge" => state.asio_available && state.current_type != "ASIO",
        "buffer-latency" => {
            state.estimated_delay_ms() > DELAY_WARNING_MS
                && state
                    .buffer_sizes
                    .iter()
                    .any(|size| *size < state.buffer_size)
        }
        "rate-not-48k" => {
            state.device_open
                && state.sample_rate > 0.0
                && state.sample_rate.round() as i32 != 48_000
        }
        _ => false,
    }
}

fn make_spec(id: &str, state: &AudioDeviceState) -> Option<BannerSpec> {
    let (headline, body, variant, action_label, action, dismissable) = match id {
        "mic-denied" => (
            "Microphone access is off.".to_owned(),
            " Ronnie can’t hear your instrument until you allow it in your privacy settings, then relaunch.".to_owned(),
            BannerVariant::Error,
            "Allow Access",
            BannerAction::OpenMicSettings,
            false,
        ),
        "no-input" => (
            "No audio input.".to_owned(),
            if state.device_open {
                " No input device is selected. Ronnie can’t hear your instrument.".to_owned()
            } else {
                " Your audio device was disconnected or couldn’t be opened. Ronnie can’t hear your instrument.".to_owned()
            },
            BannerVariant::Error,
            "Open Settings",
            BannerAction::OpenSettings,
            false,
        ),
        "no-output" => (
            "No audio output.".to_owned(),
            " No output device is selected, so you won’t hear Ronnie.".to_owned(),
            BannerVariant::Error,
            "Open Settings",
            BannerAction::OpenSettings,
            false,
        ),
        "input-muted" => (
            if state.feedback_risk {
                "Input muted to prevent feedback.".to_owned()
            } else {
                "Input is muted.".to_owned()
            },
            if state.feedback_risk {
                " You’re using a microphone with speakers. Plug in headphones or an interface, then turn “Hear Yourself” on.".to_owned()
            } else {
                " Turn “Hear Yourself” on so Ronnie can hear your instrument.".to_owned()
            },
            BannerVariant::Warning,
            "Open Settings",
            BannerAction::OpenSettings,
            false,
        ),
        "feedback-risk" => (
            "Feedback risk.".to_owned(),
            " Your mic can hear your speakers. Use headphones, or turn “Hear Yourself” off.".to_owned(),
            BannerVariant::Warning,
            "Open Settings",
            BannerAction::OpenSettings,
            true,
        ),
        "asio-nudge" => (
            "Lower latency available.".to_owned(),
            " Your interface has an ASIO driver, which is faster than Windows Audio.".to_owned(),
            BannerVariant::Info,
            "Switch to ASIO",
            BannerAction::SwitchToAsio,
            true,
        ),
        "buffer-latency" => {
            let milliseconds = state.estimated_delay_ms();
            (
                "Noticeable delay?".to_owned(),
                format!(" The delay is about {milliseconds:.0} ms with a buffer of {} samples. A smaller buffer will feel more responsive.", state.buffer_size),
                BannerVariant::Info,
                "Open Settings",
                BannerAction::OpenSettings,
                true,
            )
        }
        "rate-not-48k" => {
            let rate = state.sample_rate / 1000.0;
            let rate_text = if (state.sample_rate % 1000.0).abs() < f64::EPSILON {
                format!("{rate:.0}")
            } else {
                format!("{rate:.1}")
            };
            (
                format!("Running at {rate_text} kHz."),
                " Ronnie runs lightest at 48 kHz. Any rate works fine, this one just uses a bit more CPU.".to_owned(),
                BannerVariant::Info,
                "Open Settings",
                BannerAction::OpenSettings,
                true,
            )
        }
        _ => return None,
    };
    Some(BannerSpec {
        id: id.to_owned(),
        headline,
        body,
        variant,
        action_label: action_label.to_owned(),
        action,
        dismissable,
    })
}

/// Builds the banner of one audio chain fault. All text is fixed except the tone title.
fn make_fault_spec(fault: &ChainFault) -> BannerSpec {
    let title = fault.title.as_deref();
    let (headline, body) = match fault.kind {
        ChainFaultKind::ModelRejected => (
            format!("{} cannot run.", block_name(title)),
            " Ronnie bypasses this block.",
        ),
        ChainFaultKind::MissingModel => (
            format!("{} has no model data.", block_name(title)),
            " Ronnie bypasses this block. Load the tone again.",
        ),
        ChainFaultKind::NonFiniteOutput => (
            if title.is_some() {
                format!("{} made invalid audio.", block_name(title))
            } else {
                "The audio chain made invalid audio.".to_owned()
            },
            " Ronnie replaces it with silence.",
        ),
        ChainFaultKind::ChainNotPrepared => (
            "The audio chain cannot update.".to_owned(),
            " Ronnie keeps the last chain that worked.",
        ),
        ChainFaultKind::ProcessingFailed => (
            format!("{} stopped working.", block_name(title)),
            " Ronnie bypasses this block.",
        ),
    };
    BannerSpec {
        id: format!("{FAULT_BANNER_PREFIX}{}", fault.identity),
        headline,
        body: body.to_owned(),
        variant: BannerVariant::Error,
        action_label: String::new(),
        action: BannerAction::None,
        dismissable: true,
    }
}

/// The tone title for one line of the strip, because a title can come from a server.
fn block_name(title: Option<&str>) -> String {
    let name = one_line_text(title.unwrap_or_default());
    if name.is_empty() {
        return NO_TONE_TITLE.to_owned();
    }
    if name.chars().count() <= MAX_BANNER_TITLE_CHARS {
        return name;
    }
    let shortened = name
        .chars()
        .take(MAX_BANNER_TITLE_CHARS)
        .collect::<String>();
    format!("{}…", shortened.trim_end())
}

/// Text from outside this program, on one line and with no character that hides other text.
pub(super) fn one_line_text(text: &str) -> String {
    let visible = text
        .chars()
        .filter(|character| !is_invisible_format(*character))
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    visible.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Characters with no glyph that can reorder or hide the fixed words after a title.
const fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

/// User action from the native banner strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BannerViewAction {
    /// Run the rule's main action.
    Action(BannerAction),
    /// Dismiss one rule for 24 hours.
    Dismiss(String),
}

/// Renders one banner strip.
pub fn show(ui: &mut egui::Ui, banner: &BannerSpec) -> Option<BannerViewAction> {
    let root = ui.available_rect_before_wrap();
    let painter = ui.painter_at(root);
    painter.line_segment(
        [
            egui::pos2(root.left(), root.bottom() - 1.0),
            egui::pos2(root.right(), root.bottom() - 1.0),
        ],
        egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
    );
    let center_y = (root.bottom() - 1.0 - root.top()) / 2.0 + root.top();
    let icon_color = if banner.variant == BannerVariant::Error {
        crate::theme::palette::FIRE
    } else {
        crate::theme::palette::BLAZE
    };
    let icon_center = egui::pos2(root.left() + 32.0, center_y);
    painter.circle_stroke(icon_center, 7.2, egui::Stroke::new(1.6, icon_color));
    painter.text(
        icon_center,
        egui::Align2::CENTER_CENTER,
        "!",
        egui::FontId::new(10.0, egui::FontFamily::Name("arimo-bold".into())),
        icon_color,
    );

    let action_font = egui::FontId::new(11.5, egui::FontFamily::Name("arimo-bold".into()));
    let ignore_width = if banner.dismissable {
        text_width(ui, "Ignore", action_font.clone()).round() + 8.0
    } else {
        0.0
    };
    let action_right =
        root.right() - 24.0 - ignore_width - if banner.dismissable { 10.0 } else { 0.0 };
    let button_center_y = center_y + 0.5;
    let mut result = None;
    let mut text_right = action_right;
    if banner.action != BannerAction::None {
        let action_width = text_width(ui, &banner.action_label, action_font.clone()).round() + 24.0;
        let action_rect = egui::Rect::from_center_size(
            egui::pos2(action_right - action_width / 2.0, button_center_y),
            egui::vec2(action_width, 22.0),
        );
        let action = banner_button(
            ui,
            action_rect,
            &banner.action_label,
            action_font.clone(),
            true,
        );
        result = action
            .clicked()
            .then_some(BannerViewAction::Action(banner.action));
        text_right = action_rect.left() - 10.0;
    }
    if banner.dismissable {
        let ignore_rect = egui::Rect::from_center_size(
            egui::pos2(root.right() - 24.0 - ignore_width / 2.0, button_center_y),
            egui::vec2(ignore_width, 20.0),
        );
        if banner_button(ui, ignore_rect, "Ignore", action_font, false).clicked() {
            result = Some(BannerViewAction::Dismiss(banner.id.clone()));
        }
    }

    let text_x = root.left() + 50.0;
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = (text_right - text_x).max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.append(
        &banner.headline,
        0.0,
        egui::text::TextFormat {
            font_id: egui::FontId::new(12.5, egui::FontFamily::Name("arimo-bold".into())),
            color: egui::Color32::WHITE,
            line_height: Some(17.5),
            ..Default::default()
        },
    );
    job.append(
        &banner.body,
        0.0,
        egui::text::TextFormat {
            font_id: egui::FontId::proportional(12.5),
            color: egui::Color32::WHITE,
            line_height: Some(17.5),
            ..Default::default()
        },
    );
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    painter.galley(
        egui::pos2(text_x, root.top() + 12.75),
        galley,
        egui::Color32::WHITE,
    );
    result
}

fn text_width(ui: &egui::Ui, text: &str, font: egui::FontId) -> f32 {
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE)
            .size()
            .x
    })
}

fn banner_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    label: &str,
    font: egui::FontId,
    primary: bool,
) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(label), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.to_owned()));
    let color = if primary {
        egui::Color32::WHITE
    } else {
        crate::theme::palette::TEXT_SECONDARY
    };
    if primary {
        ui.painter_at(rect).rect_stroke(
            rect,
            7.0,
            egui::Stroke::new(1.0, egui::Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter_at(rect).text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        color,
    );
    response
}

/// Ordered native window and banner operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStep {
    /// Resize before showing or after hiding the strip.
    Resize(i32, i32),
    /// Make the banner visible.
    ShowBanner,
    /// Hide the banner before shrinking.
    HideBanner,
}

/// Plans snapped transitions so no content is compressed or exposed early.
#[must_use]
pub fn banner_window_transition(before: Chrome, after: Chrome, scale: f64) -> Vec<WindowStep> {
    let (width, height) = after.window_size(scale);
    match (before.banner, after.banner) {
        (false, true) => vec![WindowStep::Resize(width, height), WindowStep::ShowBanner],
        (true, false) => vec![WindowStep::HideBanner, WindowStep::Resize(width, height)],
        _ if before != after => vec![WindowStep::Resize(width, height)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    use super::*;

    /// A device state that shows the dismissible latency banner.
    fn buffer_banner_state() -> AudioDeviceState {
        AudioDeviceState {
            mic_denied: false,
            device_open: true,
            input_device: "In".to_owned(),
            output_device: "Out".to_owned(),
            has_active_input: true,
            hear_yourself: true,
            feedback_risk: false,
            asio_available: false,
            current_type: "ALSA".to_owned(),
            buffer_size: 1024,
            buffer_sizes: vec![256, 1024],
            sample_rate: 48_000.0,
            delay_ms: None,
        }
    }

    #[test]
    fn the_delay_banner_quotes_the_estimate_and_shows_above_20_ms() {
        let mut state = buffer_banner_state();
        state.delay_ms = Some(27.4);
        let mut store = BannerStore::new();
        store.evaluate(Some(&state), 0);
        let banner = store.active().expect("banner");
        assert_eq!(banner.id, "buffer-latency");
        assert!(banner.body.contains("about 27 ms"), "{}", banner.body);

        state.delay_ms = Some(19.0);
        store.evaluate(Some(&state), 0);
        assert!(store.active().is_none(), "19 ms is fine");

        // With no estimate, the two buffers are the delay: 2 x 1024 frames at 48 kHz.
        state.delay_ms = None;
        assert!((state.estimated_delay_ms() - 2048.0 / 48.0).abs() < 1e-9);

        // No smaller buffer exists, so the banner has no advice.
        state.delay_ms = Some(40.0);
        state.buffer_size = 256;
        store.evaluate(Some(&state), 0);
        assert!(store.active().is_none());
    }

    fn store_in(directory: &tempfile::TempDir) -> (BannerStore, SettingsStore) {
        let settings = SettingsStore::new(directory.path().join("ui-preferences.settings"));
        (BannerStore::with_settings(settings.clone()), settings)
    }

    /// Replaces the read with one that also counts. A count starts after this call.
    fn count_reads(store: &mut BannerStore) -> Arc<AtomicUsize> {
        let count = Arc::new(AtomicUsize::new(0));
        let file = store.file.as_mut().expect("a store with a file");
        let read = Arc::clone(&file.read);
        let counter = Arc::clone(&count);
        file.read = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            read()
        });
        count
    }

    /// Waits until the read on another thread has reported, and gives its answer to the store.
    fn wait_for_read(store: &mut BannerStore, state: &AudioDeviceState, now_ms: i64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while store.read_in_progress() {
            assert!(Instant::now() < deadline, "a read did not end");
            std::thread::sleep(Duration::from_millis(1));
            store.evaluate(Some(state), now_ms);
        }
    }

    #[test]
    fn many_frames_read_the_preferences_file_at_most_one_time_each_few_seconds() {
        const FRAMES: i64 = 6_000;
        const FRAME_MS: i64 = 16;
        let directory = tempfile::tempdir().expect("tempdir");
        let (mut store, _settings) = store_in(&directory);
        let reads = count_reads(&mut store);
        let state = buffer_banner_state();

        for frame in 0..FRAMES {
            store.evaluate(Some(&state), frame * FRAME_MS);
        }
        wait_for_read(&mut store, &state, FRAMES * FRAME_MS);

        let most = (FRAMES * FRAME_MS / DISMISSED_READ_INTERVAL_MS + 1) as usize;
        let count = reads.load(Ordering::SeqCst);
        assert!(count <= most, "{count} reads in {FRAMES} frames");
        assert!(count >= 1, "a dismissal in another process must arrive");
    }

    #[test]
    fn a_frame_does_not_wait_for_a_read_of_the_preferences_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (mut store, _settings) = store_in(&directory);
        let state = buffer_banner_state();
        store.evaluate(Some(&state), 0);
        let (release, held) = mpsc::channel::<()>();
        let held = Mutex::new(held);
        let (started_tx, started) = mpsc::channel();
        let finished = Arc::new(AtomicUsize::new(0));
        let ended = Arc::clone(&finished);
        store.file.as_mut().expect("file").read = Arc::new(move || {
            let _ = started_tx.send(());
            // A frame that waits for this read meets this limit, and the test then fails.
            let _ = held
                .lock()
                .expect("gate")
                .recv_timeout(Duration::from_secs(5));
            ended.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        });

        store.evaluate(Some(&state), DISMISSED_READ_INTERVAL_MS);

        assert_eq!(finished.load(Ordering::SeqCst), 0, "the frame waited");
        started
            .recv_timeout(Duration::from_secs(10))
            .expect("the read starts on another thread");
        for frame in 1..100 {
            store.evaluate(Some(&state), DISMISSED_READ_INTERVAL_MS + frame);
        }
        assert_eq!(finished.load(Ordering::SeqCst), 0, "a frame waited");
        drop(release);
        wait_for_read(&mut store, &state, DISMISSED_READ_INTERVAL_MS + 100);
    }

    #[test]
    fn a_read_from_before_a_dismissal_does_not_bring_the_banner_back() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (mut store, settings) = store_in(&directory);
        let state = buffer_banner_state();
        store.evaluate(Some(&state), 0);
        assert_eq!(store.active().expect("banner").id, "buffer-latency");

        // The first read here saw the file before the dismissal, and it reports only after it.
        let (release, held) = mpsc::channel::<()>();
        let held = Mutex::new(held);
        let (started_tx, started) = mpsc::channel();
        let file = store.file.as_mut().expect("file");
        let read = Arc::clone(&file.read);
        let calls = AtomicUsize::new(0);
        file.read = Arc::new(move || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let _ = started_tx.send(());
                let _ = held
                    .lock()
                    .expect("gate")
                    .recv_timeout(Duration::from_secs(10));
                return Ok(None);
            }
            read()
        });
        store.evaluate(Some(&state), DISMISSED_READ_INTERVAL_MS);
        started
            .recv_timeout(Duration::from_secs(10))
            .expect("the read starts");

        assert!(
            store
                .dismiss("buffer-latency", DISMISSED_READ_INTERVAL_MS)
                .expect("dismiss")
        );
        assert!(store.active().is_none(), "a dismissal shows at once");
        drop(release);
        std::thread::sleep(Duration::from_millis(50));

        for frame in 1..50 {
            store.evaluate(Some(&state), DISMISSED_READ_INTERVAL_MS + frame);
            assert!(store.active().is_none(), "the old read came back");
        }
        assert!(
            settings
                .read()
                .expect("preferences")
                .get(DISMISSED_BANNERS_KEY)
                .is_some(),
            "the dismissal persists"
        );
    }

    #[test]
    fn a_dismissal_keeps_a_dismissal_that_another_process_just_saved() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (mut store, settings) = store_in(&directory);
        let state = buffer_banner_state();
        store.evaluate(Some(&state), 1_000);

        // The other process saves this in the time between two reads of this store.
        let until = 1_000 + IGNORE_MS;
        settings
            .merge_key(
                DISMISSED_BANNERS_KEY,
                &format!(r#"{{"rate-not-48k":{until}}}"#),
            )
            .expect("other process");
        assert!(store.dismiss("buffer-latency", 1_000).expect("dismiss"));

        let saved = settings.read().expect("preferences");
        let saved: BTreeMap<String, i64> =
            serde_json::from_str(saved.get(DISMISSED_BANNERS_KEY).expect("saved")).expect("JSON");
        assert_eq!(saved.get("rate-not-48k"), Some(&until), "{saved:?}");
        assert!(saved.contains_key("buffer-latency"), "{saved:?}");
    }

    #[test]
    fn a_dismissal_that_cannot_be_saved_keeps_the_banner() {
        let directory = tempfile::tempdir().expect("tempdir");
        // A directory at this path makes each read and each write fail.
        let settings = SettingsStore::new(directory.path());
        let mut store = BannerStore::with_settings(settings);
        let state = buffer_banner_state();
        store.evaluate(Some(&state), 1_000);

        assert!(store.dismiss("buffer-latency", 1_000).is_err());
        store.evaluate(Some(&state), 1_016);

        assert_eq!(store.active().expect("banner").id, "buffer-latency");
    }
}
