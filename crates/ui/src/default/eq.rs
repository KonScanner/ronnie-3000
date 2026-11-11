//! Six-band EQ editing, Reference math, and slider/curve rendering.

use std::sync::Arc;

use eframe::egui::{self, Color32, Rect, Sense, Stroke};
use ronie_core::{BlockId, EqBand, EqBandType, EqParams};
use ronie_dsp::block::eq::{EqControls, EqEngine};

use crate::{
    backend::{GesturePhase, UiAction, UiBackend, UiError},
    widgets::knob::{OpenDrag, take_escape},
};

use super::spectrum::SpectrumFeed;

const MIN_FREQ: f64 = 20.0;
const MAX_FREQ: f64 = 20_000.0;
const MAX_GAIN: f64 = 15.0;
const GRAPH_H: f64 = 273.0;
const GRAPH_PAD_Y: f64 = 12.0;

/// EQ editor view selected in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqViewMode {
    /// Six gain faders.
    Sliders,
    /// Frequency response and draggable band dots.
    Curve,
}

/// Keyboard-editable band field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqField {
    /// Center or corner frequency.
    Frequency,
    /// Band gain.
    Gain,
    /// Band quality factor.
    Q,
}

/// The control that holds an open EQ gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragTarget {
    /// The graph dot of one band.
    Dot(usize),
    /// The gain fader of one band.
    Fader(usize),
    /// One field of the selected band.
    Field(EqField),
}

/// One open EQ gesture: a pointer drag, or the edit of one field.
///
/// Everything that the gesture changes is one undo step on the backend.
#[derive(Debug, Clone)]
struct DragState {
    target: DragTarget,
    /// The band that the gesture changes.
    index: usize,
    /// The last pointer position of a graph-dot drag.
    last_x: f64,
    last_y: f64,
    /// The gain motion of the whole fader drag, in dB from the start gain.
    ///
    /// Each frame adds its motion here, so the fader follows the pointer from the press point
    /// and does not jump to it.
    travel: f64,
    /// Whether the backend has the `Begin` of this gesture.
    dispatched: bool,
    /// The settings from before the gesture, which a cancel restores.
    start: EqParams,
}

/// Six-band controller with optimistic edits and visibility-scoped analysis.
pub struct EqEditor {
    backend: Arc<dyn UiBackend>,
    block_id: BlockId,
    params: EqParams,
    selected: usize,
    view: EqViewMode,
    drag: Option<DragState>,
    /// The control of the open gesture and the last pass that showed it.
    open: OpenDrag<DragTarget>,
    spectrum: SpectrumFeed,
}

impl EqEditor {
    /// Creates an editor from one block snapshot.
    ///
    /// # Errors
    /// Returns an unknown-target error when the block does not exist.
    pub fn new(backend: Arc<dyn UiBackend>, block_id: &str) -> Result<Self, UiError> {
        let id = BlockId(block_id.to_owned());
        let block = backend
            .chain_state(0)
            .chain
            .block(&id)
            .cloned()
            .ok_or_else(|| UiError::UnknownTarget(block_id.to_owned()))?;
        Ok(Self {
            backend,
            block_id: id,
            params: block.params.eq,
            selected: 1,
            view: EqViewMode::Sliders,
            drag: None,
            open: OpenDrag::default(),
            spectrum: SpectrumFeed::default(),
        })
    }

    /// Current EQ parameters.
    #[must_use]
    pub const fn params(&self) -> &EqParams {
        &self.params
    }

    /// Applies native state unless a pointer gesture owns the bands.
    ///
    /// A field edit does not own the bands, so a field shows the value that the backend holds.
    pub fn sync_params(&mut self, params: EqParams) {
        if self
            .drag
            .as_ref()
            .is_none_or(|drag| matches!(drag.target, DragTarget::Field(_)))
        {
            self.params = params;
        }
    }

    /// Selected band index.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Selects a band when it exists.
    pub fn select_band(&mut self, index: usize) {
        if index < self.params.bands.len() {
            self.selected = index;
        }
    }

    /// Current editor view.
    #[must_use]
    pub const fn view(&self) -> EqViewMode {
        self.view
    }

    /// Changes between slider and curve views.
    pub fn set_view(&mut self, view: EqViewMode) {
        self.view = view;
    }

    /// Enables or bypasses EQ processing.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.params.enabled = enabled;
        self.write_params(GesturePhase::End);
    }

    /// Moves EQ before or after the model.
    pub fn set_pre(&mut self, pre: bool) {
        self.params.pre = pre;
        self.write_params(GesturePhase::End);
    }

    /// Restores the six default bands without changing enable/pre state.
    pub fn reset(&mut self) {
        self.params.bands = reference_bands();
        self.write_params(GesturePhase::End);
    }

    /// Valid curve shapes for one band position.
    #[must_use]
    pub fn type_options(index: usize) -> Vec<EqBandType> {
        match index {
            0 => vec![EqBandType::Lowcut, EqBandType::Lowshelf],
            5 => vec![EqBandType::Highcut, EqBandType::Highshelf],
            _ => vec![EqBandType::Bell],
        }
    }

    /// Changes the selected band shape.
    pub fn change_type(&mut self, kind: EqBandType) {
        if !Self::type_options(self.selected).contains(&kind) {
            return;
        }
        let Some(band) = self.params.bands.get_mut(self.selected) else {
            return;
        };
        band.kind = kind;
        if let Some(q) = type_default_q(kind) {
            band.q = q;
        }
        self.write_params(GesturePhase::End);
    }

    /// Commits typed Hz or k-notation, clamped between neighboring bands.
    ///
    /// # Errors
    /// Returns an invalid-number or backend error.
    pub fn commit_freq(&mut self, raw: &str) -> Result<(), UiError> {
        let value = parse_freq(raw)
            .ok_or_else(|| UiError::InvalidAction("frequency is not a number".to_owned()))?;
        let (low, high) = self.freq_range(self.selected);
        let Some(band) = self.params.bands.get_mut(self.selected) else {
            return Err(UiError::UnknownTarget("EQ band".to_owned()));
        };
        band.freq_hz = value.clamp(low, high);
        self.write_params(GesturePhase::End);
        Ok(())
    }

    /// Commits typed gain in dB.
    ///
    /// # Errors
    /// Returns an invalid-number, non-gain-band, or backend error.
    pub fn commit_gain(&mut self, raw: &str) -> Result<(), UiError> {
        let value = parse_decimal(raw)
            .ok_or_else(|| UiError::InvalidAction("gain is not a number".to_owned()))?;
        let Some(band) = self.params.bands.get_mut(self.selected) else {
            return Err(UiError::UnknownTarget("EQ band".to_owned()));
        };
        if !has_gain(band.kind) {
            return Err(UiError::InvalidAction("cut bands have no gain".to_owned()));
        }
        band.gain_db = value.clamp(-MAX_GAIN, MAX_GAIN);
        self.write_params(GesturePhase::End);
        Ok(())
    }

    /// Commits typed Q.
    ///
    /// # Errors
    /// Returns an invalid-number or backend error.
    pub fn commit_q(&mut self, raw: &str) -> Result<(), UiError> {
        let value = parse_decimal(raw)
            .ok_or_else(|| UiError::InvalidAction("Q is not a number".to_owned()))?;
        let Some(band) = self.params.bands.get_mut(self.selected) else {
            return Err(UiError::UnknownTarget("EQ band".to_owned()));
        };
        band.q = value.clamp(0.1, 10.0);
        self.write_params(GesturePhase::End);
        Ok(())
    }

    /// Resets one band's effect without changing its frequency.
    pub fn reset_band(&mut self, index: usize) {
        let Some(band) = self.params.bands.get_mut(index) else {
            return;
        };
        if has_gain(band.kind) {
            band.gain_db = 0.0;
        } else {
            band.q = type_default_q(band.kind).unwrap_or(0.71);
        }
        self.selected = index.min(self.params.bands.len().saturating_sub(1));
        self.write_params(GesturePhase::End);
    }

    /// Applies one keyboard increment or decrement.
    pub fn keyboard_step(&mut self, field: EqField, direction: i32, fine: bool) {
        if direction == 0 {
            return;
        }
        let sensitivity = if fine { 0.125 } else { 1.0 };
        let (low, high) = self.freq_range(self.selected);
        let Some(band) = self.params.bands.get_mut(self.selected) else {
            return;
        };
        match field {
            EqField::Frequency => {
                let factor: f64 = if direction > 0 { 1.01 } else { 1.0 / 1.01 };
                let factor = factor.powf(sensitivity);
                band.freq_hz = (band.freq_hz * factor).clamp(low, high);
            }
            EqField::Gain => {
                if has_gain(band.kind) {
                    band.gain_db = (band.gain_db + direction as f64 * 0.5 * sensitivity)
                        .clamp(-MAX_GAIN, MAX_GAIN);
                }
            }
            EqField::Q => {
                let factor: f64 = if direction > 0 { 1.05 } else { 1.0 / 1.05 };
                band.q = (band.q * factor.powf(sensitivity)).clamp(0.1, 10.0);
            }
        }
        self.write_params(GesturePhase::End);
    }

    /// Starts a gesture of `target` on band `index`. A gesture that is still open ends first.
    fn start_drag(&mut self, target: DragTarget, index: usize) {
        self.close_drag(GesturePhase::End);
        self.drag = Some(DragState {
            target,
            index,
            last_x: 0.0,
            last_y: 0.0,
            travel: 0.0,
            dispatched: false,
            start: self.params.clone(),
        });
    }

    /// The control of the open gesture.
    fn drag_target(&self) -> Option<DragTarget> {
        self.drag.as_ref().map(|drag| drag.target)
    }

    /// Sends one write of the open gesture: `Begin` for its first write, and `Change` after it.
    fn write_in_drag(&mut self) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        let phase = if drag.dispatched {
            GesturePhase::Change
        } else {
            drag.dispatched = true;
            GesturePhase::Begin
        };
        self.write_params(phase);
    }

    /// Closes the open gesture with `End` or `Cancel`, and forgets its control.
    fn finish_drag(&mut self, phase: GesturePhase) {
        let _ = self.open.take();
        self.close_drag(phase);
    }

    /// Closes the open gesture on the backend with `End` or `Cancel`.
    ///
    /// A cancel puts back the settings from before the gesture. A gesture that sent nothing
    /// sends nothing, so it makes no undo step.
    fn close_drag(&mut self, phase: GesturePhase) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if !drag.dispatched {
            return;
        }
        if phase == GesturePhase::Cancel {
            self.params = drag.start;
        }
        self.write_params(phase);
    }

    /// Records one frame of the control `target` before its edit.
    ///
    /// A gesture that this frame leaves behind ends first: one whose control a pass did not
    /// show, or another one when `target` begins a gesture.
    fn track(&mut self, target: DragTarget, pass: u64, begins: bool) {
        let begin = begins.then_some(GesturePhase::Begin);
        if self.open.track_phase(&target, pass, begin).stale.is_some() {
            self.close_drag(GesturePhase::End);
        }
    }

    /// Starts a graph-dot gesture.
    pub fn begin_band_drag(&mut self, index: usize, x: f64, y: f64) {
        self.select_band(index);
        self.start_drag(DragTarget::Dot(index), index);
        if let Some(drag) = &mut self.drag {
            drag.last_x = x;
            drag.last_y = y;
        }
    }

    /// Applies a delta-based graph drag with 8× fine control when requested.
    pub fn drag_band_to(&mut self, x: f64, y: f64, fine: bool) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        if !matches!(drag.target, DragTarget::Dot(_)) {
            return;
        }
        let sensitivity = if fine { 0.125 } else { 1.0 };
        let dx = (x - drag.last_x) * sensitivity;
        let d_gain = (y_to_gain(y) - y_to_gain(drag.last_y)) * sensitivity;
        let dy = (y - drag.last_y) * sensitivity;
        drag.last_x = x;
        drag.last_y = y;
        let index = drag.index;
        let (low, high) = self.freq_range(index);
        let Some(band) = self.params.bands.get_mut(index) else {
            return;
        };
        band.freq_hz = norm_to_freq(freq_to_norm(band.freq_hz) + dx).clamp(low, high);
        if has_gain(band.kind) {
            band.gain_db = (band.gain_db + d_gain).clamp(-MAX_GAIN, MAX_GAIN);
        } else {
            band.q = (band.q * (-dy * 0.02).exp()).clamp(0.1, 10.0);
        }
        self.write_in_drag();
    }

    /// Ends a pointer or field gesture. The backend keeps it as one undo step.
    pub fn end_drag(&mut self) {
        self.finish_drag(GesturePhase::End);
    }

    /// Cancels a pointer or field gesture. The backend restores the settings from before it
    /// and keeps no undo step for it.
    pub fn cancel_drag(&mut self) {
        self.finish_drag(GesturePhase::Cancel);
    }

    /// Ends the gesture of a control that `pass` did not show.
    ///
    /// The owner calls this at the end of each of its frames, and also in a pass that does not
    /// show the EQ, so that later edits do not join the gesture.
    pub(crate) fn end_hidden_drag(&mut self, pass: u64) {
        if self.open.take_hidden(pass).is_some() {
            self.close_drag(GesturePhase::End);
        }
    }

    /// Ends the open gesture, for an owner that goes away.
    pub(crate) fn end_open_drag(&mut self) {
        self.finish_drag(GesturePhase::End);
    }

    /// Starts a fader gesture on band `index`. The gain then follows the pointer motion from
    /// the press point, so the fader does not jump to the pointer.
    ///
    /// A band with no gain starts no gesture.
    pub fn begin_fader_drag(&mut self, index: usize) {
        self.select_band(index);
        if self
            .params
            .bands
            .get(index)
            .is_some_and(|band| has_gain(band.kind))
        {
            self.start_drag(DragTarget::Fader(index), index);
        }
    }

    /// Moves the fader of the open fader gesture by `dy` points on a travel of `height` points.
    ///
    /// The first call sends `Begin`, also with no motion. Each later call that changes the gain
    /// sends `Change`.
    pub fn drag_fader(&mut self, dy: f64, height: f64, fine: bool) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        if !matches!(drag.target, DragTarget::Fader(_)) {
            return;
        }
        let index = drag.index;
        let Some(start) = drag.start.bands.get(index).map(|band| band.gain_db) else {
            return;
        };
        let sensitivity = if fine { 0.125 } else { 1.0 };
        let motion = (-dy / height.max(1.0)) * 2.0 * MAX_GAIN * sensitivity;
        // The travel stays inside the range, so the fader turns back at once when the pointer
        // turns back past an end. No travel gives the start gain exactly.
        drag.travel = (start + drag.travel + motion).clamp(-MAX_GAIN, MAX_GAIN) - start;
        let gain = if drag.travel == 0.0 {
            start
        } else {
            start + drag.travel
        };
        let dispatched = drag.dispatched;
        let Some(band) = self.params.bands.get_mut(index) else {
            return;
        };
        if dispatched && band.gain_db == gain {
            return;
        }
        band.gain_db = gain;
        self.write_in_drag();
    }

    /// Sets a fader from an absolute pointer y coordinate, as one undo step.
    pub fn set_slider_gain(&mut self, index: usize, y: f64, top: f64, height: f64) {
        self.select_band(index);
        let Some(band) = self.params.bands.get_mut(index) else {
            return;
        };
        if !has_gain(band.kind) {
            return;
        }
        band.gain_db = slider_gain_at(y, top, height);
        self.write_params(GesturePhase::End);
    }

    /// Applies a relative fader delta.
    pub fn nudge_slider(&mut self, index: usize, dy: f64, height: f64, fine: bool) {
        self.select_band(index);
        let Some(band) = self.params.bands.get_mut(index) else {
            return;
        };
        if !has_gain(band.kind) {
            return;
        }
        let sensitivity = if fine { 0.125 } else { 1.0 };
        let gain = (-dy / height.max(1.0)) * 2.0 * MAX_GAIN * sensitivity;
        band.gain_db = (band.gain_db + gain).clamp(-MAX_GAIN, MAX_GAIN);
        self.write_params(GesturePhase::End);
    }

    /// Neighbor-clamped frequency range for one band.
    #[must_use]
    pub fn freq_range(&self, index: usize) -> (f64, f64) {
        // A preset can hold NaN, and `f64::clamp` panics on a NaN bound.
        let neighbour = |band: Option<usize>| {
            band.and_then(|band| self.params.bands.get(band))
                .map(|band| band.freq_hz)
                .filter(|freq_hz| freq_hz.is_finite())
        };
        let low = neighbour(index.checked_sub(1)).map_or(MIN_FREQ, |freq_hz| freq_hz * 1.02);
        let high = neighbour(index.checked_add(1)).map_or(MAX_FREQ, |freq_hz| freq_hz * 0.98);
        // Neighbours can be closer than the margins allow, and `f64::clamp` panics if
        // the minimum is above the maximum.
        (low, high.max(low))
    }

    /// Combined response in dB using the processing crate's coefficients.
    #[must_use]
    pub fn response_db(&self, frequencies_hz: &[f64], sample_rate: f64) -> Vec<f64> {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let controls = EqControls::from(&self.params);
        let engine = EqEngine::new(sample_rate, &controls);
        let mut coefficients = Vec::new();
        for (index, band) in self.params.bands.iter().enumerate() {
            let active = match band.kind {
                EqBandType::Bell | EqBandType::Lowshelf | EqBandType::Highshelf => {
                    band.gain_db.abs() >= 0.05
                }
                EqBandType::Lowcut | EqBandType::Highcut => true,
            };
            if active {
                coefficients.extend(engine.coefficients(index));
            }
        }
        frequencies_hz
            .iter()
            .map(|frequency| {
                let omega = 2.0 * std::f64::consts::PI * frequency / sample_rate;
                let (cos_w, cos_2w) = (omega.cos(), (2.0 * omega).cos());
                coefficients.iter().fold(0.0, |sum, c| {
                    let (b0, b1, b2, a1, a2) = (
                        c[0] as f64,
                        c[1] as f64,
                        c[2] as f64,
                        c[3] as f64,
                        c[4] as f64,
                    );
                    let numerator = b0 * b0
                        + b1 * b1
                        + b2 * b2
                        + 2.0 * (b0 * b1 + b1 * b2) * cos_w
                        + 2.0 * b0 * b2 * cos_2w;
                    let denominator =
                        1.0 + a1 * a1 + a2 * a2 + 2.0 * (a1 + a1 * a2) * cos_w + 2.0 * a2 * cos_2w;
                    sum + 10.0 * (numerator / denominator.max(1.0e-24)).max(1.0e-24).log10()
                })
            })
            .collect()
    }

    /// Enables or disables the analyzer.
    pub fn set_spectrum_visible(&mut self, visible: bool) {
        if self.spectrum.enabled() == visible {
            return;
        }
        self.spectrum.set_visible(visible);
    }

    /// Whether analysis is enabled.
    #[must_use]
    pub const fn spectrum_enabled(&self) -> bool {
        self.spectrum.enabled()
    }

    /// Publishes control-thread bins while visible.
    pub fn set_spectrum_bins(&mut self, bins: [f32; 64]) {
        self.spectrum.set_bins(bins);
    }

    /// Current bins or the disabled floor.
    #[must_use]
    pub const fn spectrum_bins(&self) -> &[f32; 64] {
        self.spectrum.bins()
    }

    /// Renders the selected EQ editor view.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.spectrum
            .set_bins(self.backend.spectrum_bins(&self.block_id));
        let height = 275.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(320.0), height),
            Sense::hover(),
        );
        ui.add_enabled_ui(self.params.enabled, |ui| match self.view {
            EqViewMode::Sliders => self.show_sliders(ui, rect),
            EqViewMode::Curve => self.show_curve(ui, rect),
        });
    }

    fn show_sliders(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let pass = ui.ctx().cumulative_pass_nr();
        paint_gain_grid(ui, rect);
        self.paint_spectrum(ui, rect);
        let travel = Rect::from_min_size(
            rect.left_top() + egui::vec2(0.0, 23.0),
            egui::vec2(rect.width(), 225.0),
        );
        for index in 0..self.params.bands.len() {
            let Some(band) = self.params.bands.get(index).copied() else {
                continue;
            };
            let column_width = (rect.width() - 32.0) / self.params.bands.len() as f32;
            let center_x = rect.left() + 16.0 + (index as f32 + 0.5) * column_width;
            let track = Rect::from_center_size(
                egui::pos2(center_x, travel.center().y),
                egui::vec2(8.0, travel.height()),
            );
            let editable = has_gain(band.kind);
            let mut painter = ui.painter_at(track);
            if !editable {
                painter.multiply_opacity(0.35);
            }
            painter.rect_filled(track, 0.0, Color32::from_rgba_unmultiplied(50, 50, 50, 204));
            let gain = if editable { band.gain_db } else { 0.0 };
            let fraction = ((gain + MAX_GAIN) / (2.0 * MAX_GAIN)) as f32;
            let thumb_y = travel.top() + travel.height() * (1.0 - fraction);
            if fraction > 0.001 {
                let fill =
                    Rect::from_min_max(egui::pos2(track.left(), thumb_y), track.right_bottom());
                paint_vertical_gradient(
                    &painter,
                    fill,
                    &[
                        (0.0, Color32::WHITE),
                        (0.2, Color32::from_rgba_unmultiplied(220, 220, 220, 230)),
                        (0.4, Color32::from_rgba_unmultiplied(160, 160, 160, 217)),
                        (0.6, Color32::from_rgba_unmultiplied(100, 100, 100, 191)),
                        (0.8, Color32::from_rgba_unmultiplied(50, 50, 50, 153)),
                        (1.0, Color32::from_rgba_unmultiplied(30, 30, 30, 76)),
                    ],
                );
            }
            let thumb = Rect::from_center_size(
                egui::pos2(center_x.round(), thumb_y.round()),
                egui::vec2(30.0, 14.0),
            );
            let mut cap_painter = ui.painter_at(thumb);
            if !editable {
                cap_painter.multiply_opacity(0.35);
            }
            paint_vertical_gradient(
                &cap_painter,
                thumb,
                &[
                    (0.0, Color32::WHITE),
                    (1.0, Color32::from_rgb(214, 214, 219)),
                ],
            );
            cap_painter.rect_stroke(
                thumb,
                0.0,
                Stroke::new(1.0, Color32::BLACK),
                egui::StrokeKind::Inside,
            );
            cap_painter.line_segment(
                [
                    egui::pos2(thumb.left() + 1.0, thumb.center().y),
                    egui::pos2(thumb.right() - 1.0, thumb.center().y),
                ],
                Stroke::new(2.0, Color32::BLACK),
            );
            let hit = thumb.expand(6.0);
            let response = ui.interact(
                hit,
                ui.scope_id().with(("eq-fader", index)),
                Sense::click_and_drag(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Slider,
                    editable,
                    if editable {
                        format!("Band {} gain", index + 1)
                    } else {
                        format!("Band {} gain unavailable", index + 1)
                    },
                )
            });
            let target = DragTarget::Fader(index);
            let begins = response.drag_started() && editable;
            self.track(target, pass, begins);
            let fine = ui.input(|input| input.modifiers.shift);
            let motion = f64::from(response.drag_delta().y);
            if begins {
                self.begin_fader_drag(index);
                self.drag_fader(motion, travel.height() as f64, fine);
            } else if self.drag_target() == Some(target) {
                if response.dragged() {
                    self.drag_fader(motion, travel.height() as f64, fine);
                } else {
                    // egui ends a drag when Escape is pressed, and then the drag is a cancel.
                    // The cancel uses the press, so that it does not also close the page.
                    self.finish_drag(if take_escape(ui) {
                        GesturePhase::Cancel
                    } else {
                        GesturePhase::End
                    });
                }
            }
            if response.double_clicked()
                || (response.clicked() && ui.input(|input| input.modifiers.alt))
            {
                self.reset_band(index);
            }
            let label = format_freq(band.freq_hz);
            let font = egui::FontId::proportional(9.0);
            let color = crate::theme::palette::GRAPH_LABEL;
            let center = egui::pos2(center_x, rect.bottom() - 12.0);
            if index == 0 || index + 1 == self.params.bands.len() {
                let text_width = ui.ctx().fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(label.clone(), font.clone(), color)
                        .rect
                        .width()
                });
                let left = center_x - (text_width + 16.0) / 2.0;
                paint_eq_kind_glyph(
                    ui.painter(),
                    Rect::from_center_size(
                        egui::pos2(left + 6.0, center.y),
                        egui::vec2(12.0, 11.0),
                    ),
                    band.kind,
                    color,
                );
                ui.painter().text(
                    egui::pos2(left + 16.0, center.y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    font,
                    color,
                );
            } else {
                ui.painter()
                    .text(center, egui::Align2::CENTER_CENTER, label, font, color);
            }
        }
    }

    fn show_curve(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let pass = ui.ctx().cumulative_pass_nr();
        paint_curve_grid(ui, rect);
        painter_band_caption(self, ui, rect);
        let painter = ui.painter_at(rect);
        self.paint_spectrum(ui, rect);
        let sample_rate = self.backend.chain_state(0).preparation.host_sample_rate;
        let frequencies: Vec<f64> = (0..800)
            .map(|index| norm_to_freq(index as f64 / 799.0))
            .collect();
        let response = self.response_db(&frequencies, sample_rate);
        let points: Vec<egui::Pos2> = response
            .into_iter()
            .enumerate()
            .map(|(index, gain)| {
                egui::pos2(
                    rect.left() + index as f32 / 799.0 * rect.width(),
                    rect.top()
                        + gain_to_y(gain).clamp(-8.0, GRAPH_H + 8.0) as f32 / GRAPH_H as f32
                            * rect.height(),
                )
            })
            .collect();
        let zero_y = rect.top() + gain_to_y(0.0) as f32 / GRAPH_H as f32 * rect.height();
        let mut area = egui::epaint::Mesh::default();
        for pair in points.windows(2) {
            let [from, to] = pair else {
                continue;
            };
            let base = area.vertices.len() as u32;
            for point in [
                *from,
                *to,
                egui::pos2(to.x, zero_y),
                egui::pos2(from.x, zero_y),
            ] {
                area.vertices.push(egui::epaint::Vertex {
                    pos: point,
                    uv: egui::Pos2::ZERO,
                    color: Color32::from_rgba_unmultiplied(142, 142, 147, 25),
                });
            }
            area.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        painter.add(egui::Shape::mesh(area));
        for pair in points.windows(2) {
            painter.line_segment(
                [pair[0], pair[1]],
                Stroke::new(1.25, crate::theme::palette::TEXT_MUTED),
            );
        }
        for index in 0..self.params.bands.len() {
            let Some(band) = self.params.bands.get(index).copied() else {
                continue;
            };
            let center = egui::pos2(
                rect.left() + freq_to_norm(band.freq_hz) as f32 * rect.width(),
                rect.top()
                    + gain_to_y(if has_gain(band.kind) {
                        band.gain_db
                    } else {
                        0.0
                    }) as f32
                        / GRAPH_H as f32
                        * rect.height(),
            );
            let hit = Rect::from_center_size(center, egui::vec2(18.0, 18.0));
            let response = ui.interact(
                hit,
                ui.scope_id().with(("eq-dot", index)),
                Sense::click_and_drag(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Slider,
                    true,
                    format!("Band {} frequency and gain", index + 1),
                )
            });
            if response.clicked() {
                self.select_band(index);
            }
            if (response.clicked() && ui.input(|input| input.modifiers.alt))
                || response.double_clicked()
            {
                self.reset_band(index);
            }
            let target = DragTarget::Dot(index);
            self.track(target, pass, response.drag_started());
            if response.drag_started() {
                let point = response.interact_pointer_pos().unwrap_or(center);
                self.begin_band_drag(
                    index,
                    (point.x - rect.left()) as f64 / rect.width() as f64,
                    (point.y - rect.top()) as f64 / rect.height() as f64 * GRAPH_H,
                );
            }
            if self.drag_target() == Some(target) {
                if response.dragged() {
                    if let Some(point) = response.interact_pointer_pos() {
                        self.drag_band_to(
                            (point.x - rect.left()) as f64 / rect.width() as f64,
                            (point.y - rect.top()) as f64 / rect.height() as f64 * GRAPH_H,
                            ui.input(|input| input.modifiers.shift),
                        );
                    }
                } else {
                    // egui ends a drag when Escape is pressed, and then the drag is a cancel.
                    self.finish_drag(if take_escape(ui) {
                        GesturePhase::Cancel
                    } else {
                        GesturePhase::End
                    });
                }
            }
            if self.selected == index {
                painter.circle_stroke(center, 9.0, Stroke::new(1.5, Color32::WHITE));
            }
            painter.circle_filled(
                center,
                5.5,
                if self.selected == index {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(184, 184, 190)
                },
            );
            painter.circle_stroke(center, 5.5, Stroke::new(1.5, Color32::BLACK));
        }
        self.show_curve_chips(ui, rect);
    }

    fn show_curve_chips(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let pass = ui.ctx().cumulative_pass_nr();
        let chip_rect = Rect::from_min_size(
            rect.left_top() + egui::vec2(16.0, 227.0),
            egui::vec2(450.0, 20.0),
        );
        let mut chip_ui = ui.new_child(egui::UiBuilder::new().max_rect(chip_rect));
        let Some(band) = self.params.bands.get(self.selected).copied() else {
            return;
        };
        chip_ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
        chip_ui.spacing_mut().button_padding = egui::vec2(4.0, 0.0);
        chip_ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
        chip_ui
            .style_mut()
            .text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.0));
        let track = crate::theme::palette::CONTROL_FILL;
        let subtle = Color32::from_rgba_unmultiplied(235, 235, 245, 112);
        {
            let style = chip_ui.style_mut();
            for visuals in [
                &mut style.visuals.widgets.inactive,
                &mut style.visuals.widgets.hovered,
                &mut style.visuals.widgets.active,
            ] {
                visuals.bg_fill = track;
                visuals.weak_bg_fill = track;
                visuals.fg_stroke = Stroke::new(1.0, Color32::WHITE);
                visuals.corner_radius = egui::CornerRadius::same(2);
                visuals.expansion = 0.0;
            }
        }
        chip_ui.horizontal(|ui| {
            for kind in Self::type_options(self.selected) {
                if eq_type_button(ui, kind, band.kind == kind).clicked() {
                    self.change_type(kind);
                }
            }
            let (low, high) = self.freq_range(self.selected);
            let mut frequency = band.freq_hz;
            let frequency_response = ui.add(
                egui::DragValue::new(&mut frequency)
                    .prefix(egui::RichText::new("Freq ").color(subtle))
                    .min_size(egui::vec2(85.0, 20.0))
                    .custom_formatter(|value, _| format_freq(value))
                    .range(low..=high),
            );
            ui.ctx()
                .accesskit_node_builder(frequency_response.id, |builder| {
                    builder.set_label("Frequency".to_owned());
                });
            self.field_frame(ui, EqField::Frequency, &frequency_response, frequency, pass);
            let mut gain = band.gain_db;
            let gain_response = ui.add_enabled(
                has_gain(band.kind),
                egui::DragValue::new(&mut gain)
                    .prefix(egui::RichText::new("Gain ").color(subtle))
                    .min_size(egui::vec2(95.0, 20.0))
                    .custom_formatter(|value, _| format!("{value:.1} dB"))
                    .range(-MAX_GAIN..=MAX_GAIN),
            );
            ui.ctx()
                .accesskit_node_builder(gain_response.id, |builder| {
                    builder.set_label("Gain".to_owned());
                });
            self.field_frame(ui, EqField::Gain, &gain_response, gain, pass);
            let mut q = band.q;
            let q_response = ui.add(
                egui::DragValue::new(&mut q)
                    .prefix(egui::RichText::new("Q ").color(subtle))
                    .min_size(egui::vec2(55.0, 20.0))
                    .custom_formatter(|value, _| format!("{value:.2}"))
                    .range(0.1..=10.0),
            );
            ui.ctx().accesskit_node_builder(q_response.id, |builder| {
                builder.set_label("Q".to_owned());
            });
            self.field_frame(ui, EqField::Q, &q_response, q, pass);
        });
    }

    /// Writes one frame of a field of the selected band.
    ///
    /// A field drag, or the edit of a field with the keyboard focus, is one gesture: `Begin` at
    /// its first change, and `End` when the drag stops or the field loses the focus. Escape
    /// cancels it. A change with no drag and no focus, such as one from a screen reader, is one
    /// undo step of its own.
    fn field_frame(
        &mut self,
        ui: &egui::Ui,
        field: EqField,
        response: &egui::Response,
        value: f64,
        pass: u64,
    ) {
        let target = DragTarget::Field(field);
        let active = response.dragged() || response.has_focus();
        let changed = response.changed();
        let begins = changed && active && self.drag_target() != Some(target);
        self.track(target, pass, begins);
        if begins {
            self.start_drag(target, self.selected);
        }
        let open = self.drag_target() == Some(target);
        if changed {
            if !self.set_field(field, value) {
                return;
            }
            if active {
                self.write_in_drag();
            } else if open {
                self.finish_drag(GesturePhase::End);
            } else {
                self.write_params(GesturePhase::End);
            }
        } else if open && !active {
            self.finish_drag(if take_escape(ui) {
                GesturePhase::Cancel
            } else {
                GesturePhase::End
            });
        }
    }

    /// Sets one field of the selected band, held in its range, and sends nothing.
    ///
    /// Returns `false` when the band has no such value, such as the gain of a cut band.
    fn set_field(&mut self, field: EqField, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }
        let (low, high) = self.freq_range(self.selected);
        let Some(band) = self.params.bands.get_mut(self.selected) else {
            return false;
        };
        match field {
            EqField::Frequency => band.freq_hz = value.clamp(low, high),
            EqField::Gain if has_gain(band.kind) => {
                band.gain_db = value.clamp(-MAX_GAIN, MAX_GAIN);
            }
            EqField::Gain => return false,
            EqField::Q => band.q = value.clamp(0.1, 10.0),
        }
        true
    }

    fn paint_spectrum(&self, ui: &mut egui::Ui, rect: Rect) {
        if !self.spectrum.enabled() {
            return;
        }
        let mut mesh = egui::epaint::Mesh::default();
        for index in 1..self.spectrum.bins().len() {
            let left_db = self.spectrum.bins()[index - 1];
            let right_db = self.spectrum.bins()[index];
            let left_energy = ((left_db + 80.0) / 80.0).clamp(0.0, 1.0);
            let right_energy = ((right_db + 80.0) / 80.0).clamp(0.0, 1.0);
            let left_x = rect.left() + (index - 1) as f32 / 63.0 * rect.width();
            let right_x = rect.left() + index as f32 / 63.0 * rect.width();
            let left_top = rect.bottom() - left_energy * rect.height();
            let right_top = rect.bottom() - right_energy * rect.height();
            let polygon = [
                egui::pos2(left_x, left_top),
                egui::pos2(right_x, right_top),
                egui::pos2(right_x, rect.bottom()),
                egui::pos2(left_x, rect.bottom()),
            ];
            for (from, to) in [(0.0, 0.3), (0.3, 0.6), (0.6, 1.0)] {
                let clipped = clip_horizontal(&polygon, rect.top() + from * rect.height(), true);
                let clipped = clip_horizontal(&clipped, rect.top() + to * rect.height(), false);
                for vertex in 1..clipped.len().saturating_sub(1) {
                    let base = mesh.vertices.len() as u32;
                    for point in [clipped[0], clipped[vertex], clipped[vertex + 1]] {
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: point,
                            uv: egui::Pos2::ZERO,
                            color: spectrum_ramp((point.y - rect.top()) / rect.height()),
                        });
                    }
                    mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
                }
            }
        }
        ui.painter_at(rect).add(egui::Shape::mesh(mesh));
    }

    fn write_params(&mut self, gesture: GesturePhase) {
        let _ = self.backend.dispatch(UiAction::SetEq {
            block_id: self.block_id.clone(),
            params: self.params.clone(),
            gesture,
        });
    }
}

fn clip_horizontal(polygon: &[egui::Pos2], boundary: f32, keep_below: bool) -> Vec<egui::Pos2> {
    let mut output = Vec::with_capacity(polygon.len() + 1);
    let Some(mut previous) = polygon.last().copied() else {
        return output;
    };
    let inside = |point: egui::Pos2| {
        if keep_below {
            point.y >= boundary
        } else {
            point.y <= boundary
        }
    };
    let mut previous_inside = inside(previous);
    for &current in polygon {
        let current_inside = inside(current);
        if current_inside != previous_inside {
            let delta = current.y - previous.y;
            if delta.abs() > f32::EPSILON {
                let fraction = (boundary - previous.y) / delta;
                output.push(egui::pos2(
                    previous.x + (current.x - previous.x) * fraction,
                    boundary,
                ));
            }
        }
        if current_inside {
            output.push(current);
        }
        previous = current;
        previous_inside = current_inside;
    }
    output
}

fn eq_type_button(ui: &mut egui::Ui, kind: EqBandType, active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 20.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, type_label(kind)));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, crate::theme::palette::CONTROL_FILL);
    let color = if active {
        Color32::WHITE
    } else {
        crate::theme::palette::TEXT_MUTED
    };
    paint_eq_kind_glyph(&painter, rect, kind, color);
    response
}

fn paint_eq_kind_glyph(painter: &egui::Painter, rect: Rect, kind: EqBandType, color: Color32) {
    let origin = rect.center() - egui::vec2(8.0, 7.0);
    let start = match kind {
        EqBandType::Lowcut => [1.0, 13.0],
        EqBandType::Lowshelf | EqBandType::Bell => [1.0, 11.0],
        EqBandType::Highshelf | EqBandType::Highcut => [1.0, 3.0],
    };
    let mut points = vec![origin + egui::vec2(start[0], start[1])];
    match kind {
        EqBandType::Lowcut => {
            cubic_to(&mut points, origin, [4.0, 13.0], [5.0, 3.0], [9.0, 3.0]);
            points.push(origin + egui::vec2(15.0, 3.0));
        }
        EqBandType::Lowshelf => {
            cubic_to(&mut points, origin, [5.0, 11.0], [6.0, 3.0], [10.0, 3.0]);
            points.push(origin + egui::vec2(15.0, 3.0));
        }
        EqBandType::Bell => {
            cubic_to(&mut points, origin, [4.0, 11.0], [5.0, 3.0], [8.0, 3.0]);
            cubic_to(&mut points, origin, [11.0, 3.0], [12.0, 11.0], [15.0, 11.0]);
        }
        EqBandType::Highshelf => {
            cubic_to(&mut points, origin, [5.0, 3.0], [6.0, 11.0], [10.0, 11.0]);
            points.push(origin + egui::vec2(15.0, 11.0));
        }
        EqBandType::Highcut => {
            points.push(origin + egui::vec2(7.0, 3.0));
            cubic_to(&mut points, origin, [11.0, 3.0], [12.0, 13.0], [15.0, 13.0]);
        }
    }
    painter.add(egui::epaint::PathShape::line(
        points,
        Stroke::new(1.6, color),
    ));
}

fn cubic_to(
    points: &mut Vec<egui::Pos2>,
    origin: egui::Pos2,
    control1: [f32; 2],
    control2: [f32; 2],
    end: [f32; 2],
) {
    let Some(start) = points.last().copied() else {
        return;
    };
    let control1 = origin + egui::vec2(control1[0], control1[1]);
    let control2 = origin + egui::vec2(control2[0], control2[1]);
    let end = origin + egui::vec2(end[0], end[1]);
    for index in 1..=12 {
        let t = index as f32 / 12.0;
        let u = 1.0 - t;
        points.push(egui::pos2(
            u * u * u * start.x
                + 3.0 * u * u * t * control1.x
                + 3.0 * u * t * t * control2.x
                + t * t * t * end.x,
            u * u * u * start.y
                + 3.0 * u * u * t * control1.y
                + 3.0 * u * t * t * control2.y
                + t * t * t * end.y,
        ));
    }
}

fn paint_vertical_gradient(painter: &egui::Painter, rect: Rect, stops: &[(f32, Color32)]) {
    if rect.height() <= 0.0 {
        return;
    }
    let mut mesh = egui::epaint::Mesh::default();
    for pair in stops.windows(2) {
        let [(from, from_color), (to, to_color)] = pair else {
            continue;
        };
        let top = rect.top() + *from * rect.height();
        let bottom = rect.top() + *to * rect.height();
        let base = mesh.vertices.len() as u32;
        for (point, color) in [
            (rect.left_top(), *from_color),
            (rect.right_top(), *from_color),
            (rect.right_bottom(), *to_color),
            (rect.left_bottom(), *to_color),
        ] {
            let point = egui::pos2(point.x, if point.y == rect.top() { top } else { bottom });
            mesh.vertices.push(egui::epaint::Vertex {
                pos: point,
                uv: egui::Pos2::ZERO,
                color,
            });
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn paint_gain_grid(ui: &egui::Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    for gain in [-15.0, -10.0, -5.0, 0.0, 5.0, 10.0, 15.0] {
        let y = (rect.top() + 23.0 + (MAX_GAIN - gain) as f32 / (2.0 * MAX_GAIN as f32) * 225.0)
            .round();
        let color =
            Color32::from_rgba_unmultiplied(235, 235, 245, if gain == 0.0 { 46 } else { 15 });
        if gain == 0.0 {
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(rect.left(), y),
                    egui::pos2(rect.right(), y + 0.5),
                ),
                0.0,
                color,
            );
        } else {
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                Stroke::new(1.0, color),
            );
        }
        let label = if gain > 0.0 {
            format!("+{gain:.0}")
        } else {
            format!("{gain:.0}")
        };
        painter.text(
            egui::pos2(rect.left() + 16.0, y + 1.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(9.0),
            crate::theme::palette::GRAPH_LABEL,
        );
    }
}

fn paint_curve_grid(ui: &egui::Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    for (frequency, label) in [
        (50.0, "50"),
        (100.0, "100"),
        (200.0, "200"),
        (500.0, "500"),
        (1000.0, "1k"),
        (2000.0, "2k"),
        (5000.0, "5k"),
        (10_000.0, "10k"),
    ] {
        let x = rect.left() + freq_to_norm(frequency) as f32 * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(235, 235, 245, 18)),
        );
        painter.text(
            egui::pos2(x + 4.0, rect.bottom() - 16.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(9.0),
            crate::theme::palette::GRAPH_LABEL,
        );
    }
    for gain in [-7.5, 7.5, 0.0] {
        let y = rect.top() + gain_to_y(gain) as f32 / GRAPH_H as f32 * rect.height();
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(235, 235, 245, if gain == 0.0 { 46 } else { 12 }),
            ),
        );
    }
}

fn painter_band_caption(editor: &EqEditor, ui: &mut egui::Ui, rect: Rect) {
    let Some(band) = editor.params.bands.get(editor.selected) else {
        return;
    };
    let painter = ui.painter_at(rect);
    let position = rect.left_top() + egui::vec2(16.0, 16.0);
    let font = egui::FontId::proportional(11.0);
    let kind = band.kind;
    let band = format!("Band {}", editor.selected + 1);
    let rest = format!(" · {}", type_label(kind));
    painter.text(
        position + egui::vec2(0.0, 1.0),
        egui::Align2::LEFT_TOP,
        format!("{band}{rest}"),
        font.clone(),
        Color32::from_black_alpha(230),
    );
    let band_width = ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(band.clone(), font.clone(), Color32::WHITE)
            .rect
            .width()
    });
    painter.text(
        position,
        egui::Align2::LEFT_TOP,
        band,
        font.clone(),
        Color32::WHITE,
    );
    painter.text(
        position + egui::vec2(band_width, 0.0),
        egui::Align2::LEFT_TOP,
        rest,
        font,
        crate::theme::palette::TEXT_MUTED,
    );
}

fn spectrum_ramp(position: f32) -> Color32 {
    // Position 0 is the top of the plot, which is the loudest part of the spectrum.
    let color = crate::theme::palette::fire_ramp(1.0 - position);
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 76)
}

/// Maps frequency to the 20 Hz–20 kHz logarithmic axis.
#[must_use]
pub fn freq_to_norm(freq_hz: f64) -> f64 {
    (freq_hz.clamp(MIN_FREQ, MAX_FREQ).ln() - MIN_FREQ.ln()) / (MAX_FREQ.ln() - MIN_FREQ.ln())
}

/// Maps a normalized axis position to frequency.
#[must_use]
pub fn norm_to_freq(norm: f64) -> f64 {
    (MIN_FREQ.ln() + norm.clamp(0.0, 1.0) * (MAX_FREQ.ln() - MIN_FREQ.ln())).exp()
}

/// Maps gain to graph y coordinates.
#[must_use]
pub fn gain_to_y(gain_db: f64) -> f64 {
    GRAPH_H / 2.0 - (gain_db / MAX_GAIN) * (GRAPH_H / 2.0 - GRAPH_PAD_Y)
}

/// Maps graph y coordinates to gain.
#[must_use]
pub fn y_to_gain(y: f64) -> f64 {
    ((GRAPH_H / 2.0 - y) / (GRAPH_H / 2.0 - GRAPH_PAD_Y)) * MAX_GAIN
}

/// Maps pointer y to a -15…+15 fader value.
#[must_use]
pub fn slider_gain_at(y: f64, top: f64, height: f64) -> f64 {
    let t = ((y - top) / height.max(1.0)).clamp(0.0, 1.0);
    (1.0 - t) * 2.0 * MAX_GAIN - MAX_GAIN
}

/// Formats frequency using the Reference labels.
#[must_use]
pub fn format_freq(freq_hz: f64) -> String {
    if freq_hz >= 10_000.0 {
        format!("{:.1}k", freq_hz / 1000.0)
    } else if freq_hz >= 1000.0 {
        format!("{:.2}k", freq_hz / 1000.0)
    } else {
        format!("{} Hz", freq_hz.round() as i64)
    }
}

/// Parses typed Hz or k-notation, accepting a decimal comma.
#[must_use]
pub fn parse_freq(raw: &str) -> Option<f64> {
    let cleaned = raw.trim().to_lowercase().replace(',', ".");
    let cleaned = cleaned.strip_suffix("hz").unwrap_or(&cleaned).trim();
    let has_k = cleaned.contains('k');
    let number = cleaned.replace('k', "");
    let number = number.trim();
    if number.is_empty() || !number.chars().all(|ch| "0123456789.-+".contains(ch)) {
        return None;
    }
    let value = number.parse::<f64>().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(if has_k { value * 1000.0 } else { value })
}

/// Parses a typed decimal, accepting a decimal comma.
#[must_use]
pub fn parse_decimal(raw: &str) -> Option<f64> {
    let cleaned = raw.trim().replace(',', ".");
    if cleaned.is_empty() || !cleaned.chars().all(|ch| "0123456789.-+".contains(ch)) {
        return None;
    }
    cleaned
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// Whether a curve shape has editable gain.
#[must_use]
pub const fn has_gain(kind: EqBandType) -> bool {
    matches!(
        kind,
        EqBandType::Bell | EqBandType::Lowshelf | EqBandType::Highshelf
    )
}

/// Display label for a curve shape.
#[must_use]
pub const fn type_label(kind: EqBandType) -> &'static str {
    match kind {
        EqBandType::Lowcut => "Low Cut",
        EqBandType::Lowshelf => "Low Shelf",
        EqBandType::Bell => "Bell",
        EqBandType::Highshelf => "High Shelf",
        EqBandType::Highcut => "High Cut",
    }
}

/// Q applied when changing shape; bells retain their current Q.
#[must_use]
pub const fn type_default_q(kind: EqBandType) -> Option<f64> {
    if matches!(kind, EqBandType::Bell) {
        None
    } else {
        Some(0.71)
    }
}

/// Six Reference-default bands.
#[must_use]
pub fn reference_bands() -> Vec<EqBand> {
    vec![
        EqBand {
            kind: EqBandType::Lowcut,
            freq_hz: 20.0,
            gain_db: 0.0,
            q: 0.71,
        },
        EqBand {
            kind: EqBandType::Bell,
            freq_hz: 150.0,
            gain_db: 0.0,
            q: 1.0,
        },
        EqBand {
            kind: EqBandType::Bell,
            freq_hz: 500.0,
            gain_db: 0.0,
            q: 1.0,
        },
        EqBand {
            kind: EqBandType::Bell,
            freq_hz: 1500.0,
            gain_db: 0.0,
            q: 1.0,
        },
        EqBand {
            kind: EqBandType::Bell,
            freq_hz: 4000.0,
            gain_db: 0.0,
            q: 1.0,
        },
        EqBand {
            kind: EqBandType::Highshelf,
            freq_hz: 8000.0,
            gain_db: 0.0,
            q: 0.71,
        },
    ]
}
