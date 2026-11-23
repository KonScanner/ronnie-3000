//! The dark menu that a right-click on a block of the chain strip opens: BYPASS, REPLACE…,
//! DUPLICATE and REMOVE, under the block's icon.

use eframe::egui::{self, Color32, FontId, Rect, RichText, Stroke};
use ronnie_core::BlockId;

use super::add_menu::{
    Caret, HAIRLINE, HOVER, MUTED, TEXT, bold, menu_item, paint_board, text_in, tracked,
};

const WIDTH: f32 = 200.0;
const PADDING: f32 = 6.0;
/// The muted title line with the block's name.
const TITLE: f32 = 26.0;
const ROW: f32 = 36.0;
/// The hairline before REMOVE and its margins.
const DIVIDER: f32 = 9.0;
const GAP_BELOW_ANCHOR: f32 = 6.0;
/// Where the caret sits from the left edge of the menu, when the window leaves room.
const CARET_INSET: f32 = 36.0;
/// REMOVE, a soft red that reads on the black board.
const DANGER: Color32 = Color32::from_rgb(0xff, 0x8a, 0x80);

/// One row of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockMenuAction {
    Bypass,
    Replace,
    Duplicate,
    Remove,
}

/// The menu's own state; it lives in egui temp data between frames.
#[derive(Debug, Clone, Default)]
pub(super) struct BlockMenu {
    /// The block whose menu is open, the strip icon that it opened under, and the block's title.
    open: Option<(BlockId, Rect, String)>,
    /// Whether the menu opened on this frame, so that the click that opened it does not close it.
    fresh: bool,
}

impl BlockMenu {
    fn id() -> egui::Id {
        egui::Id::unique("stage-block-menu")
    }

    pub(super) fn load(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp::<Self>(Self::id()))
            .unwrap_or_default()
    }

    pub(super) fn store(&self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(Self::id(), self.clone()));
    }

    pub(super) const fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Opens the menu of block `id` under its strip icon `anchor`, or closes it when it is that
    /// block's menu already.
    pub(super) fn toggle(&mut self, id: BlockId, anchor: Rect, title: String) {
        if self.open.as_ref().is_some_and(|(open, _, _)| *open == id) {
            self.open = None;
        } else {
            self.open = Some((id, anchor, title));
            self.fresh = true;
        }
    }

    /// Closes the menu when its block left the chain.
    pub(super) fn retain(&mut self, exists: impl Fn(&BlockId) -> bool) {
        if self.open.as_ref().is_some_and(|(id, _, _)| !exists(id)) {
            self.open = None;
        }
    }
}

/// The rows from the top: the action, its label, its colour, and the hint at its right.
const ROWS: [(BlockMenuAction, &str, Hint); 4] = [
    (
        BlockMenuAction::Bypass,
        "BYPASS",
        Hint::Text("double-click"),
    ),
    (BlockMenuAction::Replace, "REPLACE\u{2026}", Hint::None),
    (BlockMenuAction::Duplicate, "DUPLICATE", Hint::None),
    (BlockMenuAction::Remove, "REMOVE", Hint::BackspaceKey),
];

/// What the right end of a row shows.
#[derive(Clone, Copy)]
enum Hint {
    None,
    Text(&'static str),
    /// The ⌫ key, painted since the embedded fonts have no glyph for it.
    BackspaceKey,
}

fn height() -> f32 {
    PADDING * 2.0 + TITLE + ROW * ROWS.len() as f32 + DIVIDER
}

/// The menu's rect and the x of its caret: under `anchor`, with the caret near the left edge
/// over the anchor's center, inside the window.
fn place(anchor: Rect, screen: Rect) -> (Rect, f32) {
    let left = (anchor.center().x - CARET_INSET).clamp(
        screen.left() + 4.0,
        (screen.right() - WIDTH - 4.0).max(screen.left() + 4.0),
    );
    let rect = Rect::from_min_size(
        egui::pos2(left, anchor.bottom() + GAP_BELOW_ANCHOR),
        egui::vec2(WIDTH, height()),
    );
    let caret = anchor
        .center()
        .x
        .clamp(rect.left() + 14.0, rect.right() - 14.0);
    (rect, caret)
}

/// Draws the menu when it is open and returns the block, the icon that the menu opened under,
/// and the row that the player chose. Escape, a click outside the menu and a choice close it.
pub(super) fn show(
    ui: &mut egui::Ui,
    menu: &mut BlockMenu,
) -> Option<(BlockId, Rect, BlockMenuAction)> {
    let (id, anchor, title) = menu.open.clone()?;
    let fresh = std::mem::take(&mut menu.fresh);
    let (rect, caret) = place(anchor, ui.ctx().content_rect());
    let mut chosen = None;
    egui::Area::new(egui::Id::unique("stage-block-menu-area"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ui.ctx(), |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            chosen = draw(ui, rect, caret, &title);
        });

    let (escape, clicked_at) = ui.input(|input| {
        (
            input.key_pressed(egui::Key::Escape),
            (input.pointer.primary_clicked() || input.pointer.secondary_clicked())
                .then(|| input.pointer.latest_pos())
                .flatten(),
        )
    });
    let outside = !fresh && clicked_at.is_some_and(|pos| !rect.contains(pos));
    if chosen.is_some() || escape || outside {
        menu.open = None;
    }
    chosen.map(|action| (id, anchor, action))
}

fn draw(ui: &mut egui::Ui, rect: Rect, caret: f32, title: &str) -> Option<BlockMenuAction> {
    let painter = ui.painter().clone();
    paint_board(&painter, rect, Caret::Top(caret));
    let inner = rect.shrink(PADDING);
    text_in(
        ui,
        Rect::from_min_max(
            inner.min + egui::vec2(10.0, 0.0),
            egui::pos2(inner.right() - 10.0, inner.top() + TITLE),
        ),
        tracked(&title.to_uppercase(), bold(9.5), MUTED, 0.14),
        false,
    );
    let mut top = inner.top() + TITLE;
    let mut chosen = None;
    for (action, label, hint) in ROWS {
        if action == BlockMenuAction::Remove {
            painter.hline(
                (inner.left() + 6.0)..=(inner.right() - 6.0),
                top + DIVIDER / 2.0,
                Stroke::new(1.0, HAIRLINE),
            );
            top += DIVIDER;
        }
        let row = Rect::from_min_size(
            egui::pos2(inner.left(), top),
            egui::vec2(inner.width(), ROW),
        );
        top += ROW;
        let response = menu_item(ui, row, label, label);
        if response.hovered() {
            painter.rect_filled(row, 6.0, HOVER);
        }
        let color = if action == BlockMenuAction::Remove {
            DANGER
        } else {
            TEXT
        };
        text_in(
            ui,
            Rect::from_min_max(
                egui::pos2(row.left() + 14.0, row.top()),
                egui::pos2(row.right() - 14.0, row.bottom()),
            ),
            tracked(label, bold(11.0), color, 0.06),
            false,
        );
        match hint {
            Hint::None => {}
            Hint::Text(text) => text_in(
                ui,
                Rect::from_min_max(
                    egui::pos2(row.center().x, row.top()),
                    egui::pos2(row.right() - 14.0, row.bottom()),
                ),
                RichText::new(text)
                    .font(FontId::monospace(10.0))
                    .color(MUTED),
                true,
            ),
            Hint::BackspaceKey => paint_backspace(
                &painter,
                egui::pos2(row.right() - 14.0, row.center().y),
                MUTED,
            ),
        }
        if response.clicked() {
            chosen = Some(action);
        }
    }
    chosen
}

/// The ⌫ key with its right edge at `right`: a box with a point on its left and an x in it.
fn paint_backspace(painter: &egui::Painter, right: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.1, color);
    let (half, width, point) = (4.5, 13.0, 4.5);
    let x = |dx: f32| right.x - width + dx;
    let y = right.y;
    painter.add(egui::epaint::PathShape::closed_line(
        vec![
            egui::pos2(x(0.0), y),
            egui::pos2(x(point), y - half),
            egui::pos2(right.x, y - half),
            egui::pos2(right.x, y + half),
            egui::pos2(x(point), y + half),
        ],
        stroke,
    ));
    let center = egui::pos2(x(point + (width - point) / 2.0), y);
    for sign in [-1.0, 1.0] {
        painter.line_segment(
            [
                center + egui::vec2(-2.0, -2.0 * sign),
                center + egui::vec2(2.0, 2.0 * sign),
            ],
            stroke,
        );
    }
}
