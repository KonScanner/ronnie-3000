//! Standalone audio-device state, settings controls, and channel selection.

use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

use eframe::egui;
use ronie_core::InputMode;

use crate::backend::{AudioDeviceConfig, DeviceAction, HostControl, UiAction, UiBackend, UiError};

use super::{AudioDeviceState, BannerStore, forms};

/// The width of the labels at the left of the page.
const ROW_LABEL_WIDTH: f32 = 72.0;

/// The width of the rate field, which holds "192000 Hz" and its chevron.
const RATE_FIELD_WIDTH: f32 = 122.0;

/// The level of a meter for silence, in dB.
const LEVEL_FLOOR_DB: f32 = -60.0;

/// A channel at or above this level has a signal. This is above the noise of an open input.
const SIGNAL_FLOOR_DB: f32 = -50.0;

/// Seconds between two looks at the devices while the page is visible.
const REFRESH_SECONDS: f64 = 5.0;
/// The heading of the section of the saved key.
pub const KEY_SECTION: &str = "Community";

/// One row of a device list. The identity selects, and the label only tells the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceOption {
    /// Stable identity of the row.
    pub id: String,
    /// The text that the user sees. Two rows can have one label.
    pub label: String,
}

impl AudioDeviceOption {
    fn named(name: &str) -> Self {
        Self {
            id: name.to_owned(),
            label: name.to_owned(),
        }
    }
}

/// One discoverable audio input channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInputChannel {
    /// Zero-based hardware channel index.
    pub index: usize,
    /// Display name.
    pub name: String,
    /// Whether this channel is selected.
    pub active: bool,
}

/// One discoverable MIDI input and its enablement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiInputDevice {
    /// Stable operating-system identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether packets from this input are accepted.
    pub enabled: bool,
}

/// Complete audio and MIDI state consumed by standalone settings.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemDeviceState {
    /// State shared with global banner evaluation.
    pub banner: AudioDeviceState,
    /// Available backend display names.
    pub device_types: Vec<String>,
    /// Available input rows.
    pub input_devices: Vec<AudioDeviceOption>,
    /// Available output rows.
    pub output_devices: Vec<AudioDeviceOption>,
    /// Identity of the selected input row, or empty when none is selected.
    pub selected_input: String,
    /// Identity of the selected output row, or empty when none is selected.
    pub selected_output: String,
    /// Available input channels.
    pub input_channels: Vec<AudioInputChannel>,
    /// Available MIDI inputs.
    pub midi_inputs: Vec<MidiInputDevice>,
    /// Supported sample rates.
    pub sample_rates: Vec<f64>,
    /// Supported output channel pairs.
    pub output_pairs: Vec<String>,
    /// Active output pair, or -1 for the default.
    pub active_output_pair: i32,
    /// Whether input and output are selected separately.
    pub separate_io: bool,
    /// Whether a vendor control panel is available.
    pub has_control_panel: bool,
    /// Whether Bluetooth MIDI pairing is available.
    pub bt_midi_available: bool,
    /// Latest mutation error shown inline.
    pub error: Option<String>,
    /// Short text while the audio system works, such as "Opening…".
    pub status: Option<String>,
    /// A message about a saved choice or a driver that is not available.
    pub notice: Option<String>,
    /// What the page says about a rate that the sound server converts.
    pub rate_note: Option<String>,
}

impl SystemDeviceState {
    /// Parses the complete neutral fixture representation.
    #[must_use]
    pub fn from_ui_value(value: &serde_json::Value) -> Option<Self> {
        let banner = AudioDeviceState::from_ui_value(value)?;
        let strings = |key: &str| {
            value[key]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        // A fixture can give each row as one name, which then is also its identity.
        let rows = |key: &str| -> Vec<AudioDeviceOption> {
            value[key]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            if let Some(name) = item.as_str() {
                                return Some(AudioDeviceOption::named(name));
                            }
                            Some(AudioDeviceOption {
                                id: item["id"].as_str()?.to_owned(),
                                label: item["label"].as_str()?.to_owned(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let selected = |id_key: &str, name_key: &str| {
            value[id_key]
                .as_str()
                .or_else(|| value[name_key].as_str())
                .unwrap_or_default()
                .to_owned()
        };
        let input_channels = value["inputChannels"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|channel| {
                        Some(AudioInputChannel {
                            index: usize::try_from(channel["index"].as_u64()?).ok()?,
                            name: channel["name"].as_str()?.to_owned(),
                            active: channel["active"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let midi_inputs = value["midiInputs"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|input| {
                        Some(MidiInputDevice {
                            id: input["id"].as_str()?.to_owned(),
                            name: input["name"].as_str()?.to_owned(),
                            enabled: input["enabled"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            banner,
            device_types: strings("deviceTypes"),
            input_devices: rows("inputDevices"),
            output_devices: rows("outputDevices"),
            selected_input: selected("inputDeviceId", "inputDevice"),
            selected_output: selected("outputDeviceId", "outputDevice"),
            input_channels,
            midi_inputs,
            sample_rates: value["sampleRates"]
                .as_array()
                .map(|items| items.iter().filter_map(serde_json::Value::as_f64).collect())
                .unwrap_or_default(),
            output_pairs: strings("outputPairs"),
            active_output_pair: value["activeOutputPair"].as_i64().unwrap_or(-1) as i32,
            separate_io: value["separateIO"].as_bool().unwrap_or(true),
            has_control_panel: value["hasControlPanel"].as_bool().unwrap_or(false),
            bt_midi_available: value["btMidiAvailable"].as_bool().unwrap_or(false),
            error: None,
            status: None,
            notice: None,
            rate_note: None,
        })
    }

    /// Current typed device configuration for the audio manager.
    #[must_use]
    pub fn config(&self) -> AudioDeviceConfig {
        AudioDeviceConfig {
            sample_rate: Some(self.banner.sample_rate),
            buffer_frames: Some(self.banner.buffer_size),
            input: (!self.selected_input.is_empty()).then(|| self.selected_input.clone()),
            output: (!self.selected_output.is_empty()).then(|| self.selected_output.clone()),
            midi_inputs: self
                .midi_inputs
                .iter()
                .filter(|input| input.enabled)
                .map(|input| input.id.clone())
                .collect(),
        }
    }

    /// Applies a typed configuration after the audio manager accepts it.
    pub fn apply_config(&mut self, config: &AudioDeviceConfig) {
        if let Some(rate) = config.sample_rate {
            self.banner.sample_rate = rate;
        }
        if let Some(frames) = config.buffer_frames {
            self.banner.buffer_size = frames;
        }
        let label = |rows: &[AudioDeviceOption], id: &Option<String>| {
            let id = id.as_deref().unwrap_or_default();
            rows.iter()
                .find(|row| row.id == id)
                .map_or_else(|| id.to_owned(), |row| row.label.clone())
        };
        self.banner.input_device = label(&self.input_devices, &config.input);
        self.banner.output_device = label(&self.output_devices, &config.output);
        self.selected_input = config.input.clone().unwrap_or_default();
        self.selected_output = config.output.clone().unwrap_or_default();
        for input in &mut self.midi_inputs {
            input.enabled = config.midi_inputs.contains(&input.id);
        }
    }

    /// Input rows, including a stale disconnected selection when needed.
    #[must_use]
    pub fn input_options(&self) -> Vec<AudioDeviceOption> {
        device_options(
            &self.input_devices,
            &self.selected_input,
            &self.banner.input_device,
        )
    }

    /// Output rows, including a stale disconnected selection when needed.
    #[must_use]
    pub fn output_options(&self) -> Vec<AudioDeviceOption> {
        device_options(
            &self.output_devices,
            &self.selected_output,
            &self.banner.output_device,
        )
    }

    /// Inline recovery error for a configured device that failed to open.
    #[must_use]
    pub fn inline_error(&self) -> Option<String> {
        if self.error.is_some() {
            return self.error.clone();
        }
        let has_selection =
            !self.banner.input_device.is_empty() || !self.banner.output_device.is_empty();
        (has_selection && !self.banner.device_open)
            .then(|| "The audio device couldn’t be opened.".to_owned())
    }
}

fn device_options(
    devices: &[AudioDeviceOption],
    current: &str,
    current_label: &str,
) -> Vec<AudioDeviceOption> {
    let mut options = Vec::new();
    if !current.is_empty() && !devices.iter().any(|device| device.id == current) {
        options.push(AudioDeviceOption {
            id: current.to_owned(),
            label: format!("{current_label} (disconnected)"),
        });
    }
    options.extend(devices.iter().cloned());
    options
}

/// Audio settings controller that sends complete mutations through `UiBackend`.
pub struct SystemSettingsController {
    backend: Arc<dyn UiBackend>,
    selection_order: RefCell<Vec<usize>>,
    last_error: RefCell<Option<String>>,
    advanced: Cell<bool>,
    last_refresh: Cell<Option<f64>>,
    /// Whether the user saved a TONE3000 key, so the page offers its removal.
    key_saved: Cell<bool>,
    /// Whether the user asked to remove the key. The chrome takes this request.
    remove_key_requested: Cell<bool>,
}

impl SystemSettingsController {
    /// Creates a controller from the latest device snapshot.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>) -> Self {
        let selection_order = backend.device_settings().map_or_else(Vec::new, |device| {
            device
                .input_channels
                .iter()
                .filter(|channel| channel.active)
                .map(|channel| channel.index)
                .collect()
        });
        Self {
            backend,
            selection_order: RefCell::new(selection_order),
            last_error: RefCell::new(None),
            advanced: Cell::new(false),
            last_refresh: Cell::new(None),
            key_saved: Cell::new(false),
            remove_key_requested: Cell::new(false),
        }
    }

    /// Selects whether the page shows the saved key and its "Remove key" control.
    pub fn set_key_saved(&self, saved: bool) {
        self.key_saved.set(saved);
    }

    /// Whether the page shows the saved key.
    #[must_use]
    pub fn key_saved(&self) -> bool {
        self.key_saved.get()
    }

    /// Takes the request of the "Remove key" control, at most one time.
    #[must_use]
    pub fn take_remove_key_request(&self) -> bool {
        self.remove_key_requested.take()
    }

    /// Stable section names in display order.
    #[must_use]
    pub fn section_names(&self) -> Vec<&'static str> {
        vec![
            "Audio",
            "Input",
            "Output",
            "Quality",
            "Advanced",
            "Driver",
            "Channels",
            "Hear Yourself",
            "MIDI Inputs",
        ]
    }

    /// Latest complete device snapshot.
    #[must_use]
    pub fn device(&self) -> Option<SystemDeviceState> {
        self.backend.device_settings()
    }

    /// Changes the audio backend without rebuilding view state.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn set_backend(&self, backend: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Device(DeviceAction::SelectBackend(
                backend.to_owned(),
            )))?;
        Ok(())
    }

    /// Changes the sample rate.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn set_sample_rate(&self, rate: u32) -> Result<(), UiError> {
        let mut config = self
            .device()
            .ok_or_else(|| UiError::Control("audio state is unavailable".to_owned()))?
            .config();
        config.sample_rate = Some(f64::from(rate));
        self.configure(config)
    }

    /// Changes the callback buffer size.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn set_buffer_size(&self, frames: u32) -> Result<(), UiError> {
        let mut config = self
            .device()
            .ok_or_else(|| UiError::Control("audio state is unavailable".to_owned()))?
            .config();
        config.buffer_frames = Some(frames);
        self.configure(config)
    }

    /// Changes the selected input device, or selects none.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn set_input_device(&self, device: Option<String>) -> Result<(), UiError> {
        let mut config = self
            .device()
            .ok_or_else(|| UiError::Control("audio state is unavailable".to_owned()))?
            .config();
        config.input = device;
        self.configure(config)
    }

    /// Changes the selected output device, or selects none.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn set_output_device(&self, device: Option<String>) -> Result<(), UiError> {
        let mut config = self
            .device()
            .ok_or_else(|| UiError::Control("audio state is unavailable".to_owned()))?
            .config();
        config.output = device;
        self.configure(config)
    }

    fn configure(&self, config: AudioDeviceConfig) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Device(DeviceAction::Configure(config)))?;
        Ok(())
    }

    /// Enables or disables input monitoring.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn set_hear_yourself(&self, enabled: bool) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Device(DeviceAction::SetHearYourself(enabled)))?;
        Ok(())
    }

    /// Selects one channel, swapping out the oldest selection in stereo mode.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn select_channel(&self, index: usize) -> Result<(), UiError> {
        if self.active_channels().contains(&index) {
            return Ok(());
        }
        let stereo = self.backend.chain_state(0).session.input_mode == InputMode::Stereo;
        let mut next = self.active_channels();
        next.push(index);
        if stereo {
            if next.len() > 2 {
                next.drain(..next.len() - 2);
            }
        } else {
            next = vec![index];
        }
        self.set_input_channels(next)
    }

    /// Switches between mono and stereo input selection.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn set_stereo(&self, stereo: bool) -> Result<(), UiError> {
        let current = self.active_channels();
        let latest = current.last().copied().unwrap_or(0);
        let next = if stereo {
            let other = self
                .device()
                .and_then(|device| {
                    device
                        .input_channels
                        .iter()
                        .find(|channel| channel.index != latest)
                        .map(|channel| channel.index)
                })
                .unwrap_or(latest);
            vec![latest, other]
        } else {
            vec![latest]
        };
        self.backend.dispatch(UiAction::SetInputMode(if stereo {
            InputMode::Stereo
        } else {
            InputMode::Left
        }))?;
        self.set_input_channels(next)
    }

    fn set_input_channels(&self, channels: Vec<usize>) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Device(DeviceAction::SetInputChannels(
                channels.clone(),
            )))?;
        *self.selection_order.borrow_mut() = channels;
        Ok(())
    }

    /// Selected channels in most-recent selection order.
    #[must_use]
    pub fn active_channels(&self) -> Vec<usize> {
        let active = self.device().map_or_else(Vec::new, |device| {
            device
                .input_channels
                .iter()
                .filter(|channel| channel.active)
                .map(|channel| channel.index)
                .collect::<Vec<_>>()
        });
        let mut ordered = self
            .selection_order
            .borrow()
            .iter()
            .copied()
            .filter(|index| active.contains(index))
            .collect::<Vec<_>>();
        for index in active {
            if !ordered.contains(&index) {
                ordered.push(index);
            }
        }
        ordered
    }

    /// Latest raw-input levels for the visible channel meters.
    #[must_use]
    pub fn input_levels(&self) -> Vec<f32> {
        self.backend.input_channel_levels()
    }

    /// Enables or disables one MIDI input.
    ///
    /// # Errors
    /// Returns a backend configuration or persistence error.
    pub fn set_midi_enabled(&self, id: &str, enabled: bool) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Device(DeviceAction::SetMidiInputEnabled {
                id: id.to_owned(),
                enabled,
            }))?;
        Ok(())
    }

    /// Plays a short output test tone.
    ///
    /// # Errors
    /// Returns a backend output error.
    pub fn play_test_tone(&self) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Device(DeviceAction::PlayTestTone))?;
        Ok(())
    }

    /// Selects an output channel pair.
    ///
    /// # Errors
    /// Returns a backend configuration error.
    pub fn set_output_pair(&self, pair: i32) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Device(DeviceAction::SetOutputPair(pair)))?;
        Ok(())
    }

    /// IDs of inline warnings shared with the global banner evaluator.
    #[must_use]
    pub fn inline_banner_ids(&self) -> Vec<&'static str> {
        let Some(device) = self.device() else {
            return Vec::new();
        };
        let mut store = BannerStore::new();
        store.evaluate(Some(&device.banner), 0);
        store
            .active()
            .and_then(|banner| match banner.id.as_str() {
                "mic-denied" => Some(vec!["mic-denied"]),
                "no-input" => Some(vec!["no-input"]),
                "no-output" => Some(vec!["no-output"]),
                "input-muted" => Some(vec!["input-muted"]),
                "feedback-risk" => Some(vec!["feedback-risk"]),
                "asio-nudge" => Some(vec!["asio-nudge"]),
                "buffer-latency" => Some(vec!["buffer-latency"]),
                "rate-not-48k" => Some(vec!["rate-not-48k"]),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn apply_view(&self, result: Result<(), UiError>) {
        if let Err(error) = result {
            *self.last_error.borrow_mut() = Some(error.to_string());
        } else {
            self.last_error.borrow_mut().take();
        }
    }

    /// Whether the advanced part of the page is open.
    #[must_use]
    pub fn advanced_open(&self) -> bool {
        self.advanced.get()
    }

    /// Opens or closes the advanced part of the page.
    pub fn set_advanced_open(&self, open: bool) {
        self.advanced.set(open);
    }

    /// Asks for a new look at the devices, at most one time in each interval.
    fn refresh_devices_at_intervals(&self, ui: &egui::Ui) {
        let now = ui.input(|input| input.time);
        if self
            .last_refresh
            .get()
            .is_none_or(|last| now - last >= REFRESH_SECONDS)
        {
            self.last_refresh.set(Some(now));
            self.backend.refresh_devices();
        }
    }

    /// Renders the complete standalone audio and MIDI page, and then the saved key.
    pub fn show(&self, ui: &mut egui::Ui) {
        let shown = self.show_audio(ui);
        if self.key_saved.get() {
            if !shown {
                ui.add_space(forms::SECTION_GAP - 0.5);
            }
            self.show_key(ui);
        }
    }

    /// The community section with the saved key. It never shows the key or a part of it.
    fn show_key(&self, ui: &mut egui::Ui) {
        forms::heading(ui, KEY_SECTION);
        ui.add_space(8.0);
        forms::help(ui, "A TONE3000 key is saved.");
        ui.add_space(forms::CONTROL_GAP);
        let (row, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), forms::FIELD_HEIGHT),
            egui::Sense::hover(),
        );
        let button = egui::Rect::from_min_size(row.min, egui::vec2(132.0, forms::FIELD_HEIGHT));
        if outlined_text_button(ui, button, "Remove key", true) {
            self.remove_key_requested.set(true);
        }
        ui.add_space(forms::SECTION_GAP - 0.5);
    }

    /// Renders the audio sections. Returns false when no device state exists.
    fn show_audio(&self, ui: &mut egui::Ui) -> bool {
        let Some(device) = self.device() else {
            forms::help(ui, "Audio device state is unavailable.");
            return false;
        };
        self.refresh_devices_at_intervals(ui);
        let mut banners = BannerStore::new();
        banners.evaluate(Some(&device.banner), 0);
        let banner = banners.active().cloned();

        if device.banner.mic_denied
            && let Some(spec) = banner.as_ref().filter(|spec| spec.id == "mic-denied")
        {
            if alert_card(ui, spec, true) {
                let _ = self.backend.dispatch(UiAction::Navigate(
                    crate::backend::NavigationAction::OpenMicrophonePrivacy,
                ));
            }
            ui.add_space(forms::CONTROL_GAP);
        }
        if let Some(error) = device
            .inline_error()
            .or_else(|| self.last_error.borrow().clone())
        {
            let retry = alert_message(
                ui,
                "The audio device couldn’t be opened.",
                &format!("{error} Hit Retry, or choose another device."),
                super::BannerVariant::Error,
            );
            if retry {
                self.apply_view(
                    self.backend
                        .dispatch(UiAction::Device(DeviceAction::Restart))
                        .map(|_| ()),
                );
            }
            ui.add_space(forms::CONTROL_GAP);
        }
        if let Some(notice) = &device.notice {
            alert_content(ui, "", notice, super::BannerVariant::Warning, None);
            ui.add_space(forms::CONTROL_GAP);
        }
        let hear_warnings = self.inline_banner_ids();
        let mut showed_warning = false;
        for warning in hear_warnings
            .iter()
            .filter(|id| matches!(**id, "feedback-risk" | "input-muted"))
        {
            if let Some(spec) = banner.as_ref().filter(|spec| spec.id == *warning) {
                alert_card(ui, spec, false);
                showed_warning = true;
            }
        }
        if showed_warning {
            ui.add_space(forms::CONTROL_GAP);
        }
        audio_group(self, ui, &device, banner.as_ref());
        ui.add_space(forms::SECTION_GAP - 0.5);
        true
    }
}

/// One label at the left and its controls in the rest of the row.
fn labeled_row(ui: &mut egui::Ui, label: &str, height: f32) -> egui::Rect {
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    if !label.is_empty() {
        ui.painter().text(
            egui::pos2(
                row.left(),
                row.top() + forms::FIELD_HEIGHT.min(height) / 2.0,
            ),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::new(13.0, egui::FontFamily::Name("arimo-bold".into())),
            egui::Color32::WHITE,
        );
    }
    egui::Rect::from_min_max(
        egui::pos2(row.left() + ROW_LABEL_WIDTH, row.top()),
        row.right_bottom(),
    )
}

fn audio_group(
    controller: &SystemSettingsController,
    ui: &mut egui::Ui,
    device: &SystemDeviceState,
    banner: Option<&super::BannerSpec>,
) {
    egui::Frame::new()
        .fill(egui::Color32::BLACK)
        .stroke(egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE))
        .corner_radius(10.0)
        .inner_margin(20)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            group_title(ui, device);
            ui.add_space(forms::CONTROL_GAP);
            input_row(controller, ui, device);
            ui.add_space(forms::CONTROL_GAP);
            output_device_row(controller, ui, device);
            ui.add_space(forms::CONTROL_GAP);
            quality_row(controller, ui, device);
            quality_notes(ui, device, banner);
            ui.add_space(forms::CONTROL_GAP);
            advanced_header(controller, ui);
            if controller.advanced.get() {
                advanced_rows(controller, ui, device);
            }
        });
}

fn group_title(ui: &mut egui::Ui, device: &SystemDeviceState) {
    let (title, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
    draw_audio_interface(
        ui,
        egui::Rect::from_min_size(title.left_top(), egui::Vec2::splat(20.0)),
    );
    ui.painter().text(
        egui::pos2(title.left() + 30.0, title.center().y),
        egui::Align2::LEFT_CENTER,
        "AUDIO",
        egui::FontId::new(16.0, egui::FontFamily::Name("arimo-bold".into())),
        egui::Color32::WHITE,
    );
    if let Some(status) = &device.status {
        ui.painter().text(
            title.right_center(),
            egui::Align2::RIGHT_CENTER,
            status,
            egui::FontId::proportional(12.0),
            crate::theme::palette::TEXT_MUTED,
        );
    }
}

fn input_row(controller: &SystemSettingsController, ui: &mut egui::Ui, device: &SystemDeviceState) {
    let field = labeled_row(ui, "Input", forms::FIELD_HEIGHT);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
    device_select(
        &mut child,
        "input-device",
        "Input device",
        &device.selected_input,
        &device.input_options(),
        |selected| controller.apply_view(controller.set_input_device(selected)),
    );
    let levels = controller.input_levels();
    let active = controller.active_channels();
    let level = |channel: usize| levels.get(channel).copied().unwrap_or(LEVEL_FLOOR_DB);
    ui.add_space(8.0);
    let meter = labeled_row(ui, "", 16.0);
    let shown = if active.is_empty() {
        LEVEL_FLOOR_DB
    } else {
        active
            .iter()
            .map(|channel| level(*channel))
            .fold(LEVEL_FLOOR_DB, f32::max)
    };
    ui.painter().text(
        meter.left_center(),
        egui::Align2::LEFT_CENTER,
        "Level",
        egui::FontId::proportional(12.0),
        crate::theme::palette::TEXT_MUTED,
    );
    dot_meter(
        ui,
        egui::Rect::from_min_size(
            egui::pos2(meter.left() + 44.0, meter.center().y - 2.0),
            egui::vec2(88.0, 4.0),
        ),
        shown,
    );
    // A guitar on another channel is the usual reason for a silent input.
    if shown < SIGNAL_FLOOR_DB
        && let Some(channel) = device.input_channels.iter().find(|channel| {
            !active.contains(&channel.index) && level(channel.index) >= SIGNAL_FLOOR_DB
        })
    {
        ui.add_space(8.0);
        forms::help(
            ui,
            &format!(
                "Signal on {}. Open Advanced to choose that channel.",
                channel.name
            ),
        );
    }
}

fn quality_row(
    controller: &SystemSettingsController,
    ui: &mut egui::Ui,
    device: &SystemDeviceState,
) {
    let row = labeled_row(ui, "Quality", forms::FIELD_HEIGHT);
    // The rate text is short, and the buffer text holds its size and its time.
    let rate_rect =
        egui::Rect::from_min_size(row.left_top(), egui::vec2(RATE_FIELD_WIDTH, row.height()));
    let buffer_rect = egui::Rect::from_min_max(
        egui::pos2(row.left() + RATE_FIELD_WIDTH + 8.0, row.top()),
        row.right_bottom(),
    );
    let mut rate_ui = ui.new_child(egui::UiBuilder::new().max_rect(rate_rect));
    let mut selected = device.banner.sample_rate;
    let response = forms::select_field(
        &mut rate_ui,
        "sample-rate",
        format!("{} Hz", format_rate(selected)),
        false,
        |ui| {
            for rate in &device.sample_rates {
                ui.selectable_value(&mut selected, *rate, format!("{} Hz", format_rate(*rate)));
            }
        },
    );
    response.response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Sample rate".to_owned())
    });
    if (selected - device.banner.sample_rate).abs() > f64::EPSILON {
        controller.apply_view(controller.set_sample_rate(selected as u32));
    }
    let mut buffer_ui = ui.new_child(egui::UiBuilder::new().max_rect(buffer_rect));
    let mut selected = device.banner.buffer_size;
    let response = forms::select_field(
        &mut buffer_ui,
        "buffer-size",
        format_buffer_option(selected, device.banner.sample_rate),
        false,
        |ui| {
            for size in &device.banner.buffer_sizes {
                ui.selectable_value(
                    &mut selected,
                    *size,
                    format_buffer_option(*size, device.banner.sample_rate),
                );
            }
        },
    );
    response.response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Buffer size".to_owned())
    });
    if selected != device.banner.buffer_size {
        controller.apply_view(controller.set_buffer_size(selected));
    }
}

fn quality_notes(
    ui: &mut egui::Ui,
    device: &SystemDeviceState,
    banner: Option<&super::BannerSpec>,
) {
    if let Some(delay) = device.banner.delay_ms {
        ui.add_space(8.0);
        forms::help(
            ui,
            &format!("The delay from the input to the output is about {delay:.0} ms."),
        );
    }
    if let Some(note) = &device.rate_note {
        ui.add_space(8.0);
        forms::help(ui, note);
    }
    if device.banner.buffer_sizes.len() == 1 && device.banner.current_type == "JACK" {
        ui.add_space(8.0);
        forms::help(ui, "The JACK server sets the rate and the buffer.");
    }
    if let Some(spec) = banner.filter(|spec| spec.id == "rate-not-48k") {
        ui.add_space(forms::CONTROL_GAP);
        let _ = alert_card(ui, spec, false);
    }
}

fn advanced_header(controller: &SystemSettingsController, ui: &mut egui::Ui) {
    let open = controller.advanced.get();
    let (row, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
    let response = ui.interact(row, ui.scope_id().with("advanced"), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Advanced"));
    let color = if response.hovered() {
        egui::Color32::WHITE
    } else {
        crate::theme::palette::TEXT_MUTED
    };
    let center = egui::pos2(row.left() + 5.0, row.center().y);
    let (tip, wings) = if open {
        (
            center + egui::vec2(0.0, 3.0),
            [
                center + egui::vec2(-4.0, -2.0),
                center + egui::vec2(4.0, -2.0),
            ],
        )
    } else {
        (
            center + egui::vec2(3.0, 0.0),
            [
                center + egui::vec2(-2.0, -4.0),
                center + egui::vec2(-2.0, 4.0),
            ],
        )
    };
    let stroke = egui::Stroke::new(1.6, color);
    ui.painter().line_segment([wings[0], tip], stroke);
    ui.painter().line_segment([wings[1], tip], stroke);
    ui.painter().text(
        egui::pos2(row.left() + 18.0, row.center().y),
        egui::Align2::LEFT_CENTER,
        "Advanced",
        egui::FontId::new(13.0, egui::FontFamily::Name("arimo-bold".into())),
        color,
    );
    if response.clicked() {
        controller.advanced.set(!open);
    }
}

fn advanced_rows(
    controller: &SystemSettingsController,
    ui: &mut egui::Ui,
    device: &SystemDeviceState,
) {
    if device.device_types.len() > 1 {
        ui.add_space(forms::CONTROL_GAP);
        let row = labeled_row(ui, "Driver", 26.0);
        let names: Vec<&str> = device.device_types.iter().map(String::as_str).collect();
        let active = names
            .iter()
            .position(|name| *name == device.banner.current_type)
            .unwrap_or(0);
        if let Some(chosen) = segmented_control(ui, row.left_top(), &names, active, "audio-driver")
        {
            controller.apply_view(controller.set_backend(names[chosen]));
        }
    }
    input_channel_picker(controller, ui, device);
    if device.output_pairs.len() > 1 {
        ui.add_space(forms::CONTROL_GAP);
        let field = labeled_row(ui, "Output pair", forms::FIELD_HEIGHT);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let mut selected = device.active_output_pair.max(0);
        let selected_text = device
            .output_pairs
            .get(selected as usize)
            .cloned()
            .unwrap_or_default();
        let response =
            forms::select_field(&mut child, "output-pairs", selected_text, false, |ui| {
                for (index, pair) in device.output_pairs.iter().enumerate() {
                    ui.selectable_value(&mut selected, index as i32, pair);
                }
            });
        response.response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Output channels".to_owned())
        });
        if selected != device.active_output_pair.max(0) {
            controller.apply_view(controller.set_output_pair(selected));
        }
    }
    ui.add_space(forms::CONTROL_GAP);
    let mut hear = device.banner.hear_yourself;
    if forms::toggle_row_with_gap(
        ui,
        "Hear Yourself",
        "Hear your instrument while playing.",
        &mut hear,
        4.0,
    ) {
        controller.apply_view(controller.set_hear_yourself(hear));
    }
    if device.has_control_panel {
        ui.add_space(forms::CONTROL_GAP);
        let (buttons, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), forms::FIELD_HEIGHT),
            egui::Sense::hover(),
        );
        let width = (buttons.width() - 8.0) / 2.0;
        for (index, label) in ["Open Control Panel", "Reset Device"]
            .into_iter()
            .enumerate()
        {
            let rect = egui::Rect::from_min_size(
                buttons.left_top() + egui::vec2(index as f32 * (width + 8.0), 0.0),
                egui::vec2(width, buttons.height()),
            );
            if outlined_text_button(ui, rect, label, true) {
                let _ = controller.backend.dispatch(UiAction::Device(if index == 0 {
                    DeviceAction::OpenControlPanel
                } else {
                    DeviceAction::Restart
                }));
            }
        }
    }
    ui.add_space(forms::CONTROL_GAP);
    midi_inputs_section(controller, ui, device);
}

/// A select field whose rows carry an identity, so that one click selects one row and two rows
/// with one label stay two rows.
fn device_select(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    current: &str,
    options: &[AudioDeviceOption],
    apply: impl FnOnce(Option<String>),
) {
    let mut selected = current.to_owned();
    let selected_text = options
        .iter()
        .find(|option| option.id == current)
        .map_or_else(|| "No device".to_owned(), |option| option.label.clone());
    let response = forms::select_field(ui, id, selected_text, current.is_empty(), |ui| {
        for option in options {
            ui.selectable_value(&mut selected, option.id.clone(), &option.label);
        }
    });
    response
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, label.to_owned()));
    if selected != current {
        apply((!selected.is_empty()).then_some(selected));
    }
}

fn input_channel_picker(
    controller: &SystemSettingsController,
    ui: &mut egui::Ui,
    device: &SystemDeviceState,
) {
    // One channel leaves nothing to choose.
    if device.input_channels.len() < 2 {
        return;
    }
    let stereo = controller.active_channels().len() > 1;
    ui.add_space(forms::CONTROL_GAP);
    let row = labeled_row(ui, "Channels", 26.0);
    if let Some(chosen) = segmented_control(
        ui,
        row.left_top(),
        &["Mono", "Stereo"],
        usize::from(stereo),
        "input-mode",
    ) {
        controller.apply_view(controller.set_stereo(chosen == 1));
    }
    ui.add_space(8.0);
    forms::help(
        ui,
        if stereo {
            "Play your instrument and pick the two channels with signal."
        } else {
            "Play your instrument and select the channel with signal."
        },
    );
    ui.add_space(8.0);
    let levels = controller.input_levels();
    let height = (device.input_channels.len() as f32 * 34.0).min(296.0);
    let (list, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    for (row, channel) in device.input_channels.iter().enumerate() {
        let rect = egui::Rect::from_min_size(
            list.left_top() + egui::vec2(0.0, row as f32 * 34.0),
            egui::vec2(list.width(), 34.0),
        );
        let selected = controller.active_channels().contains(&channel.index);
        let response = ui.interact(
            rect,
            ui.scope_id().with(("input-channel", channel.index)),
            egui::Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::RadioButton,
                true,
                selected,
                format!("Input channel {}", channel.index + 1),
            )
        });
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, 8.0, egui::Color32::from_white_alpha(15));
        }
        let indicator = egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(4.0, 8.0),
            egui::Vec2::splat(18.0),
        );
        choice_indicator(ui, indicator, selected, stereo);
        let index_x = indicator.right() + 12.0;
        ui.painter().text(
            egui::pos2(index_x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            (channel.index + 1).to_string(),
            egui::FontId::proportional(14.0),
            if selected {
                egui::Color32::WHITE
            } else {
                crate::theme::palette::TEXT_MUTED
            },
        );
        let name_x = index_x + 16.0 + 12.0;
        let meter = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 4.0 - 44.0, rect.center().y),
            egui::vec2(88.0, 4.0),
        );
        ui.painter().text(
            egui::pos2(name_x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &channel.name,
            egui::FontId::new(11.0, egui::FontFamily::Name("roboto-mono".into())),
            crate::theme::palette::TEXT_SUBTLE,
        );
        dot_meter(
            ui,
            meter,
            levels.get(channel.index).copied().unwrap_or(LEVEL_FLOOR_DB),
        );
        if response.clicked() {
            controller.apply_view(controller.select_channel(channel.index));
        }
    }
    if stereo {
        ui.add_space(8.0);
        forms::help(
            ui,
            "Pick any two. Selecting a third swaps out your oldest pick.",
        );
    }
}

/// A row of choices that starts at `origin`. It returns the choice that a click selected.
fn segmented_control(
    ui: &mut egui::Ui,
    origin: egui::Pos2,
    labels: &[&str],
    active: usize,
    id: &str,
) -> Option<usize> {
    let font = egui::FontId::new(11.0, egui::FontFamily::Name("arimo-bold".into()));
    let widths: Vec<f32> = labels
        .iter()
        .map(|label| {
            ui.ctx().fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap((*label).to_owned(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            }) + 24.0
        })
        .collect();
    let rect =
        egui::Rect::from_min_size(origin, egui::vec2(widths.iter().sum::<f32>() + 6.0, 26.0));
    ui.painter()
        .rect_filled(rect, 8.0, egui::Color32::from_rgb(10, 10, 10));
    ui.painter().rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
        egui::StrokeKind::Inside,
    );
    let mut selected = None;
    let mut x = rect.left() + 3.0;
    for (index, label) in labels.iter().enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + 3.0),
            egui::vec2(widths[index], rect.height() - 6.0),
        );
        let is_active = index == active;
        if is_active {
            ui.painter()
                .rect_filled(cell, 6.0, egui::Color32::from_white_alpha(41));
        }
        let response = ui.interact(cell, ui.scope_id().with((id, *label)), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::RadioButton, true, is_active, *label)
        });
        ui.painter().text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if is_active {
                egui::Color32::WHITE
            } else {
                crate::theme::palette::TEXT_MUTED
            },
        );
        if response.clicked() && !is_active {
            selected = Some(index);
        }
        x += cell.width();
    }
    selected
}

fn output_device_row(
    controller: &SystemSettingsController,
    ui: &mut egui::Ui,
    device: &SystemDeviceState,
) {
    let row = labeled_row(ui, "Output", forms::FIELD_HEIGHT);
    let test_tone = controller.backend.supports(HostControl::TestTone);
    let select_width = if test_tone {
        row.width() - 84.0
    } else {
        row.width()
    };
    let select = egui::Rect::from_min_size(row.left_top(), egui::vec2(select_width, row.height()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(select));
    device_select(
        &mut child,
        "output-device",
        "Output device",
        &device.selected_output,
        &device.output_options(),
        |selected| controller.apply_view(controller.set_output_device(selected)),
    );
    if !test_tone {
        return;
    }
    let test = egui::Rect::from_min_size(
        egui::pos2(row.right() - 76.0, row.top()),
        egui::vec2(76.0, row.height()),
    );
    let enabled = device.banner.device_open && !device.banner.output_device.is_empty();
    if test_button(ui, test, enabled) {
        controller.apply_view(controller.play_test_tone());
    }
}

fn test_button(ui: &mut egui::Ui, rect: egui::Rect, enabled: bool) -> bool {
    let response = ui.interact(
        rect,
        ui.scope_id().with("output-test"),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Test"));
    let color = if enabled {
        egui::Color32::WHITE
    } else {
        crate::theme::palette::TEXT_SUBTLE
    };
    ui.painter().rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
        egui::StrokeKind::Inside,
    );
    let font = egui::FontId::proportional(14.0);
    let galley = ui
        .ctx()
        .fonts_mut(|fonts| fonts.layout_no_wrap("Test".to_owned(), font, color));
    let content_width = 22.0 + galley.size().x;
    let left = rect.center().x - content_width / 2.0;
    draw_volume(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(left + 7.5, rect.center().y),
            egui::Vec2::splat(15.0),
        ),
        color,
    );
    ui.painter().galley(
        egui::pos2(left + 22.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    enabled && response.clicked()
}

fn midi_inputs_help(ui: &mut egui::Ui) {
    let mut job = egui::text::LayoutJob::single_section(
        "Enable the devices you want to control the plugin with. Set what each knob or pedal does in Plugin Settings → MIDI Mapping.".to_owned(),
        egui::TextFormat {
            font_id: egui::FontId::proportional(14.0),
            color: crate::theme::palette::TEXT_MUTED,
            line_height: Some(20.3),
            ..Default::default()
        },
    );
    job.wrap.max_width = ui.available_width();
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    let _ = ui.add(egui::Label::new(galley));
}

fn midi_inputs_section(
    controller: &SystemSettingsController,
    ui: &mut egui::Ui,
    device: &SystemDeviceState,
) {
    forms::heading(ui, "MIDI Inputs");
    ui.add_space(8.0);
    midi_inputs_help(ui);
    ui.add_space(forms::CONTROL_GAP);
    let height = if device.midi_inputs.is_empty() {
        58.8
    } else {
        2.0 + device.midi_inputs.len() as f32 * 40.0
    };
    let (card, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    ui.painter().rect_stroke(
        card,
        10.0,
        egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
        egui::StrokeKind::Inside,
    );
    if device.midi_inputs.is_empty() {
        ui.painter().text(
            card.center(),
            egui::Align2::CENTER_CENTER,
            "No MIDI devices found. Connect one and it will appear here.",
            egui::FontId::new(12.0, egui::FontFamily::Name("arimo-italic".into())),
            crate::theme::palette::TEXT_SUBTLE,
        );
    } else {
        for (index, input) in device.midi_inputs.iter().enumerate() {
            let rect = egui::Rect::from_min_size(
                card.left_top() + egui::vec2(1.0, 1.0 + index as f32 * 40.0),
                egui::vec2(card.width() - 2.0, 40.0),
            );
            let response = ui.interact(
                rect,
                ui.scope_id().with(("midi-input", &input.id)),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::CheckBox, true, input.enabled, &input.name)
            });
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_white_alpha(15));
            }
            choice_indicator(
                ui,
                egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(13.0, 11.0),
                    egui::Vec2::splat(18.0),
                ),
                input.enabled,
                true,
            );
            ui.painter().text(
                rect.left_center() + egui::vec2(43.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &input.name,
                egui::FontId::proportional(14.0),
                if input.enabled {
                    egui::Color32::WHITE
                } else {
                    crate::theme::palette::TEXT_MUTED
                },
            );
            if input.enabled {
                ui.painter().text(
                    rect.right_center() - egui::vec2(13.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    "enabled",
                    egui::FontId::proportional(11.0),
                    crate::theme::palette::TEXT_SUBTLE,
                );
            }
            if response.clicked() {
                controller.apply_view(controller.set_midi_enabled(&input.id, !input.enabled));
            }
        }
    }
    if device.bt_midi_available {
        ui.add_space(forms::CONTROL_GAP);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 41.0), egui::Sense::hover());
        let response = ui.interact(
            rect,
            ui.scope_id().with("bluetooth-midi"),
            egui::Sense::click(),
        );
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Bluetooth MIDI"));
        ui.painter().rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
            egui::StrokeKind::Inside,
        );
        let font = egui::FontId::new(13.0, egui::FontFamily::Name("arimo-bold".into()));
        let galley = ui.ctx().fonts_mut(|fonts| {
            fonts.layout_no_wrap("Bluetooth MIDI".to_owned(), font, egui::Color32::WHITE)
        });
        let left = rect.center().x - (22.0 + galley.size().x) / 2.0;
        draw_bluetooth(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(left + 7.0, rect.center().y),
                egui::Vec2::splat(14.0),
            ),
        );
        ui.painter().galley(
            egui::pos2(left + 22.0, rect.center().y - galley.size().y / 2.0),
            galley,
            egui::Color32::WHITE,
        );
        if response.clicked() {
            let _ = controller
                .backend
                .dispatch(UiAction::Device(DeviceAction::OpenBluetoothMidi));
        }
    }
}

fn format_buffer_option(samples: u32, rate: f64) -> String {
    let mut label = format!("{samples} samples");
    if rate > 0.0 {
        let milliseconds = f64::from(samples) * 1000.0 / rate;
        if milliseconds < 10.0 {
            label.push_str(&format!(", {milliseconds:.1} ms"));
        } else {
            label.push_str(&format!(", {milliseconds:.0} ms"));
        }
    }
    label
}

fn format_rate(rate: f64) -> String {
    if (rate - rate.round()).abs() < 0.001 {
        format!("{}", rate.round() as i64)
    } else {
        rate.to_string()
    }
}

fn choice_indicator(ui: &mut egui::Ui, rect: egui::Rect, selected: bool, square: bool) {
    let color = if selected {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(113, 113, 122)
    };
    let stroke = egui::Stroke::new(2.0, color);
    if square {
        ui.painter()
            .rect_stroke(rect, 5.0, stroke, egui::StrokeKind::Inside);
        if selected {
            ui.painter().rect_filled(
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(8.0)),
                2.0,
                egui::Color32::WHITE,
            );
        }
    } else {
        ui.painter().circle_stroke(rect.center(), 8.0, stroke);
        if selected {
            ui.painter()
                .circle_filled(rect.center(), 4.0, egui::Color32::WHITE);
        }
    }
}

fn dot_meter(ui: &mut egui::Ui, rect: egui::Rect, db: f32) {
    for index in 0..7 {
        let position = index as f32 / 6.0;
        let threshold = -60.0 + index as f32 * 10.0;
        let active = if index == 6 {
            db >= 0.0
        } else {
            db >= threshold
        };
        let color = crate::theme::palette::fire_ramp(position);
        ui.painter().circle_filled(
            egui::pos2(rect.left() + 2.0 + index as f32 * 14.0, rect.center().y),
            2.0,
            egui::Color32::from_rgba_unmultiplied(
                color.r(),
                color.g(),
                color.b(),
                if active { 255 } else { 56 },
            ),
        );
    }
}

fn draw_audio_interface(ui: &mut egui::Ui, rect: egui::Rect) {
    let scale = rect.width() / 20.0;
    let point = |x: f32, y: f32| rect.left_top() + egui::vec2(x * scale, y * scale);
    let stroke = egui::Stroke::new(1.667 * scale, egui::Color32::WHITE);
    let points = [
        (1.84, 9.65),
        (4.54, 4.26),
        (5.16, 3.58),
        (6.03, 3.33),
        (13.97, 3.33),
        (14.84, 3.58),
        (15.46, 4.26),
        (18.16, 9.65),
        (18.33, 10.39),
        (18.33, 15.0),
        (17.84, 16.18),
        (16.67, 16.67),
        (3.33, 16.67),
        (2.15, 16.18),
        (1.67, 15.0),
        (1.67, 10.39),
        (1.84, 9.65),
    ]
    .map(|(x, y)| point(x, y));
    ui.painter().add(egui::Shape::line(points.to_vec(), stroke));
    ui.painter()
        .line_segment([point(1.71, 10.01), point(18.29, 10.01)], stroke);
    for x in [5.0, 8.34] {
        ui.painter()
            .circle_filled(point(x, 13.33), 0.75 * scale, egui::Color32::WHITE);
    }
}

fn draw_volume(ui: &mut egui::Ui, rect: egui::Rect, color: egui::Color32) {
    let scale = rect.width() / 24.0;
    let point = |x: f32, y: f32| rect.left_top() + egui::vec2(x * scale, y * scale);
    let stroke = egui::Stroke::new(2.0 * scale, color);
    let speaker = [
        (11.0, 4.7),
        (6.4, 7.6),
        (3.0, 8.0),
        (3.0, 16.0),
        (6.4, 16.4),
        (11.0, 19.3),
        (11.0, 4.7),
    ]
    .map(|(x, y)| point(x, y));
    ui.painter()
        .add(egui::Shape::line(speaker.to_vec(), stroke));
    for (radius, start, end) in [(5.0, -55.0, 55.0), (9.0, -45.0, 45.0)] {
        let points = (0..=16)
            .map(|step| {
                let angle = (start + (end - start) * step as f32 / 16.0).to_radians();
                point(11.0 + radius * angle.cos(), 12.0 + radius * angle.sin())
            })
            .collect::<Vec<_>>();
        ui.painter().add(egui::Shape::line(points, stroke));
    }
}

fn draw_bluetooth(ui: &mut egui::Ui, rect: egui::Rect) {
    let scale = rect.width() / 24.0;
    let point = |x: f32, y: f32| rect.left_top() + egui::vec2(x * scale, y * scale);
    let points = [
        (7.0, 7.0),
        (17.0, 17.0),
        (12.0, 22.0),
        (12.0, 2.0),
        (17.0, 7.0),
        (7.0, 17.0),
    ]
    .map(|(x, y)| point(x, y));
    ui.painter().add(egui::Shape::line(
        points.to_vec(),
        egui::Stroke::new(2.0 * scale, egui::Color32::WHITE),
    ));
}

fn outlined_text_button(ui: &mut egui::Ui, rect: egui::Rect, label: &str, enabled: bool) -> bool {
    let response = ui.interact(rect, ui.scope_id().with(label), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    ui.painter().rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if enabled {
            egui::Color32::WHITE
        } else {
            crate::theme::palette::TEXT_SUBTLE
        },
    );
    enabled && response.clicked()
}

fn alert_card(ui: &mut egui::Ui, spec: &super::BannerSpec, action: bool) -> bool {
    let body = match spec.id.as_str() {
        "mic-denied" => spec
            .body
            .replacen(" hear your instrument", "\nhear your instrument", 1)
            .replacen(" privacy settings", "\nprivacy settings", 1),
        "feedback-risk" => spec.body.replacen(" or turn", "\nor turn", 1),
        "input-muted" => spec
            .body
            .replacen(" speakers. Plug", "\nspeakers. Plug", 1)
            .replacen("Yourself” on.", "\nYourself” on.", 1),
        "rate-not-48k" => spec
            .body
            .replacen(" rate works fine", "\nrate works fine", 1),
        _ => spec.body.clone(),
    };
    alert_content(
        ui,
        &spec.headline,
        &body,
        spec.variant,
        action.then_some(spec.action_label.as_str()),
    )
}

fn alert_message(
    ui: &mut egui::Ui,
    headline: &str,
    body: &str,
    variant: super::BannerVariant,
) -> bool {
    alert_content(ui, headline, body, variant, Some("Retry"))
}

/// Left edge of the text column in an alert card.
const ALERT_TEXT_LEFT: f32 = 40.0;
/// Right padding of the text column in an alert card.
const ALERT_TEXT_RIGHT: f32 = 14.0;
/// Height of the action button of an alert card.
const ALERT_ACTION_HEIGHT: f32 = 22.0;
/// Space between the text and the action button of an alert card.
const ALERT_ACTION_GAP: f32 = 8.0;

/// The card height and the action button area, relative to the card top left.
///
/// The button is below the text, at the left edge of the text column, so it never covers the
/// text. A card without an action keeps its one-row height.
fn alert_layout(
    text_height: f32,
    text_y: f32,
    action_width: Option<f32>,
) -> (f32, Option<egui::Rect>) {
    let Some(action_width) = action_width else {
        return (text_height.max(17.0) + 24.0, None);
    };
    let button = egui::Rect::from_min_size(
        egui::pos2(ALERT_TEXT_LEFT, text_y + text_height + ALERT_ACTION_GAP),
        egui::vec2(action_width, ALERT_ACTION_HEIGHT),
    );
    (button.bottom() + 12.0, Some(button))
}

/// The headline and the body as one text, with one space between two sentences.
fn alert_join_needs_space(headline: &str, body: &str) -> bool {
    !headline.is_empty()
        && !body.is_empty()
        && !headline.ends_with(char::is_whitespace)
        && !body.starts_with(char::is_whitespace)
}

fn alert_content(
    ui: &mut egui::Ui,
    headline: &str,
    body: &str,
    variant: super::BannerVariant,
    action: Option<&str>,
) -> bool {
    let width = ui.available_width();
    let action_galley = action.map(|label| {
        ui.ctx().fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                label.to_owned(),
                egui::FontId::new(11.5, egui::FontFamily::Name("arimo-bold".into())),
                egui::Color32::WHITE,
            )
        })
    });
    let text_width = (width - ALERT_TEXT_LEFT - ALERT_TEXT_RIGHT).max(1.0);
    let mut job = egui::text::LayoutJob::default();
    let mut append = |text: &str, bold: bool, tracking: f32, leading: f32| {
        job.append(
            text,
            leading,
            egui::TextFormat {
                font_id: egui::FontId::new(
                    12.5,
                    if bold {
                        egui::FontFamily::Name("arimo-bold".into())
                    } else {
                        egui::FontFamily::Proportional
                    },
                ),
                color: egui::Color32::WHITE,
                line_height: Some(18.75),
                extra_letter_spacing: tracking,
                ..Default::default()
            },
        );
    };
    if !headline.is_empty() {
        append(headline, true, 0.0, 0.0);
    }
    if alert_join_needs_space(headline, body) {
        append(" ", false, 0.0, 0.0);
    }
    let mut remaining = body;
    if let Some(rest) = remaining.strip_prefix(" Ronnie") {
        append(" Ronnie", false, 0.0, 0.0);
        remaining = rest;
    }
    while let Some(index) = remaining.find("“Hear Yourself”") {
        append(&remaining[..index], false, 0.0, 0.0);
        append("“Hear Yourself”", true, 0.0, 0.5);
        remaining = &remaining[index + "“Hear Yourself”".len()..];
    }
    append(remaining, false, 0.0, 0.0);
    job.wrap.max_width = text_width;
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    let text_y = 11.5;
    let (card_height, button) = alert_layout(
        galley.size().y,
        text_y,
        action_galley.as_ref().map(|galley| galley.size().x + 24.0),
    );
    let (card, _) = ui.allocate_exact_size(egui::vec2(width, card_height), egui::Sense::hover());
    ui.painter().rect_filled(card, 10.0, egui::Color32::BLACK);
    ui.painter().rect_stroke(
        card,
        10.0,
        egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
        egui::StrokeKind::Inside,
    );
    let icon_color = match variant {
        super::BannerVariant::Error => crate::theme::palette::FIRE,
        _ => crate::theme::palette::BLAZE,
    };
    let icon = egui::Rect::from_min_size(
        card.left_top() + egui::vec2(14.0, 13.0),
        egui::Vec2::splat(16.0),
    );
    ui.painter()
        .circle_stroke(icon.center(), 7.2, egui::Stroke::new(1.6, icon_color));
    ui.painter().text(
        icon.center(),
        egui::Align2::CENTER_CENTER,
        "!",
        egui::FontId::new(10.0, egui::FontFamily::Name("arimo-bold".into())),
        icon_color,
    );
    ui.painter().galley(
        card.left_top() + egui::vec2(ALERT_TEXT_LEFT, text_y),
        galley,
        egui::Color32::WHITE,
    );
    let mut clicked = false;
    if let (Some(label), Some(galley), Some(button)) = (action, action_galley, button) {
        let button = button.translate(card.left_top().to_vec2());
        let response = ui.interact(
            button,
            ui.scope_id().with(("alert-action", label)),
            egui::Sense::click(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        ui.painter().rect_stroke(
            button,
            7.0,
            egui::Stroke::new(1.0, egui::Color32::WHITE),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(
            button.center() - galley.size() / 2.0,
            galley,
            egui::Color32::WHITE,
        );
        clicked = response.clicked();
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::{ALERT_TEXT_LEFT, alert_join_needs_space, alert_layout};
    use eframe::egui;

    #[test]
    fn the_action_button_is_below_the_text_at_the_text_column() {
        let text_height = 3.0 * 18.75;
        let text = egui::Rect::from_min_size(
            egui::pos2(ALERT_TEXT_LEFT, 11.5),
            egui::vec2(600.0, text_height),
        );
        let (height, button) = alert_layout(text_height, 11.5, Some(60.0));
        let button = button.expect("a card with an action has a button");

        assert!(!button.intersects(text));
        assert_eq!(button.left(), ALERT_TEXT_LEFT);
        assert!(button.bottom() < height);
    }

    #[test]
    fn a_card_without_an_action_keeps_one_row() {
        assert_eq!(alert_layout(18.75, 11.5, None), (42.75, None));
    }

    #[test]
    fn the_headline_and_the_body_have_one_space_between_them() {
        assert!(alert_join_needs_space(
            "The audio device couldn’t be opened.",
            "ATH-M50x is in use."
        ));
        assert!(!alert_join_needs_space("Feedback risk.", " Your mic"));
        assert!(!alert_join_needs_space("", "Notice."));
    }
}
