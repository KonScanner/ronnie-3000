//! Plugin preferences, machine defaults, calibration, and oversampling.

use std::sync::Arc;

use eframe::egui;
use ronie_core::params::{ParameterCatalog, ParameterId, ParameterValue};

use crate::backend::{GesturePhase, NavigationAction, UiAction, UiBackend, UiError};

use super::{MidiMapController, forms};

/// Plugin settings controller using only typed backend mutations.
pub struct PluginSettingsController {
    backend: Arc<dyn UiBackend>,
    midi: MidiMapController,
    host_action_failed: bool,
}

impl PluginSettingsController {
    /// Creates a controller over shared processor and UI preference state.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>) -> Self {
        let midi = MidiMapController::new(backend.clone());
        Self {
            backend,
            midi,
            host_action_failed: false,
        }
    }

    /// Mutable MIDI mapping controller with persistent picker state.
    pub fn midi_mut(&mut self) -> &mut MidiMapController {
        &mut self.midi
    }

    /// Stable section names in display order.
    #[must_use]
    pub fn section_names(&self) -> Vec<&'static str> {
        vec![
            "Show Info Bar",
            "Show Gate",
            "Show Pitch Shift",
            "NAM A2 Size",
            "Per-Block NAM Size",
            "Per-Block Normalization",
            "Calibration",
            "Oversampling",
            "Multi-Core Processing",
            "MIDI Mapping",
            "Presets",
            "Diagnostics",
        ]
    }

    /// Sets one view preference.
    ///
    /// # Errors
    /// Returns a preference persistence error.
    pub fn set_preference(&self, key: &str, value: bool) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::SetPreference {
            key: key.to_owned(),
            value,
        })?;
        Ok(())
    }

    /// Enables or disables input calibration and remembers the machine default.
    ///
    /// # Errors
    /// Returns a parameter or persistence error.
    pub fn set_calibrated(&self, enabled: bool) -> Result<(), UiError> {
        self.set_machine_parameter("calibrateInput", if enabled { 1.0 } else { 0.0 })
    }

    /// Enables or disables oversampling and remembers the machine default.
    ///
    /// # Errors
    /// Returns a parameter or persistence error.
    pub fn set_oversampling(&self, enabled: bool) -> Result<(), UiError> {
        self.set_machine_parameter("osEnabled", if enabled { 1.0 } else { 0.0 })
    }

    /// Selects 2x, 4x, or 8x oversampling and remembers the machine default.
    ///
    /// # Errors
    /// Returns an invalid-choice, parameter, or persistence error.
    pub fn set_oversampling_factor(&self, factor: u8) -> Result<(), UiError> {
        if factor > 2 {
            return Err(UiError::InvalidAction(
                "oversampling factor must be 0, 1, or 2".to_owned(),
            ));
        }
        self.set_machine_parameter("osFactor", f32::from(factor))
    }

    fn set_machine_parameter(&self, id: &'static str, value: f32) -> Result<(), UiError> {
        let id = ParameterId(id);
        self.backend.dispatch(UiAction::SetParameter {
            id,
            value: ParameterValue(value),
            gesture: GesturePhase::Change,
        })?;
        self.backend.dispatch(UiAction::PersistMachineDefault(id))?;
        Ok(())
    }

    /// Selects full or lite models for new blocks.
    ///
    /// # Errors
    /// Returns a chain-state error.
    pub fn set_nam_full_size(&self, full: bool) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::SetNamFullSize(full))?;
        Ok(())
    }

    /// Enables or disables multicore processing.
    ///
    /// # Errors
    /// Returns a chain-state error.
    pub fn set_multi_core(&self, enabled: bool) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::SetMultiCore(enabled))?;
        Ok(())
    }

    /// Current calibration text draft.
    #[must_use]
    pub fn calibration_draft(&self) -> Option<String> {
        self.backend.calibration_draft()
    }

    /// Replaces the calibration draft without changing processor state.
    ///
    /// # Errors
    /// Returns a backend error when the draft cannot be retained.
    pub fn edit_calibration_draft(&self, text: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::SetCalibrationDraft(Some(text.to_owned())))?;
        Ok(())
    }

    /// Commits a valid dBu draft, clamped to the parameter's real-unit range.
    ///
    /// # Errors
    /// Returns a parameter or persistence error.
    pub fn commit_calibration_draft(&self) -> Result<(), UiError> {
        let Some(draft) = self.calibration_draft() else {
            return Ok(());
        };
        let valid = !draft.trim().is_empty()
            && draft.chars().all(|character| {
                character.is_ascii_digit() || matches!(character, '-' | '+' | '.')
            });
        if valid && let Ok(value) = draft.trim().parse::<f32>() {
            let spec = ParameterCatalog::v1()
                .iter()
                .find(|spec| spec.id == ParameterId("inputCalibrationLevel"))
                .ok_or(UiError::UnknownParameter(ParameterId(
                    "inputCalibrationLevel",
                )))?;
            self.set_machine_parameter("inputCalibrationLevel", value.clamp(spec.min, spec.max))?;
            self.backend.dispatch(UiAction::SetCalibrationDraft(None))?;
        }
        Ok(())
    }

    /// Whether the latest request to the file manager failed.
    #[must_use]
    pub const fn host_action_failed(&self) -> bool {
        self.host_action_failed
    }

    /// Opens the user preset folder through the host navigation boundary.
    ///
    /// # Errors
    /// Returns a host navigation error.
    pub fn open_presets_folder(&self) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Navigate(NavigationAction::OpenPresetsFolder))?;
        Ok(())
    }

    /// Renders the complete plugin settings page.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        for (index, (label, description, key, default)) in [
            (
                "Show Info Bar",
                "Shows help text on hover explaining what elements do, plus keyboard shortcuts and a CPU meter.",
                "t3k.showHints",
                true,
            ),
            (
                "Show Gate",
                "Reduces unwanted noise like hum and hiss while you're playing.",
                "t3k.showGateControl",
                true,
            ),
            (
                "Show Pitch Shift",
                "Transpose your instrument up or down without retuning or use a MIDI expression pedal for whammy-style effects.",
                "t3k.showPitchControl",
                true,
            ),
            (
                "Per-Block NAM Size",
                "Adds a LITE/FULL toggle to every NAM block. When off, a block only shows its size if it differs from your default.",
                "t3k.showBlockSizeControl",
                false,
            ),
            (
                "Per-Block Normalization",
                "Each block has normalization enabled, which levels output for consistent volume across signal blocks. Turning this on reveals an optional control that lets you disable normalization per block.",
                "t3k.showBlockNormalizeControl",
                false,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if index == 3 {
                forms::field_heading(
                    ui,
                    "NAM A2 Size",
                    "Default size for new NAM blocks. Existing blocks keep their own, so presets load as saved.",
                );
                let full = self.backend.chain_state(0).nam_full_size;
                if forms::radio_option(ui, "A2-Lite", "Sounds great and uses less CPU", !full) {
                    let _ = self.set_nam_full_size(false);
                }
                ui.add_space(forms::CONTROL_GAP);
                if forms::radio_option(ui, "A2-Full", "Maximum accuracy model", full) {
                    let _ = self.set_nam_full_size(true);
                }
                ui.add_space(forms::SECTION_GAP);
            }
            let mut value = self.backend.preference(key).unwrap_or(default);
            if forms::toggle_row(ui, label, description, &mut value) {
                let _ = self.set_preference(key, value);
            }
            if value && key == "t3k.showBlockSizeControl" {
                ui.add_space(forms::CONTROL_GAP);
                forms::tip(ui, "Look for LITE/FULL next to each block's power button.");
            }
            if value && key == "t3k.showBlockNormalizeControl" {
                ui.add_space(forms::CONTROL_GAP);
                forms::tip(
                    ui,
                    "Normalization is now controlled per block. Look for the = icon on each block, enabled by default.",
                );
            }
            ui.add_space(forms::SECTION_GAP);
        }

        let calibrated = self
            .backend
            .parameter(ParameterId("calibrateInput"))
            .is_some_and(|value| value.0 > 0.5);
        let mut enabled = calibrated;
        if forms::toggle_row_with_offsets(
            ui,
            "Calibration",
            "Matches your input level to the capture's original recording level, for accurate gain staging.",
            &mut enabled,
            20.3,
            forms::ToggleOffsets {
                heading: 0.5,
                description: 1.5,
                toggle: -1.5,
            },
        ) {
            let _ = self.set_calibrated(enabled);
        }
        if calibrated {
            ui.add_space(forms::CONTROL_GAP + 0.5);
            forms::field(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut draft = self.calibration_draft().unwrap_or_else(|| {
                        self.backend
                            .parameter(ParameterId("inputCalibrationLevel"))
                            .map_or_else(|| "12.0".to_owned(), |value| format!("{:.1}", value.0))
                    });
                    let width = (ui.available_width() - 52.0).max(40.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut draft)
                            .hint_text("Value")
                            .desired_width(width)
                            .margin(egui::Margin::ZERO)
                            .frame(egui::Frame::new()),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            "Input calibration level".to_owned(),
                        )
                    });
                    if response.changed() {
                        let _ = self.edit_calibration_draft(&draft);
                    }
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        let _ = self.commit_calibration_draft();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("dBu")
                                .font(egui::FontId::new(
                                    14.0,
                                    egui::FontFamily::Name("arimo-bold".into()),
                                ))
                                .color(crate::theme::palette::TEXT_MUTED),
                        );
                    });
                });
            });
            ui.add_space(forms::CONTROL_GAP);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                forms::help(
                    ui,
                    "Set the dBu level that matches your DAW's max digital level. Typical values: +12 dBu (professional gear), +4 dBu (semi-pro). ",
                );
                if forms::text_action(ui, "Learn More").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(
                        "https://neural-amp-modeler.readthedocs.io/en/latest/tutorials/calibration.html",
                    ));
                }
            });
            ui.add_space(forms::CONTROL_GAP);
            forms::gauge_tip(ui);
            ui.add_space(forms::CONTROL_GAP - 1.0);
            forms::help(
                ui,
                "When one calibrated NAM feeds another, output calibration data also sets the handoff level between them.",
            );
        }
        ui.add_space(forms::SECTION_GAP + 0.5);

        let oversampling = self
            .backend
            .parameter(ParameterId("osEnabled"))
            .is_some_and(|value| value.0 > 0.5);
        let mut enabled = oversampling;
        if forms::toggle_row_with_offsets(
            ui,
            "Oversampling",
            "Reduces aliasing. Higher rates improve quality but use more CPU.",
            &mut enabled,
            20.3,
            forms::ToggleOffsets {
                heading: 2.0,
                description: 2.0,
                toggle: -2.0,
            },
        ) {
            let _ = self.set_oversampling(enabled);
        }
        if oversampling {
            ui.add_space(10.0);
            forms::help(ui, "Rate");
            ui.add_space(8.0);
            let mut factor = self
                .backend
                .parameter(ParameterId("osFactor"))
                .map_or(0, |value| value.0.round() as u8);
            let selected = match factor {
                1 => "4X",
                2 => "8X",
                _ => "2X - Default",
            };
            ui.add_space(0.5);
            let response = forms::select(
                ui,
                "oversampling-rate",
                egui::RichText::new(selected).size(14.0),
                false,
                |ui| {
                    for (value, label) in [(0, "2X - Default"), (1, "4X"), (2, "8X")] {
                        ui.selectable_value(&mut factor, value, label);
                    }
                },
            );
            ui.add_space(-2.5);
            response.response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::ComboBox,
                    true,
                    "Oversampling rate".to_owned(),
                )
            });
            if factor
                != self
                    .backend
                    .parameter(ParameterId("osFactor"))
                    .map_or(0, |value| value.0.round() as u8)
            {
                let _ = self.set_oversampling_factor(factor);
            }
        }
        ui.add_space(forms::SECTION_GAP + 8.5);

        let mut multi_core = self.backend.chain_state(0).multi_core;
        if forms::toggle_row_with_offsets(
            ui,
            "Multi-Core Processing",
            "Spreads the work across CPU cores for more headroom: stereo chains process in parallel, and oversampled NAM models split across cores. Doesn't change the sound.",
            &mut multi_core,
            20.8,
            forms::ToggleOffsets {
                toggle: -2.5,
                ..Default::default()
            },
        ) {
            let _ = self.set_multi_core(multi_core);
        }
        ui.add_space(forms::SECTION_GAP);

        forms::heading(ui, "MIDI Mapping");
        ui.add_space(8.5);
        forms::help(
            ui,
            "Control the plugin from pedals and knobs. Mappings are saved with the plugin and work in your DAW too.",
        );
        ui.add_space(forms::CONTROL_GAP - 0.5);
        self.midi.show(ui);
        ui.add_space(forms::SECTION_GAP);

        forms::field_heading(
            ui,
            "Presets",
            "Your saved presets are files in a folder you can back up, share, or copy to another computer.",
        );
        if forms::text_action(ui, "Open presets folder").clicked() {
            self.host_action_failed = self.open_presets_folder().is_err();
        }
        ui.add_space(forms::SECTION_GAP);

        forms::field_heading(
            ui,
            "Diagnostics",
            "Copy recent diagnostic logs to the clipboard and paste them into a bug report.",
        );
        if forms::cta(ui, "Copy Logs").clicked() {
            let _ = self
                .backend
                .dispatch(UiAction::Navigate(NavigationAction::CopyLogs));
            if let Some(text) = self.backend.diagnostic_log() {
                ui.ctx().copy_text(text);
            }
        }
        ui.add_space(forms::CONTROL_GAP);
        if forms::text_action(ui, "Reveal log file on disk").clicked() {
            self.host_action_failed = self
                .backend
                .dispatch(UiAction::Navigate(NavigationAction::RevealLogs))
                .is_err();
        }
        if self.host_action_failed {
            ui.add_space(forms::CONTROL_GAP);
            forms::help(ui, "Ronnie could not open the file manager.");
        }
        ui.add_space(forms::SECTION_GAP);
        ui.label(
            egui::RichText::new(format!("Ronnie v{}", env!("CARGO_PKG_VERSION")))
                .size(12.0)
                .color(crate::theme::palette::TEXT_SUBTLE),
        );
        ui.add_space(20.0);
    }
}
