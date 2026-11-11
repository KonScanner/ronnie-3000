//! One module for every popover: placement, paint, dismissal, stacking, and focus return.
//!
//! A caller gives a trigger or anchor, a size, and content, and gets back the pick. The
//! caller keeps its own open state and closes when [`Shown::dismissed`] says so.
//!
//! egui's [`egui::Popup`] is not the base here. It hides a popup on every opening frame for a
//! sizing pass, it positions by pivot and gap, and popups have no shared order for Escape. The
//! first two would change pixels, and the third would break a stack. It also closes on a
//! click, while this module closes on a press, so a text selection that ends outside a
//! popover does not close it.

use eframe::egui::{self, Color32, Rect, Stroke, Vec2};

use super::overlays::DismissReason;
use crate::theme::palette;

/// Control that opened a popover.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trigger {
    rect: Rect,
    focus: Option<egui::Id>,
}

impl Trigger {
    /// A trigger area and the widget that takes focus when the popover closes.
    ///
    /// A press inside `rect` is not an outside press, so the owner can toggle the popover.
    #[must_use]
    pub const fn new(rect: Rect, focus: Option<egui::Id>) -> Self {
        Self { rect, focus }
    }
}

impl From<&egui::Response> for Trigger {
    fn from(response: &egui::Response) -> Self {
        Self::new(response.rect, Some(response.id))
    }
}

/// Where the panel goes, in the coordinates of the parent layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// The panel's top-left corner is at this position.
    At(egui::Pos2),
    /// The panel's bottom-left corner is `gap` above the anchor's top-left corner.
    Above {
        /// Rectangle that the panel follows.
        anchor: Rect,
        /// Distance between the panel and the anchor.
        gap: f32,
    },
    /// The panel's top-left corner is `gap` below the anchor's bottom-left corner.
    Below {
        /// Rectangle that the panel follows.
        anchor: Rect,
        /// Distance between the panel and the anchor.
        gap: f32,
    },
}

impl Placement {
    fn top_left(self, size: Vec2) -> egui::Pos2 {
        match self {
            Self::At(position) => position,
            Self::Above { anchor, gap } => anchor.left_top() - egui::vec2(0.0, size.y + gap),
            Self::Below { anchor, gap } => anchor.left_bottom() + egui::vec2(0.0, gap),
        }
    }
}

/// How the panel background paints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Surface {
    /// A rectangle at the exact panel area, with an optional inside edge.
    Panel {
        /// Background fill.
        fill: Color32,
        /// Corner radius.
        radius: f32,
        /// Edge drawn inside the panel area.
        edge: Option<Stroke>,
    },
    /// An egui frame that wraps the content, for content that lays itself out.
    Frame(egui::Frame),
}

impl Surface {
    /// The dark panel with a solid edge.
    #[must_use]
    pub fn edged(radius: f32) -> Self {
        Self::Panel {
            fill: palette::PANEL,
            radius,
            edge: Some(Stroke::new(1.0, palette::PANEL_EDGE)),
        }
    }

    /// The dark panel with a translucent hairline.
    #[must_use]
    pub fn hairline(radius: f32) -> Self {
        Self::Panel {
            fill: palette::PANEL,
            radius,
            edge: Some(Stroke::new(1.0, palette::HAIRLINE)),
        }
    }
}

/// What one frame of a popover produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct Shown<P> {
    /// Value that the content picked this frame.
    pub pick: Option<P>,
    /// Why the owner must close this popover, if it must.
    pub dismissed: Option<DismissReason>,
}

/// A popover for one frame.
#[derive(Debug, Clone, Copy)]
#[must_use]
pub struct Popover {
    id: egui::Id,
    size: Vec2,
    placement: Placement,
    trigger: Option<Trigger>,
    surface: Surface,
    constrain_to: Option<Rect>,
}

impl Popover {
    /// Describes a popover with a stable identity. Without a trigger, focus does not move.
    pub fn new(id: egui::Id, placement: Placement, size: Vec2, surface: Surface) -> Self {
        Self {
            id,
            size,
            placement,
            trigger: None,
            surface,
            constrain_to: None,
        }
    }

    /// Sets the control that opened this popover, when one exists.
    pub fn trigger(mut self, trigger: impl Into<Option<Trigger>>) -> Self {
        self.trigger = trigger.into();
        self
    }

    /// Bounds the layer by `rect` instead of the window.
    ///
    /// A frame surface moves to stay inside `rect`. A panel surface stays at its placement, and
    /// `rect` only clips it.
    pub fn constrain_to(mut self, rect: Rect) -> Self {
        self.constrain_to = Some(rect);
        self
    }

    /// Paints the popover and applies the dismissal rules. The pick closes the popover.
    ///
    /// The popover adopts the transform of the parent layer, so its graphics and its input
    /// stay with a zoomed parent. `content` receives the panel area in the same coordinates
    /// as the placement.
    pub fn show<P>(
        self,
        ui: &egui::Ui,
        content: impl FnOnce(&mut egui::Ui, Rect) -> Option<P>,
    ) -> Shown<P> {
        let context = ui.ctx().clone();
        let panel = Rect::from_min_size(self.placement.top_left(self.size), self.size);
        let to_global = context
            .layer_transform_to_global(ui.layer_id())
            .unwrap_or(egui::emath::TSTransform::IDENTITY);
        let just_opened = Stack::enter(&context, self.id);

        let mut area = egui::Area::new(self.id)
            .fixed_pos(panel.min)
            .order(egui::Order::Foreground);
        if let Some(rect) = self.constrain_to {
            area = area.constrain_to(rect);
        }
        if matches!(self.surface, Surface::Panel { .. }) {
            // A panel paints at its exact area. A moved area adds the distance to its own size,
            // and the extra area would then hide an outside press.
            area = area.constrain(false);
        }
        context.set_transform_layer(area.layer(), to_global);
        let shown = area.show(&context, |ui| match self.surface {
            Surface::Panel { fill, radius, edge } => {
                ui.expand_to_include_rect(panel);
                let painter = ui.painter_at(panel);
                painter.rect_filled(panel, radius, fill);
                if let Some(edge) = edge {
                    painter.rect_stroke(panel, radius, edge, egui::StrokeKind::Inside);
                }
                content(ui, panel)
            }
            Surface::Frame(frame) => frame.show(ui, |ui| content(ui, panel)).inner,
        });

        let hit_area = to_global * panel.union(shown.response.rect);
        Stack::place(&context, self.id, hit_area);
        let dismissed = if shown.inner.is_some() {
            Some(DismissReason::Action)
        } else if Stack::take_escape(&context, self.id) {
            Some(DismissReason::Escape)
        } else if !just_opened
            && Stack::pressed_outside(&context, self.id, hit_area, self.trigger_area(to_global))
        {
            Some(DismissReason::Outside)
        } else {
            None
        };
        if let Some(reason) = dismissed {
            Stack::leave(&context, self.id);
            if reason != DismissReason::Outside
                && let Some(focus) = self.trigger.and_then(|trigger| trigger.focus)
            {
                context.memory_mut(|memory| memory.request_focus(focus));
            }
        }
        Shown {
            pick: shown.inner,
            dismissed,
        }
    }

    fn trigger_area(&self, to_global: egui::emath::TSTransform) -> Option<Rect> {
        self.trigger.map(|trigger| to_global * trigger.rect)
    }
}

/// Whether a surface under the popovers may use an Escape press this frame.
///
/// Every takeover must ask this, because a frame shows it before its popovers. Without this
/// check one Escape would close a popover and the takeover under it.
#[must_use]
pub fn escape_reaches_surface(context: &egui::Context) -> bool {
    !any_open(context) && !egui::Popup::is_any_open(context) && context.input(new_escape_press)
}

/// Whether this frame has a new Escape press.
///
/// A held key repeats. A repeat is not a new press, so it must not close the next surface.
fn new_escape_press(input: &egui::InputState) -> bool {
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                repeat: false,
                ..
            }
        )
    })
}

/// Whether a popover is on the screen.
#[must_use]
pub fn any_open(context: &egui::Context) -> bool {
    !open_rects(context).is_empty()
}

/// Window areas of the open popovers, from the bottom of the stack to the top.
#[must_use]
pub fn open_rects(context: &egui::Context) -> Vec<Rect> {
    Stack::read(context)
        .into_iter()
        .map(|entry| entry.area)
        .collect()
}

/// Open popovers in opening order, with the top one last. It lives in the egui context, so
/// a popover deep in one module stacks over a popover in another module.
#[derive(Debug, Clone, Default)]
struct Stack {
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    id: egui::Id,
    last_pass: u64,
    area: Rect,
}

impl Stack {
    fn with<R>(context: &egui::Context, write: impl FnOnce(&mut Self, u64) -> R) -> R {
        let pass = context.cumulative_pass_nr();
        context.data_mut(|data| {
            let stack = data.get_temp_mut_or_default::<Self>(egui::Id::unique("popover-stack"));
            // A popover that the previous pass did not show has closed.
            stack.entries.retain(|entry| entry.last_pass + 1 >= pass);
            write(stack, pass)
        })
    }

    fn read(context: &egui::Context) -> Vec<Entry> {
        Self::with(context, |stack, _| stack.entries.clone())
    }

    /// Marks a popover as shown this pass. Returns whether it opened this pass.
    fn enter(context: &egui::Context, id: egui::Id) -> bool {
        Self::with(context, |stack, pass| {
            if let Some(entry) = stack.entries.iter_mut().find(|entry| entry.id == id) {
                entry.last_pass = pass;
                false
            } else {
                stack.entries.push(Entry {
                    id,
                    last_pass: pass,
                    area: Rect::NOTHING,
                });
                true
            }
        })
    }

    fn place(context: &egui::Context, id: egui::Id, area: Rect) {
        Self::with(context, |stack, _| {
            if let Some(entry) = stack.entries.iter_mut().find(|entry| entry.id == id) {
                entry.area = area;
            }
        });
    }

    fn leave(context: &egui::Context, id: egui::Id) {
        Self::with(context, |stack, _| {
            stack.entries.retain(|entry| entry.id != id);
        });
    }

    /// Consumes an Escape press when `id` is the top popover, so the press closes one only.
    fn take_escape(context: &egui::Context, id: egui::Id) -> bool {
        let is_top = Self::with(context, |stack, _| {
            stack.entries.last().is_some_and(|entry| entry.id == id)
        });
        is_top
            && context.input_mut(|input| {
                let modifiers = input.modifiers;
                new_escape_press(input) && input.consume_key(modifiers, egui::Key::Escape)
            })
    }

    /// Whether a pointer press landed outside this popover, its trigger, and every popover
    /// over it. A press in a popover above must not close the ones below it.
    fn pressed_outside(
        context: &egui::Context,
        id: egui::Id,
        hit_area: Rect,
        trigger: Option<Rect>,
    ) -> bool {
        let Some(pressed_at) = context.input(|input| {
            input
                .pointer
                .any_pressed()
                .then(|| input.pointer.latest_pos())
                .flatten()
        }) else {
            return false;
        };
        if hit_area.contains(pressed_at) || trigger.is_some_and(|rect| rect.contains(pressed_at)) {
            return false;
        }
        Self::with(context, |stack, _| {
            !stack
                .entries
                .iter()
                .skip_while(|entry| entry.id != id)
                .skip(1)
                .any(|entry| entry.area.contains(pressed_at))
        })
    }
}
