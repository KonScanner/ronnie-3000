//! MIDI mapping snapshots, target catalog, learning, and typed-CC workflows.

use std::sync::Arc;

use eframe::egui;
use ronie_core::{MidiMapping, MidiSource};

use crate::backend::{MidiAction, UiAction, UiBackend, UiError};

/// Host-neutral MIDI mapping and learn state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiMapSnapshot {
    /// Channel filter: 0 is omni, 1–16 selects one channel.
    pub channel: u8,
    /// Target currently armed for learning, or empty.
    pub learn_target_id: String,
    /// Session mappings in stable order.
    pub mappings: Vec<MidiMapping>,
    /// UI-only typed CC draft, retained across view rebuilds.
    pub cc_draft: Option<String>,
}

/// Behavior assigned to a mappable target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiTargetKind {
    /// One-shot action.
    Trigger,
    /// Boolean state toggle.
    Toggle,
    /// Absolute continuous value.
    Continuous,
}

/// One target offered by the MIDI picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiTarget {
    /// Stable target identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Display group.
    pub group: String,
    /// Mapping behavior.
    pub kind: MidiTargetKind,
}

/// Complete Reference-compatible target catalog.
#[must_use]
pub fn mappable_targets() -> Vec<MidiTarget> {
    let mut targets = Vec::new();
    let mut add = |id: &str, name: &str, group: &str, kind: MidiTargetKind| {
        targets.push(MidiTarget {
            id: id.to_owned(),
            name: name.to_owned(),
            group: group.to_owned(),
            kind,
        });
    };
    for (id, name, group, kind) in [
        (
            "presetPrevious",
            "Previous Preset",
            "Presets",
            MidiTargetKind::Trigger,
        ),
        (
            "presetNext",
            "Next Preset",
            "Presets",
            MidiTargetKind::Trigger,
        ),
        (
            "inputLevel",
            "Input Gain",
            "Global",
            MidiTargetKind::Continuous,
        ),
        (
            "outputLevel",
            "Output Level",
            "Global",
            MidiTargetKind::Continuous,
        ),
        (
            "outputBalance",
            "Output Balance",
            "Global",
            MidiTargetKind::Continuous,
        ),
        (
            "gateEnabled",
            "Gate Power",
            "Noise Gate",
            MidiTargetKind::Toggle,
        ),
        (
            "gateThreshold",
            "Gate Threshold",
            "Noise Gate",
            MidiTargetKind::Continuous,
        ),
        (
            "gateRelease",
            "Gate Release",
            "Noise Gate",
            MidiTargetKind::Continuous,
        ),
        (
            "gateHold",
            "Gate Hold",
            "Noise Gate",
            MidiTargetKind::Continuous,
        ),
        (
            "gateRange",
            "Gate Range",
            "Noise Gate",
            MidiTargetKind::Continuous,
        ),
        (
            "pitchEnabled",
            "Pitch Power",
            "Pitch Shift",
            MidiTargetKind::Toggle,
        ),
        (
            "pitchSemitones",
            "Pitch",
            "Pitch Shift",
            MidiTargetKind::Continuous,
        ),
        (
            "pitchStep",
            "Pitch Step",
            "Pitch Shift",
            MidiTargetKind::Toggle,
        ),
        (
            "pitchTonality",
            "Pitch Tonality",
            "Pitch Shift",
            MidiTargetKind::Continuous,
        ),
        (
            "toneEqEnabled",
            "Tone Stack Power",
            "Tone Stack",
            MidiTargetKind::Toggle,
        ),
        ("toneBass", "Bass", "Tone Stack", MidiTargetKind::Continuous),
        ("toneMid", "Mid", "Tone Stack", MidiTargetKind::Continuous),
        (
            "toneTreble",
            "Treble",
            "Tone Stack",
            MidiTargetKind::Continuous,
        ),
        (
            "spreadEnabled",
            "Spread Power",
            "Spread",
            MidiTargetKind::Toggle,
        ),
        (
            "spreadOffset",
            "Spread Offset",
            "Spread",
            MidiTargetKind::Continuous,
        ),
        (
            "spreadWobble",
            "Spread Wobble",
            "Spread",
            MidiTargetKind::Continuous,
        ),
        (
            "spreadWobbleEnabled",
            "Spread Wobble Power",
            "Spread",
            MidiTargetKind::Toggle,
        ),
        (
            "spreadCrossover",
            "Spread Crossover",
            "Spread",
            MidiTargetKind::Continuous,
        ),
        (
            "spreadCrossoverEnabled",
            "Spread Crossover Power",
            "Spread",
            MidiTargetKind::Toggle,
        ),
        (
            "spreadDiffuseEnabled",
            "Spread Diffuse Power",
            "Spread",
            MidiTargetKind::Toggle,
        ),
        (
            "stereoEnabled",
            "Stereo Mode",
            "Stereo",
            MidiTargetKind::Toggle,
        ),
        (
            "alignEnabled",
            "Align Power",
            "Align",
            MidiTargetKind::Toggle,
        ),
        (
            "alignOffset",
            "Align Offset",
            "Align",
            MidiTargetKind::Continuous,
        ),
        (
            "alignWobble",
            "Align Wobble",
            "Align",
            MidiTargetKind::Continuous,
        ),
        (
            "alignWobbleEnabled",
            "Align Wobble Power",
            "Align",
            MidiTargetKind::Toggle,
        ),
        (
            "alignCrossover",
            "Align Crossover",
            "Align",
            MidiTargetKind::Continuous,
        ),
        (
            "alignCrossoverEnabled",
            "Align Crossover Power",
            "Align",
            MidiTargetKind::Toggle,
        ),
        (
            "alignDiffuseEnabled",
            "Align Diffuse Power",
            "Align",
            MidiTargetKind::Toggle,
        ),
        (
            "chainPanLeft",
            "Pan L",
            "Stereo",
            MidiTargetKind::Continuous,
        ),
        (
            "chainPanRight",
            "Pan R",
            "Stereo",
            MidiTargetKind::Continuous,
        ),
        ("chainSoloLeft", "Solo L", "Stereo", MidiTargetKind::Toggle),
        ("chainSoloRight", "Solo R", "Stereo", MidiTargetKind::Toggle),
        (
            "chainInvertLeft",
            "Invert L",
            "Stereo",
            MidiTargetKind::Toggle,
        ),
        (
            "chainInvertRight",
            "Invert R",
            "Stereo",
            MidiTargetKind::Toggle,
        ),
    ] {
        add(id, name, group, kind);
    }
    for index in 1..=5 {
        add(
            &format!("block{index}Power"),
            &format!("Block {index} Power"),
            "Chain",
            MidiTargetKind::Toggle,
        );
    }
    for index in 1..=5 {
        add(
            &format!("rightBlock{index}Power"),
            &format!("Block {index} Power"),
            "Chain R",
            MidiTargetKind::Toggle,
        );
    }
    targets
}

/// MIDI mapping and learn controller.
pub struct MidiMapController {
    backend: Arc<dyn UiBackend>,
    picker_open: bool,
}

impl MidiMapController {
    /// Creates a controller over shared backend MIDI state.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>) -> Self {
        Self {
            backend,
            picker_open: false,
        }
    }

    /// Latest channel, mappings, learn target, and draft.
    #[must_use]
    pub fn snapshot(&self) -> MidiMapSnapshot {
        self.backend.midi_map()
    }

    /// Unmapped targets that exist in the current chain.
    #[must_use]
    pub fn available_targets(&self) -> Vec<MidiTarget> {
        let snapshot = self.snapshot();
        let chain = self.backend.chain_state(0).chain;
        let left_count = chain.left.iter().filter(|block| !block.is_insert).count();
        let right_count = chain.right.iter().filter(|block| !block.is_insert).count();
        mappable_targets()
            .into_iter()
            .filter(|target| {
                !snapshot
                    .mappings
                    .iter()
                    .any(|mapping| mapping.target_id == target.id)
            })
            .filter(|target| {
                if let Some(index) = target
                    .id
                    .strip_prefix("rightBlock")
                    .and_then(|id| id.strip_suffix("Power"))
                    .and_then(|id| id.parse::<usize>().ok())
                {
                    chain.stereo_enabled && index > 0 && index <= right_count
                } else if let Some(index) = target
                    .id
                    .strip_prefix("block")
                    .and_then(|id| id.strip_suffix("Power"))
                    .and_then(|id| id.parse::<usize>().ok())
                {
                    index > 0 && index <= left_count
                } else {
                    true
                }
            })
            .collect()
    }

    /// Whether the target picker is open.
    #[must_use]
    pub const fn picker_open(&self) -> bool {
        self.picker_open
    }

    /// Opens the target picker without changing mapping state.
    pub fn open_picker(&mut self) {
        self.picker_open = true;
    }

    /// Arms one target from the picker.
    ///
    /// # Errors
    /// Returns a backend MIDI error.
    pub fn select_target(&mut self, target_id: &str) -> Result<(), UiError> {
        self.start_learn(target_id)
    }

    /// Starts or restarts learning for one target.
    ///
    /// # Errors
    /// Returns a backend MIDI error.
    pub fn start_learn(&mut self, target_id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Midi(MidiAction::StartLearn(target_id.to_owned())))?;
        self.picker_open = false;
        Ok(())
    }

    /// Cancels learning without changing an existing mapping.
    ///
    /// # Errors
    /// Returns a backend MIDI error.
    pub fn cancel_learn(&mut self) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Midi(MidiAction::CancelLearn))?;
        Ok(())
    }

    /// Learns one CC number.
    ///
    /// # Errors
    /// Returns a backend MIDI error.
    pub fn learn_cc(&mut self, number: u8) -> Result<(), UiError> {
        self.learn(number, MidiSource::Cc)
    }

    /// Learns one note number.
    ///
    /// # Errors
    /// Returns a backend MIDI error.
    pub fn learn_note(&mut self, number: u8) -> Result<(), UiError> {
        self.learn(number, MidiSource::Note)
    }

    fn learn(&mut self, number: u8, source: MidiSource) -> Result<(), UiError> {
        if number > 127 {
            return Err(UiError::InvalidAction(
                "MIDI number must be between 0 and 127".to_owned(),
            ));
        }
        let target_id = self.snapshot().learn_target_id;
        if target_id.is_empty() {
            return Err(UiError::InvalidAction("no MIDI target is armed".to_owned()));
        }
        self.backend
            .dispatch(UiAction::Midi(MidiAction::SetMapping {
                target_id,
                source,
                number,
            }))?;
        Ok(())
    }

    /// Replaces the UI-only CC draft with at most three digits.
    ///
    /// # Errors
    /// Returns a backend error when the draft cannot be retained.
    pub fn edit_draft(&mut self, text: &str) -> Result<(), UiError> {
        let draft: String = text.chars().filter(char::is_ascii_digit).take(3).collect();
        self.backend
            .dispatch(UiAction::Midi(MidiAction::SetDraft(Some(draft))))?;
        Ok(())
    }

    /// Current typed CC draft.
    #[must_use]
    pub fn draft(&self) -> Option<String> {
        self.backend.midi_map().cc_draft
    }

    /// Commits a valid typed CC draft and ignores out-of-range values.
    ///
    /// # Errors
    /// Returns a backend MIDI error.
    pub fn commit_draft(&mut self) -> Result<(), UiError> {
        let Some(draft) = self.backend.midi_map().cc_draft else {
            return Ok(());
        };
        let Ok(number) = draft.parse::<u8>() else {
            return Ok(());
        };
        if number > 127 {
            return Ok(());
        }
        self.learn_cc(number)?;
        self.backend
            .dispatch(UiAction::Midi(MidiAction::SetDraft(None)))?;
        Ok(())
    }

    /// Changes the channel filter.
    ///
    /// # Errors
    /// Returns an invalid-channel or backend error.
    pub fn set_channel(&mut self, channel: u8) -> Result<(), UiError> {
        if channel > 16 {
            return Err(UiError::InvalidAction(
                "MIDI channel must be 0 through 16".to_owned(),
            ));
        }
        self.backend
            .dispatch(UiAction::Midi(MidiAction::SetChannel(channel)))?;
        Ok(())
    }

    /// Removes one target mapping.
    ///
    /// # Errors
    /// Returns an unknown-target or backend error.
    pub fn remove_mapping(&mut self, target_id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::Midi(MidiAction::RemoveMapping(
                target_id.to_owned(),
            )))?;
        Ok(())
    }

    /// Resolves a target's display name.
    #[must_use]
    pub fn target_name(&self, target_id: &str) -> String {
        mappable_targets()
            .into_iter()
            .find(|target| target.id == target_id)
            .map_or_else(|| target_id.to_owned(), |target| target.name)
    }

    /// Resolves a target's group, including the live tone in a block-power row.
    #[must_use]
    pub fn target_context(&self, target_id: &str) -> String {
        let target = mappable_targets()
            .into_iter()
            .find(|target| target.id == target_id);
        let right_index = target_id
            .strip_prefix("rightBlock")
            .and_then(|id| id.strip_suffix("Power"))
            .and_then(|id| id.parse::<usize>().ok());
        let left_index = target_id
            .strip_prefix("block")
            .and_then(|id| id.strip_suffix("Power"))
            .and_then(|id| id.parse::<usize>().ok());
        let state = self.backend.chain_state(0);
        let chain = &state.chain;
        if let Some((index, right)) = right_index
            .map(|index| (index, true))
            .or_else(|| left_index.map(|index| (index, false)))
        {
            let lane = if right {
                "Chain R"
            } else if chain.stereo_enabled {
                "Chain L"
            } else {
                "Chain"
            };
            if right && !chain.stereo_enabled {
                return format!("{lane} · Stereo off");
            }
            let blocks = if right { &chain.right } else { &chain.left };
            let title = blocks
                .iter()
                .filter(|block| !block.is_insert)
                .nth(index.saturating_sub(1))
                .and_then(|block| block.tone.as_ref())
                .map_or("Empty slot", |tone| tone.title.as_str());
            return format!("{lane} · {title}");
        }
        target.map_or_else(String::new, |target| target.group)
    }

    /// Formats a mapping's CC or note source.
    #[must_use]
    pub fn source_label(&self, mapping: &MidiMapping) -> String {
        match mapping.source {
            MidiSource::Cc => format!("CC {}", mapping.number),
            MidiSource::Note => format!("Note {}", note_name(mapping.number)),
        }
    }

    /// Formats a mapping's trigger, toggle, or absolute behavior.
    #[must_use]
    pub fn behavior_label(&self, mapping: &MidiMapping) -> String {
        let kind = mappable_targets()
            .into_iter()
            .find(|target| target.id == mapping.target_id)
            .map(|target| target.kind);
        match (kind, mapping.source) {
            (Some(MidiTargetKind::Trigger), _) => "Trigger".to_owned(),
            (Some(MidiTargetKind::Toggle), _) | (_, MidiSource::Note) => "Toggle".to_owned(),
            _ => "Absolute".to_owned(),
        }
    }

    /// Renders mappings, learning, target selection, and channel filtering.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        const ROW_HEIGHT: f32 = 56.0;
        let snapshot = self.snapshot();
        let learning = snapshot.learn_target_id.as_str();
        let pending = !learning.is_empty()
            && !snapshot
                .mappings
                .iter()
                .any(|mapping| mapping.target_id == learning);
        let row_count = snapshot.mappings.len() + usize::from(pending);
        let card_height = if row_count == 0 {
            70.0
        } else {
            row_count as f32 * ROW_HEIGHT + 2.0 - if learning.is_empty() { 0.0 } else { 2.0 }
        };
        let (card, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), card_height),
            egui::Sense::hover(),
        );
        let card = if row_count == 0 {
            card.translate(egui::vec2(0.0, 0.5))
        } else {
            card
        };
        let painter = ui.painter_at(card);
        painter.rect_filled(card, 10.0, egui::Color32::BLACK);
        painter.rect_stroke(
            card,
            10.0,
            egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
            egui::StrokeKind::Inside,
        );
        let mut action = MidiRowAction::None;
        if row_count == 0 {
            ui.place(
                card.shrink2(egui::vec2(20.0, 12.0)),
                egui::Label::new(
                    egui::RichText::new(
                        "No mappings yet. Choose a control, then move it on your MIDI device or type its CC number.",
                    )
                    .font(egui::FontId::new(
                        12.0,
                        egui::FontFamily::Name("arimo-italic".into()),
                    ))
                    .color(crate::theme::palette::TEXT_SUBTLE),
                )
                .halign(egui::Align::Center)
                .wrap(),
            );
        } else {
            for (index, mapping) in snapshot.mappings.iter().enumerate() {
                let row = egui::Rect::from_min_size(
                    egui::pos2(
                        card.left() + 1.0,
                        card.top() + 1.0 + index as f32 * ROW_HEIGHT,
                    ),
                    egui::vec2(
                        card.width() - 2.0,
                        if mapping.target_id == learning {
                            ROW_HEIGHT - 2.0
                        } else {
                            ROW_HEIGHT
                        },
                    ),
                );
                let next = if mapping.target_id == learning {
                    learning_row(ui, row, index > 0, self, &snapshot)
                } else {
                    mapping_row(ui, row, index > 0, self, mapping)
                };
                if !matches!(next, MidiRowAction::None) {
                    action = next;
                }
            }
            if pending {
                let row = egui::Rect::from_min_size(
                    egui::pos2(
                        card.left() + 1.0,
                        card.top() + 1.0 + snapshot.mappings.len() as f32 * ROW_HEIGHT,
                    ),
                    egui::vec2(card.width() - 2.0, ROW_HEIGHT - 2.0),
                );
                action = learning_row(ui, row, !snapshot.mappings.is_empty(), self, &snapshot);
            }
        }
        match action {
            MidiRowAction::None => {}
            MidiRowAction::Relearn(target) => {
                let _ = self.start_learn(&target);
            }
            MidiRowAction::Remove(target) => {
                let _ = self.remove_mapping(&target);
            }
            MidiRowAction::Draft(draft) => {
                let _ = self.edit_draft(&draft);
            }
            MidiRowAction::CommitDraft => {
                let _ = self.commit_draft();
            }
            MidiRowAction::CancelLearn => {
                let _ = self.cancel_learn();
            }
        }

        ui.add_space(if learning.is_empty() {
            super::forms::CONTROL_GAP
        } else {
            super::forms::CONTROL_GAP - 1.0
        });
        let targets = self.available_targets();
        if !targets.is_empty() {
            let response = super::forms::select_button(
                ui,
                "Choose a control",
                "Control to map",
                true,
                self.picker_open,
            );
            if response.clicked() {
                self.picker_open = !self.picker_open;
            }
            if self.picker_open {
                let width = response.rect.width();
                let height = (targets.len() as f32 * 54.0).min(264.0) + 2.0;
                let picker = super::popover::Popover::new(
                    egui::Id::unique("midi-target-picker"),
                    super::popover::Placement::Below {
                        anchor: response.rect,
                        gap: 4.0,
                    },
                    egui::vec2(width, height),
                    super::popover::Surface::Frame(
                        egui::Frame::new()
                            .fill(egui::Color32::BLACK)
                            .stroke(egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE))
                            .corner_radius(6.0)
                            .inner_margin(0),
                    ),
                )
                .trigger(super::popover::Trigger::from(&response))
                .show(ui, |ui, _panel| {
                    ui.set_min_size(egui::vec2(width, height));
                    ui.set_max_size(egui::vec2(width, height));
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(height - 2.0)
                        .max_height(height - 2.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let mut selected = None;
                            for target in &targets {
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(width - 2.0, 54.0),
                                    egui::Sense::click(),
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::Role::Button,
                                        true,
                                        target.name.clone(),
                                    )
                                });
                                if response.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        0.0,
                                        egui::Color32::from_white_alpha(25),
                                    );
                                }
                                let painter = ui.painter_at(rect);
                                painter.text(
                                    rect.left_top() + egui::vec2(16.0, 13.0),
                                    egui::Align2::LEFT_TOP,
                                    &target.name,
                                    egui::FontId::proportional(14.0),
                                    egui::Color32::WHITE,
                                );
                                painter.text(
                                    rect.left_top() + egui::vec2(16.0, 35.0),
                                    egui::Align2::LEFT_TOP,
                                    self.target_context(&target.id),
                                    egui::FontId::proportional(11.0),
                                    crate::theme::palette::TEXT_SUBTLE,
                                );
                                if response.clicked() {
                                    selected = Some(target.id.clone());
                                }
                            }
                            selected
                        })
                        .inner
                });
                if let Some(target) = picker.pick {
                    let _ = self.select_target(&target);
                }
                if picker.dismissed.is_some() {
                    self.picker_open = false;
                }
            }
        }
        ui.add_space(if learning.is_empty() {
            super::forms::CONTROL_GAP
        } else {
            super::forms::CONTROL_GAP + 0.5
        });
        super::forms::help(
            ui,
            "Map Previous / Next Preset to step through presets from CC or note buttons. Program change messages also switch presets directly; each preset shows its PC number in the preset browser.",
        );
        ui.add_space(if learning.is_empty() {
            super::forms::SECTION_GAP
        } else {
            super::forms::SECTION_GAP - 2.0
        });
        super::forms::field_heading(ui, "MIDI Channel", "Omni listens on every channel.");
        let mut channel = snapshot.channel;
        let selected = if channel == 0 {
            "Omni".to_owned()
        } else {
            format!("Channel {channel}")
        };
        let response = super::forms::select(
            ui,
            "midi-channel",
            egui::RichText::new(selected).size(14.0),
            false,
            |ui| {
                for value in 0..=16 {
                    ui.selectable_value(
                        &mut channel,
                        value,
                        if value == 0 {
                            "Omni".to_owned()
                        } else {
                            format!("Channel {value}")
                        },
                    );
                }
            },
        );
        response.response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "MIDI channel".to_owned())
        });
        if channel != snapshot.channel {
            let _ = self.set_channel(channel);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MidiRowAction {
    None,
    Relearn(String),
    Remove(String),
    Draft(String),
    CommitDraft,
    CancelLearn,
}

fn mapping_row(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    divided: bool,
    controller: &MidiMapController,
    mapping: &MidiMapping,
) -> MidiRowAction {
    paint_target(ui, rect, divided, controller, &mapping.target_id);
    let remove_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 27.0, rect.center().y),
        egui::Vec2::splat(26.0),
    );
    let relearn_rect = remove_rect.translate(egui::vec2(-28.0, 0.0));
    let relearn = midi_icon(ui, relearn_rect, "Re-learn", false);
    let remove = midi_icon(ui, remove_rect, "Remove", true);
    let source_right = relearn_rect.left() - 14.0;
    let painter = ui.painter_at(rect);
    painter.text(
        egui::pos2(source_right, rect.top() + 11.0),
        egui::Align2::RIGHT_TOP,
        controller.source_label(mapping),
        egui::FontId::monospace(13.0),
        egui::Color32::WHITE,
    );
    painter.text(
        egui::pos2(source_right, rect.top() + 32.0),
        egui::Align2::RIGHT_TOP,
        controller.behavior_label(mapping),
        egui::FontId::proportional(11.0),
        crate::theme::palette::TEXT_SUBTLE,
    );
    if remove.clicked() {
        MidiRowAction::Remove(mapping.target_id.clone())
    } else if relearn.clicked() {
        MidiRowAction::Relearn(mapping.target_id.clone())
    } else {
        MidiRowAction::None
    }
}

fn learning_row(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    divided: bool,
    controller: &MidiMapController,
    snapshot: &MidiMapSnapshot,
) -> MidiRowAction {
    paint_target(ui, rect, divided, controller, &snapshot.learn_target_id);
    let cancel_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 66.0, rect.center().y - 13.0),
        egui::vec2(52.0, 26.0),
    );
    let cc_rect = egui::Rect::from_min_size(
        egui::pos2(cancel_rect.left() - 66.5, rect.center().y - 13.5),
        egui::vec2(54.0, 26.0),
    );
    let listen_right = cc_rect.left() - 14.0;
    let listen_left = listen_right - 88.0;
    let painter = ui.painter_at(rect);
    painter.circle_filled(
        egui::pos2(listen_left + 3.5, rect.center().y),
        3.5,
        crate::theme::palette::BLAZE,
    );
    painter.text(
        egui::pos2(listen_left + 15.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Listening…",
        egui::FontId::proportional(12.0),
        egui::Color32::WHITE,
    );
    let mut draft = snapshot.cc_draft.clone().unwrap_or_default();
    let response = ui.place(
        cc_rect,
        egui::TextEdit::singleline(&mut draft)
            .hint_text("CC #")
            .desired_width(54.0)
            .font(egui::FontId::monospace(12.0))
            .align(egui::Align2::CENTER_CENTER)
            .margin(egui::Margin::symmetric(6, 4)),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "MIDI CC number".to_owned())
    });
    let cancel = ui.place(
        cancel_rect,
        egui::Button::new(egui::RichText::new("Cancel").font(egui::FontId::new(
            12.0,
            egui::FontFamily::Name("arimo-bold".into()),
        )))
        .frame(false),
    );
    if cancel.clicked() {
        MidiRowAction::CancelLearn
    } else if response.changed() {
        MidiRowAction::Draft(draft)
    } else if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        MidiRowAction::CommitDraft
    } else {
        MidiRowAction::None
    }
}

fn paint_target(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    divided: bool,
    controller: &MidiMapController,
    target_id: &str,
) {
    let painter = ui.painter_at(rect);
    if divided {
        painter.line_segment(
            [rect.left_top(), rect.right_top()],
            egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
        );
    }
    painter.text(
        rect.left_top() + egui::vec2(14.0, 11.0),
        egui::Align2::LEFT_TOP,
        controller.target_name(target_id),
        egui::FontId::new(14.0, egui::FontFamily::Name("arimo-bold".into())),
        egui::Color32::WHITE,
    );
    painter.text(
        rect.left_top() + egui::vec2(14.0, 32.0),
        egui::Align2::LEFT_TOP,
        controller.target_context(target_id),
        egui::FontId::proportional(11.0),
        crate::theme::palette::TEXT_SUBTLE,
    );
}

fn midi_icon(ui: &mut egui::Ui, rect: egui::Rect, label: &str, remove: bool) -> egui::Response {
    let response = ui.interact(
        rect,
        egui::Id::unique((label, rect.center().x.to_bits(), rect.center().y.to_bits())),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.to_owned()));
    let color = if response.hovered() {
        egui::Color32::WHITE
    } else {
        crate::theme::palette::TEXT_MUTED
    };
    let painter = ui.painter_at(rect);
    if remove {
        let stroke = egui::Stroke::new(1.5, color);
        painter.line_segment(
            [
                rect.center() + egui::vec2(-5.0, -5.0),
                rect.center() + egui::vec2(5.0, 5.0),
            ],
            stroke,
        );
        painter.line_segment(
            [
                rect.center() + egui::vec2(-5.0, 5.0),
                rect.center() + egui::vec2(5.0, -5.0),
            ],
            stroke,
        );
    } else {
        let scale = 14.0 / 24.0;
        let point =
            |x: f32, y: f32| rect.center() + egui::vec2((x - 12.0) * scale, (y - 12.0) * scale);
        let mut arc = (0..=36)
            .map(|step| {
                let angle = (180.0 + 270.0 * step as f32 / 36.0).to_radians();
                point(12.0 + 9.0 * angle.cos(), 12.0 + 9.0 * angle.sin())
            })
            .collect::<Vec<_>>();
        arc.extend([point(5.26, 5.74), point(3.0, 8.0)]);
        let stroke = egui::Stroke::new(1.4, color);
        painter.add(egui::Shape::line(arc, stroke));
        painter.add(egui::Shape::line(
            vec![point(3.0, 3.0), point(3.0, 8.0), point(8.0, 8.0)],
            stroke,
        ));
    }
    response
}

fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(note % 12)],
        i32::from(note) / 12 - 1
    )
}
