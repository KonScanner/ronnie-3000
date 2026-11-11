//! Real-input driver for the Default UI in a headless egui context.
//!
//! A test names a control by its accessible label. The driver finds that control in the
//! AccessKit output of the real [`crate::default::show`] function, plays the input over
//! separate frames, and reports what the user could observe. A label that is not on the
//! screen is an error, so a test cannot pass for a button that does not exist.

use eframe::egui;
use thiserror::Error;

use crate::{
    backend::{MockBackend, UiEffect, UiEvent},
    default::ChromeController,
    fonts::Fonts,
};

/// Logical width of the design box.
const DESIGN_WIDTH: f32 = 1024.0;
/// Most mouse-wheel steps that the driver plays to reveal one control.
const WHEEL_STEPS: usize = 64;
/// Points of travel in one mouse-wheel step.
const WHEEL_STEP: f32 = 120.0;

/// Why the driver could not reach a control.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// No control on the screen has this label.
    #[error("no control is labelled {label:?}; the screen shows {visible:?}")]
    Absent {
        /// Requested label.
        label: String,
        /// Every label that is on the screen.
        visible: Vec<String>,
    },
    /// More than one control can take the input, so the test must be more exact.
    #[error("{count} controls that can take a click are labelled {label:?}")]
    Ambiguous {
        /// Requested label.
        label: String,
        /// Number of matching controls.
        count: usize,
    },
    /// The control exists, but it does not take this input.
    #[error("the control labelled {label:?} is disabled or does not sense a click")]
    NotInteractive {
        /// Requested label.
        label: String,
    },
    /// The control has no area inside the window.
    #[error("the control labelled {label:?} has no area inside the window")]
    OffScreen {
        /// Requested label.
        label: String,
    },
    /// Something else is on top of the control and takes the input.
    #[error("another surface covers the control labelled {label:?} at {pos:?}")]
    Covered {
        /// Requested label.
        label: String,
        /// Pointer position that missed the control, as text.
        pos: String,
    },
}

/// One control in the accessibility output of the latest frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    /// Accessible label.
    pub label: String,
    /// Area that takes pointer input, in window coordinates.
    pub rect: egui::Rect,
    /// Whether the control can take a click.
    pub clickable: bool,
    id: egui::Id,
}

/// What one played input did.
#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub struct Interaction {
    /// Backend effects that the input caused, in order.
    pub effects: Vec<UiEffect>,
    /// URLs that the UI asked the system to open.
    pub opened_urls: Vec<String>,
    /// Text that the UI put on the clipboard.
    pub copied_text: Vec<String>,
    /// Labels on the screen after the input.
    pub visible: Vec<String>,
}

/// Plays real pointer and key input through the real Default UI.
pub struct UiDriver {
    context: egui::Context,
    chrome: ChromeController,
    /// Only a mock records effects, so that a driver of a real backend reports none.
    backend: Option<MockBackend>,
    controls: Vec<Control>,
    /// The text of each static label in the accessibility output of the latest frame.
    texts: Vec<String>,
    focused: Option<String>,
    effects_seen: usize,
    opened_urls: Vec<String>,
    copied_text: Vec<String>,
}

impl UiDriver {
    /// Creates a driver and runs the first frame, so the first label search can succeed.
    ///
    /// `backend` must be the mock that `chrome` uses, because the driver reads effects from it.
    #[must_use]
    pub fn new(chrome: ChromeController, backend: MockBackend) -> Self {
        Self::start(chrome, Some(backend))
    }

    /// Creates a driver for a controller on a real backend, whose test reads that backend.
    ///
    /// Each [`Interaction`] of this driver reports no effects.
    #[must_use]
    pub fn on_real_backend(chrome: ChromeController) -> Self {
        Self::start(chrome, None)
    }

    fn start(chrome: ChromeController, backend: Option<MockBackend>) -> Self {
        let context = egui::Context::default();
        context.enable_accesskit();
        Fonts::install(&context);
        let effects_seen = backend.as_ref().map_or(0, |backend| backend.events().len());
        let mut driver = Self {
            context,
            chrome,
            backend,
            controls: Vec::new(),
            texts: Vec::new(),
            focused: None,
            effects_seen,
            opened_urls: Vec::new(),
            copied_text: Vec::new(),
        };
        driver.settle();
        driver
    }

    /// The controller under test.
    #[must_use]
    pub const fn chrome(&self) -> &ChromeController {
        &self.chrome
    }

    /// The controller under test, for state that a test sets up.
    pub fn chrome_mut(&mut self) -> &mut ChromeController {
        &mut self.chrome
    }

    /// The headless egui context.
    #[must_use]
    pub const fn context(&self) -> &egui::Context {
        &self.context
    }

    /// Controls in the accessibility output of the latest frame.
    #[must_use]
    pub fn controls(&self) -> &[Control] {
        &self.controls
    }

    /// Every label on the screen, sorted and without duplicates.
    #[must_use]
    pub fn visible_labels(&self) -> Vec<String> {
        let mut labels = self
            .controls
            .iter()
            .map(|control| control.label.clone())
            .collect::<Vec<_>>();
        labels.sort();
        labels.dedup();
        labels
    }

    /// Whether a control with this label is on the screen.
    #[must_use]
    pub fn is_visible(&self, label: &str) -> bool {
        self.controls.iter().any(|control| control.label == label)
    }

    /// Whether a static label with this text is on the screen.
    ///
    /// A static label carries its text as its accessible value, so it is not a control.
    #[must_use]
    pub fn shows_text(&self, text: &str) -> bool {
        self.texts.iter().any(|shown| shown == text)
    }

    /// First label on the screen that starts with `prefix`, for a label that carries a value.
    #[must_use]
    pub fn label_starting_with(&self, prefix: &str) -> Option<String> {
        self.controls
            .iter()
            .find(|control| control.label.starts_with(prefix))
            .map(|control| control.label.clone())
    }

    /// Label of the control that has keyboard focus.
    #[must_use]
    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    /// Runs one frame without input.
    pub fn frame(&mut self) {
        self.run(Vec::new());
    }

    /// Runs one frame with exactly these input events, for input that a real frame can batch.
    pub fn play(&mut self, events: Vec<egui::Event>) {
        self.run(events);
    }

    /// Runs enough frames for a surface that opened to paint and take input.
    pub fn settle(&mut self) {
        self.frame();
        self.frame();
    }

    /// Moves the pointer to a control, then presses and releases the primary button.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the label is absent, or when the control cannot take
    /// the click. No click is played in that case.
    pub fn click(&mut self, label: &str) -> Result<Interaction, DriverError> {
        self.click_with(label, egui::PointerButton::Primary)
    }

    /// Clicks the first on-screen control of several with one label, for equal slots such as
    /// two empty tile positions.
    ///
    /// The first control is the one with the largest area on the screen, and then the one
    /// nearest the left. The order of the accessibility tree follows the widget IDs and not the
    /// screen, and a control that the screen edge clips can be under an edge button.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the label is absent, or when the control cannot take
    /// the click. No click is played in that case.
    pub fn click_first(&mut self, label: &str) -> Result<Interaction, DriverError> {
        let control = self
            .controls
            .iter()
            .filter(|control| {
                control.label == label && control.clickable && control.rect.is_positive()
            })
            .min_by(|left, right| {
                right
                    .rect
                    .area()
                    .total_cmp(&left.rect.area())
                    .then(left.rect.left().total_cmp(&right.rect.left()))
            })
            .cloned();
        match control {
            Some(control) => self.click_control(&control, egui::PointerButton::Primary),
            None => self.click(label),
        }
    }

    /// Moves the pointer to a control, then presses and releases the secondary button.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the label is absent, or when the control cannot take
    /// the click. No click is played in that case.
    pub fn secondary_click(&mut self, label: &str) -> Result<Interaction, DriverError> {
        self.click_with(label, egui::PointerButton::Secondary)
    }

    /// Plays a primary click at a window position, whatever is there.
    pub fn click_at(&mut self, pos: egui::Pos2) -> Interaction {
        self.play_click(pos, egui::PointerButton::Primary);
        self.interaction()
    }

    /// Plays a primary click on a window position that no control and no popover occupies.
    ///
    /// # Panics
    /// Panics when the screen has no free position, because a test then cannot ask for an
    /// outside click. This function is for tests only.
    pub fn click_empty_space(&mut self) -> Interaction {
        let pos = self
            .empty_position()
            // A test that asks for an outside click must fail when none exists.
            .expect("the screen must have a position without a control");
        self.click_at(pos)
    }

    /// Presses the primary button at `from`, moves to `to`, and releases there.
    pub fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) -> Interaction {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.run(vec![egui::Event::PointerMoved(from)]);
        self.run(vec![button(from, true)]);
        self.run(vec![egui::Event::PointerMoved(to)]);
        self.run(vec![button(to, false)]);
        self.settle();
        self.interaction()
    }

    /// Presses the primary button on a control, moves through each offset from its center over
    /// separate frames, and releases there.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the label is absent or another surface covers it.
    pub fn drag_control(
        &mut self,
        label: &str,
        offsets: &[egui::Vec2],
    ) -> Result<Interaction, DriverError> {
        let control = self.find(label)?;
        let control = self.scroll_into_view(&control)?;
        self.move_onto(&control)?;
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let start = control.rect.center();
        self.run(vec![button(start, true)]);
        let mut pos = start;
        for offset in offsets {
            pos = start + *offset;
            self.run(vec![egui::Event::PointerMoved(pos)]);
        }
        self.run(vec![button(pos, false)]);
        self.settle();
        Ok(self.interaction())
    }

    /// Moves the pointer to a control and leaves it there.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the label is absent or another surface covers it.
    pub fn hover(&mut self, label: &str) -> Result<(), DriverError> {
        let control = self.find(label)?;
        let control = self.scroll_into_view(&control)?;
        self.move_onto(&control)
    }

    /// Moves the pointer out of the window.
    pub fn unhover(&mut self) {
        self.run(vec![egui::Event::PointerGone]);
    }

    /// Presses and releases one key without modifiers.
    pub fn press_key(&mut self, key: egui::Key) -> Interaction {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        self.run(vec![event(true)]);
        self.run(vec![event(false)]);
        self.settle();
        self.interaction()
    }

    /// Clicks a text field, replaces its text, and leaves the field focused.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the field is absent or cannot take the click.
    pub fn fill(&mut self, label: &str, text: &str) -> Result<Interaction, DriverError> {
        let _ = self.click(label)?;
        let select_all = |pressed| egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        self.run(vec![select_all(true), select_all(false)]);
        self.run(vec![egui::Event::Text(text.to_owned())]);
        self.settle();
        Ok(self.interaction())
    }

    /// Takes what happened since the last report: effects, URLs, and clipboard text.
    pub fn interaction(&mut self) -> Interaction {
        let events = self
            .backend
            .as_ref()
            .map_or_else(Vec::new, MockBackend::events);
        let effects = events
            .iter()
            .skip(self.effects_seen)
            .filter_map(|event| match event {
                UiEvent::Effect(effect) => Some(*effect),
                UiEvent::Scrolled { .. } => None,
            })
            .collect();
        self.effects_seen = events.len();
        Interaction {
            effects,
            opened_urls: std::mem::take(&mut self.opened_urls),
            copied_text: std::mem::take(&mut self.copied_text),
            visible: self.visible_labels(),
        }
    }

    fn click_with(
        &mut self,
        label: &str,
        button: egui::PointerButton,
    ) -> Result<Interaction, DriverError> {
        let control = self.find(label)?;
        self.click_control(&control, button)
    }

    fn click_control(
        &mut self,
        control: &Control,
        button: egui::PointerButton,
    ) -> Result<Interaction, DriverError> {
        if !control.clickable {
            return Err(DriverError::NotInteractive {
                label: control.label.clone(),
            });
        }
        let control = self.scroll_into_view(control)?;
        self.move_onto(&control)?;
        self.play_press_and_release(control.rect.center(), button);
        Ok(self.interaction())
    }

    /// Plays the mouse wheel until the control has an area inside the window.
    fn scroll_into_view(&mut self, control: &Control) -> Result<Control, DriverError> {
        let off_screen = || DriverError::OffScreen {
            label: control.label.clone(),
        };
        let screen = self.screen_rect();
        let mut current = control.clone();
        for _ in 0..WHEEL_STEPS {
            if current.rect.is_positive() && screen.contains_rect(current.rect) {
                return Ok(current);
            }
            let wheel = egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -WHEEL_STEP),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: egui::Modifiers::NONE,
            };
            self.run(vec![egui::Event::PointerMoved(screen.center()), wheel]);
            self.settle();
            current = self
                .controls
                .iter()
                .find(|other| other.id == control.id)
                .cloned()
                .ok_or_else(off_screen)?;
        }
        if current.rect.is_positive() {
            Ok(current)
        } else {
            Err(off_screen())
        }
    }

    fn find(&self, label: &str) -> Result<Control, DriverError> {
        let matching = self
            .controls
            .iter()
            .filter(|control| control.label == label)
            .collect::<Vec<_>>();
        let Some(first) = matching.first() else {
            return Err(DriverError::Absent {
                label: label.to_owned(),
                visible: self.visible_labels(),
            });
        };
        if matching.len() == 1 {
            return Ok((*first).clone());
        }
        let clickable = matching
            .iter()
            .filter(|control| control.clickable)
            .collect::<Vec<_>>();
        match clickable.as_slice() {
            [] => Ok((*first).clone()),
            [only] => Ok((**only).clone()),
            many => Err(DriverError::Ambiguous {
                label: label.to_owned(),
                count: many.len(),
            }),
        }
    }

    fn move_onto(&mut self, control: &Control) -> Result<(), DriverError> {
        let pos = control.rect.center();
        self.run(vec![egui::Event::PointerMoved(pos)]);
        let reached = self
            .context
            .read_response(control.id)
            .is_some_and(|response| response.contains_pointer());
        if reached {
            Ok(())
        } else {
            Err(DriverError::Covered {
                label: control.label.clone(),
                pos: format!("{pos:?}"),
            })
        }
    }

    fn play_click(&mut self, pos: egui::Pos2, button: egui::PointerButton) {
        self.run(vec![egui::Event::PointerMoved(pos)]);
        self.play_press_and_release(pos, button);
    }

    fn play_press_and_release(&mut self, pos: egui::Pos2, button: egui::PointerButton) {
        let event = |pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.run(vec![event(true)]);
        self.run(vec![event(false)]);
        self.settle();
    }

    fn empty_position(&self) -> Option<egui::Pos2> {
        let screen = self.screen_rect();
        let popovers = crate::default::popover::open_rects(&self.context);
        let free = |pos: egui::Pos2| {
            self.controls.iter().all(|control| {
                // A wide scroll surface is a background, not a control in the way.
                !control.rect.contains(pos) || !control.clickable
            }) && popovers.iter().all(|rect| !rect.contains(pos))
        };
        let step = 12.0;
        let columns = (screen.width() / step) as u32;
        let rows = (screen.height() / step) as u32;
        (1..rows)
            .flat_map(|row| {
                (1..columns)
                    .map(move |column| egui::pos2(column as f32 * step, (rows - row) as f32 * step))
            })
            .find(|pos| free(*pos))
    }

    fn screen_rect(&self) -> egui::Rect {
        let height = self.chrome.window_chrome().design_height() as f32;
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(DESIGN_WIDTH, height))
    }

    fn run(&mut self, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(self.screen_rect()),
            events,
            ..Default::default()
        };
        let chrome = &mut self.chrome;
        let mut output = self.context.run_ui(input, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ui, |ui| crate::default::show(ui, chrome));
        });
        output.textures_delta.clear();
        for command in std::mem::take(&mut output.platform_output.commands) {
            match command {
                egui::OutputCommand::OpenUrl(open) => self.opened_urls.push(open.url),
                egui::OutputCommand::CopyText(text) => self.copied_text.push(text),
                _ => {}
            }
        }
        self.read_tree(output.platform_output.accesskit_update.take());
    }

    fn read_tree(&mut self, update: Option<accesskit::TreeUpdate>) {
        let Some(update) = update else {
            return;
        };
        let context = self.context.clone();
        let screen = self.screen_rect();
        let widgets = context.viewport(|viewport| {
            viewport
                .prev_pass
                .widgets
                .layers()
                .flat_map(|(_, widgets)| widgets.iter().copied())
                .map(|widget| (widget.id.accesskit_id(), widget))
                .collect::<std::collections::HashMap<_, _>>()
        });
        self.focused = update
            .nodes
            .iter()
            .find(|(id, _)| *id == update.focus)
            .and_then(|(_, node)| node.label().map(str::to_owned));
        self.texts = update
            .nodes
            .iter()
            .filter(|(_, node)| !node.is_hidden() && node.role() == accesskit::Role::Label)
            .filter_map(|(_, node)| node.value().map(|value| value.trim().to_owned()))
            .collect();
        self.controls = update
            .nodes
            .iter()
            .filter(|(_, node)| !node.is_hidden())
            .filter_map(|(id, node)| {
                let label = node.label()?.trim();
                let widget = widgets.get(id)?;
                if label.is_empty() {
                    return None;
                }
                let to_global = context
                    .layer_transform_to_global(widget.layer_id)
                    .unwrap_or(egui::emath::TSTransform::IDENTITY);
                Some(Control {
                    label: label.to_owned(),
                    rect: (to_global * widget.interact_rect).intersect(screen),
                    clickable: node.supports_action(accesskit::Action::Click)
                        && !node.is_disabled(),
                    id: widget.id,
                })
            })
            .collect();
    }
}
