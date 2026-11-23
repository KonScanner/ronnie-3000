//! Stage header: logo, the chain as gear icons, and the preset manager.

use eframe::egui::{self, Align2, Color32, Rect, Stroke};
use ronnie_core::{ChainBlock, ChainSide};

use crate::backend::ChainViewState;
use crate::theme::Look;

use super::add_menu::AddMenu;
use super::block_menu::BlockMenu;

use super::super::{
    chain::GallerySelection,
    header::{ChromeController, draw_chevron, draw_plus},
    overlays::{DismissReason, Overlay},
    popover::Trigger,
    tile::{block_glyph, draw_glyph},
};

/// Space between the window edge and the preset manager.
const MARGIN: f32 = 24.0;
/// Left edge of the chain zone, after the logo and the name.
const STRIP_LEFT: f32 = 210.0;
/// Space between the chain zone and the preset manager. A hairline sits in its middle.
const ZONE_GAP: f32 = 32.0;
/// The preset manager takes this share of the header width, between the two limits.
const PRESETS_SHARE: f32 = 0.3;
const PRESETS_MIN: f32 = 300.0;
const PRESETS_MAX: f32 = 420.0;
/// One block button.
const BUTTON: egui::Vec2 = egui::vec2(52.0, 56.0);
/// Space between block buttons.
const BUTTON_GAP: f32 = 6.0;
/// Side of a gear glyph inside a block button.
const GLYPH: f32 = 38.0;
/// Side of the + button.
const PLUS: f32 = 40.0;
/// Space between the last block and the + button, so that the + reads as the chain's next slot.
const PLUS_GAP: f32 = 10.0;
/// Width of a scroll arrow at either end of the strip.
const ARROW: f32 = 18.0;

/// Where the parts of the header go for one window width and one chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HeaderLayout {
    /// The span that the blocks and, when the chain is too long, the scroll arrows take.
    strip: egui::Rangef,
    /// The left edge of the first block when the chain fits.
    first_block: f32,
    /// Whether the chain is too long for its zone and scrolls.
    overflow: bool,
    /// The + button, after the last block.
    plus: Rect,
    /// The preset manager.
    presets: egui::Rangef,
    /// The hairline between the chain zone and the preset manager.
    divider: f32,
}

impl HeaderLayout {
    /// The layout in the header `bounds` for blocks of `chain_width` in total.
    ///
    /// The preset manager keeps to the right edge and grows with the window between its limits.
    /// The chain and its + stay together as one group in the middle of the free zone. A chain
    /// too long for the zone fills it, scrolls, and keeps the + at the zone's right end.
    pub(super) fn new(bounds: Rect, chain_width: f32) -> Self {
        let presets_right = bounds.right() - MARGIN;
        let presets_width = (bounds.width() * PRESETS_SHARE).clamp(PRESETS_MIN, PRESETS_MAX);
        let presets = egui::Rangef::new(presets_right - presets_width, presets_right);
        let zone_left = bounds.left() + STRIP_LEFT;
        let zone_right = (presets.min - ZONE_GAP).max(zone_left + PLUS);
        let plus_room = PLUS + if chain_width > 0.0 { PLUS_GAP } else { 0.0 };
        let overflow = chain_width + plus_room > zone_right - zone_left;
        let (strip, first_block, plus_left) = if overflow {
            let strip = egui::Rangef::new(zone_left, zone_right - PLUS - PLUS_GAP);
            (strip, strip.min + ARROW, zone_right - PLUS)
        } else {
            let group = chain_width + plus_room;
            let left = zone_left + (zone_right - zone_left - group) / 2.0;
            (
                egui::Rangef::new(left, left + chain_width),
                left,
                left + group - PLUS,
            )
        };
        Self {
            strip,
            first_block,
            overflow,
            plus: Rect::from_min_size(
                egui::pos2(plus_left, bounds.center().y - PLUS / 2.0),
                egui::Vec2::splat(PLUS),
            ),
            presets,
            divider: presets.min - ZONE_GAP / 2.0,
        }
    }

    /// The layout of the header `bounds` for the chain in `state`.
    pub(super) fn of(bounds: Rect, state: &ChainViewState) -> Self {
        Self::new(bounds, chain_width(&strip_items(state)))
    }

    /// The + button's box.
    pub(super) const fn plus(&self) -> Rect {
        self.plus
    }
}

/// The width of the blocks and lane dividers in a row, with the gaps between them.
fn chain_width(items: &[StripItem<'_>]) -> f32 {
    items.iter().map(item_width).sum::<f32>() + BUTTON_GAP * items.len().saturating_sub(1) as f32
}

/// One block in strip order, with the lane divider before lane B.
enum StripItem<'a> {
    Block(&'a ChainBlock),
    Divider,
}

/// Draws the header into the current panel.
pub(super) fn show_header(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &ChainViewState,
) {
    let look = Look::get(ui.ctx());
    let bounds = ui.max_rect();
    let painter = ui.painter().clone();
    painter.hline(
        bounds.x_range(),
        bounds.bottom() - 0.5,
        Stroke::new(1.0, look.edge),
    );

    let layout = HeaderLayout::of(bounds, state);
    show_logo(ui, chrome, bounds, look);
    show_strip(ui, chrome, state, bounds, &layout, look);
    painter.vline(
        layout.divider,
        (bounds.top() + 18.0)..=(bounds.bottom() - 18.0),
        Stroke::new(1.0, look.edge),
    );
    show_presets(ui, chrome, state, bounds, layout.presets, look);
}

fn show_logo(ui: &mut egui::Ui, chrome: &mut ChromeController, bounds: Rect, look: Look) {
    let logo = Rect::from_min_size(
        bounds.left_top() + egui::vec2(24.0, 14.0),
        egui::Vec2::splat(36.0),
    );
    super::super::brand::draw_launcher_icon(ui, logo);
    let name = ui.painter().text(
        egui::pos2(logo.right() + 12.0, bounds.top() + 32.0),
        Align2::LEFT_CENTER,
        "RONNIE 3000",
        egui::FontId::new(15.0, egui::FontFamily::Name("arimo-bold".into())),
        look.text,
    );
    let home = ui
        .interact(
            logo.union(name),
            ui.scope_id().with("stage-home"),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    home.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Back to main screen"));
    if home.clicked() {
        chrome.go_to_main();
    }
}

fn strip_items(state: &ChainViewState) -> Vec<StripItem<'_>> {
    let mut items: Vec<StripItem<'_>> = state
        .chain
        .left
        .iter()
        .filter(|block| !block.is_insert)
        .map(StripItem::Block)
        .collect();
    if state.chain.stereo_enabled {
        let right: Vec<_> = state
            .chain
            .right
            .iter()
            .filter(|block| !block.is_insert)
            .collect();
        if !right.is_empty() {
            items.push(StripItem::Divider);
            items.extend(right.into_iter().map(StripItem::Block));
        }
    }
    items
}

fn item_width(item: &StripItem<'_>) -> f32 {
    match item {
        StripItem::Block(_) => BUTTON.x,
        StripItem::Divider => 9.0,
    }
}

fn show_strip(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &ChainViewState,
    bounds: Rect,
    layout: &HeaderLayout,
    look: Look,
) {
    let items = strip_items(state);
    let content_left = layout.strip.min;
    let content_right = layout.strip.max;
    let overflow = layout.overflow;

    // The first item shown, kept in memory so that the arrows can step through a long chain.
    let scroll_id = egui::Id::unique("stage-strip-first");
    let max_first = items.len().saturating_sub(1);
    let mut first: usize = ui
        .ctx()
        .data(|data| data.get_temp(scroll_id))
        .unwrap_or(0)
        .min(max_first);
    if !overflow {
        first = 0;
    }

    let (left, right) = if overflow {
        (layout.first_block, content_right - ARROW)
    } else {
        (layout.first_block, content_right)
    };

    let selected = match chrome.gallery().selection() {
        GallerySelection::BlockDetail(id) => Some(id.clone()),
        _ => None,
    };
    let painter = ui.painter().clone();
    let top = bounds.top() + (bounds.height() - BUTTON.y) / 2.0;
    let mut x = left;
    let mut last_shown = first;
    let mut toggle = None;
    let mut open = None;
    let mut menu_request = None;
    for (index, item) in items.iter().enumerate().skip(first) {
        let width = item_width(item);
        if x + width > right + 0.5 {
            break;
        }
        last_shown = index;
        match item {
            StripItem::Divider => {
                painter.vline(
                    x + width / 2.0,
                    (top + 12.0)..=(top + BUTTON.y - 12.0),
                    Stroke::new(1.0, look.edge),
                );
            }
            StripItem::Block(block) => {
                let rect = Rect::from_min_size(egui::pos2(x, top), BUTTON);
                let title = block
                    .tone
                    .as_ref()
                    .map_or_else(|| "Block".to_owned(), |tone| tone.title.clone());
                let label = format!("Chain: {title}");
                let response = ui
                    .interact(
                        rect,
                        ui.scope_id().with(("stage-block", &block.id.0)),
                        egui::Sense::click(),
                    )
                    .on_hover_text(&title);
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone())
                });
                let is_selected = selected.as_ref() == Some(&block.id);
                if response.hovered() {
                    painter.rect_filled(rect, 8.0, look.wash);
                }
                let color = if !block.params.enabled {
                    look.icon_bypassed
                } else if is_selected {
                    look.icon_selected
                } else {
                    look.icon_on
                };
                draw_glyph(
                    &painter,
                    Rect::from_center_size(rect.center(), egui::Vec2::splat(GLYPH)),
                    block_glyph(block),
                    color,
                );
                if is_selected {
                    painter.rect_filled(
                        Rect::from_center_size(
                            egui::pos2(rect.center().x, rect.bottom() - 1.0),
                            egui::vec2(20.0, 2.0),
                        ),
                        1.0,
                        look.icon_selected,
                    );
                }
                if response.double_clicked() {
                    toggle = Some((block.id.0.clone(), !block.params.enabled));
                } else if response.secondary_clicked() {
                    menu_request = Some((block.id.clone(), rect, title.clone()));
                } else if response.clicked() {
                    open = Some(block.id.0.clone());
                }
            }
        }
        x += width + BUTTON_GAP;
    }

    if overflow {
        let arrow_y = top + BUTTON.y / 2.0;
        let back = Rect::from_center_size(
            egui::pos2(content_left + ARROW / 2.0, arrow_y),
            egui::vec2(ARROW, 28.0),
        );
        let forward = Rect::from_center_size(
            egui::pos2(content_right - ARROW / 2.0, arrow_y),
            egui::vec2(ARROW, 28.0),
        );
        if first > 0 && arrow(ui, &painter, back, false, "Scroll blocks left", look) {
            first -= 1;
        }
        if last_shown + 1 < items.len()
            && arrow(ui, &painter, forward, true, "Scroll blocks right", look)
        {
            first = (first + 1).min(max_first);
        }
    }
    ui.ctx().data_mut(|data| data.insert_temp(scroll_id, first));

    show_plus(ui, chrome, state, layout.plus, look);

    if let Some((id, enabled)) = toggle {
        let _ = chrome.gallery_mut().set_enabled(&id, enabled);
    } else if let Some(id) = open {
        chrome.gallery_mut().activate(&id);
    }
    if let Some((id, rect, title)) = menu_request {
        let mut menu = BlockMenu::load(ui.ctx());
        menu.toggle(id, rect, title);
        menu.store(ui.ctx());
        if menu.is_open() {
            // One black menu at a time: the block menu takes over the add menu.
            let mut add = AddMenu::load(ui.ctx());
            add.open = false;
            add.store(ui.ctx());
        }
    }
}

fn arrow(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    right: bool,
    label: &'static str,
    look: Look,
) -> bool {
    let response = ui.interact(rect, ui.scope_id().with(label), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let color = if response.hovered() {
        look.text
    } else {
        look.text_muted
    };
    let (dx, tip) = if right { (-4.0, 4.0) } else { (4.0, -4.0) };
    let center = rect.center();
    painter.line(
        vec![
            center + egui::vec2(dx, -6.0),
            center + egui::vec2(tip, 0.0),
            center + egui::vec2(dx, 6.0),
        ],
        Stroke::new(1.8, color),
    );
    response.clicked()
}

fn show_plus(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &ChainViewState,
    rect: Rect,
    look: Look,
) {
    let painter = ui.painter().clone();
    let response = ui.interact(rect, ui.scope_id().with("stage-add"), egui::Sense::click());
    let mut menu = AddMenu::load(ui.ctx());
    // A slot's menu leaves the + dashed: the + did not open it.
    let solid = menu.opened_by_plus();
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, solid, "Add a block"));
    if solid {
        painter.rect_filled(rect, 6.0, look.solid);
        draw_plus(&painter, rect.shrink(10.0), look.solid_content);
    } else {
        let color = if response.hovered() {
            look.text
        } else {
            look.text_muted
        };
        let outline = vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
        ];
        painter.extend(egui::Shape::dashed_line(
            &outline,
            Stroke::new(1.5, color),
            4.0,
            3.0,
        ));
        draw_plus(&painter, rect.shrink(10.0), color);
    }
    if response.clicked() {
        if solid {
            menu.open = false;
        } else {
            let selected = match chrome.gallery().selection() {
                GallerySelection::BlockDetail(id) => Some(id.clone()),
                _ => None,
            };
            menu.open_for(lane_of(state, selected.as_ref()));
        }
        menu.store(ui.ctx());
    }
}

/// The lane that holds the selected block: lane B when the block sits there, else lane A.
pub(super) fn lane_of(
    state: &ChainViewState,
    selected: Option<&ronnie_core::BlockId>,
) -> ChainSide {
    if selected.is_some_and(|id| state.chain.right.iter().any(|block| &block.id == id)) {
        ChainSide::Right
    } else {
        ChainSide::Left
    }
}

/// The insert block that ends one lane.
pub(super) fn insert_of(state: &ChainViewState, side: ChainSide) -> Option<String> {
    let lane = match side {
        ChainSide::Left => &state.chain.left,
        ChainSide::Right => &state.chain.right,
    };
    lane.iter()
        .rev()
        .find(|block| block.is_insert)
        .map(|block| block.id.0.clone())
}

/// The insert block of the lane that holds the selected block.
pub(super) fn lane_insert(
    state: &ChainViewState,
    selected: Option<&ronnie_core::BlockId>,
) -> Option<String> {
    insert_of(state, lane_of(state, selected))
}

fn show_presets(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &ChainViewState,
    bounds: Rect,
    span: egui::Rangef,
    look: Look,
) {
    let painter = ui.painter().clone();
    let left = span.min;
    let right = span.max;
    let has_presets = !chrome.presets().is_empty();

    // Row 1: arrows on the left, text actions on the right.
    let row_top = bounds.top() + 6.0;
    let previous = hit(
        ui,
        Rect::from_min_size(egui::pos2(left, row_top), egui::vec2(24.0, 22.0)),
        "Previous preset",
        has_presets,
    );
    let next = hit(
        ui,
        Rect::from_min_size(egui::pos2(left + 30.0, row_top), egui::vec2(24.0, 22.0)),
        "Next preset",
        has_presets,
    );
    let chevron = chevron_color(has_presets, look);
    draw_chevron(&painter, previous.rect, false, chevron);
    draw_chevron(&painter, next.rect, true, chevron);
    painter.vline(
        left + 27.0,
        (row_top + 5.0)..=(row_top + 17.0),
        Stroke::new(1.0, look.edge),
    );
    if previous.clicked() {
        let _ = chrome.previous_preset();
    }
    if next.clicked() {
        let _ = chrome.next_preset();
    }

    // Undo and redo, after the preset arrows.
    let undo = hit(
        ui,
        Rect::from_min_size(egui::pos2(left + 66.0, row_top), egui::vec2(24.0, 22.0)),
        "Undo",
        state.can_undo,
    );
    let redo = hit(
        ui,
        Rect::from_min_size(egui::pos2(left + 92.0, row_top), egui::vec2(24.0, 22.0)),
        "Redo",
        state.can_redo,
    );
    draw_undo(
        &painter,
        undo.rect,
        false,
        if state.can_undo {
            look.text
        } else {
            look.text_muted
        },
    );
    draw_undo(
        &painter,
        redo.rect,
        true,
        if state.can_redo {
            look.text
        } else {
            look.text_muted
        },
    );
    if undo.clicked() {
        let _ = chrome.undo();
    }
    if redo.clicked() {
        let _ = chrome.redo();
    }

    let action_font = egui::FontId::new(11.0, egui::FontFamily::Name("arimo-bold".into()));
    let new_rect = Rect::from_min_max(
        egui::pos2(right - 40.0, row_top),
        egui::pos2(right, row_top + 22.0),
    );
    let save_rect = Rect::from_min_max(
        egui::pos2(new_rect.left() - 76.0, row_top),
        egui::pos2(new_rect.left() - 10.0, row_top + 22.0),
    );
    let save = hit(ui, save_rect, "Save preset", true);
    let new = hit(ui, new_rect, "New preset", true);
    painter.text(
        save_rect.right_center(),
        Align2::RIGHT_CENTER,
        "SAVE AS…",
        action_font.clone(),
        look.text,
    );
    painter.text(
        new_rect.right_center(),
        Align2::RIGHT_CENTER,
        "NEW",
        action_font,
        look.text,
    );
    if save.clicked() {
        if chrome.overlay() == Overlay::SavePreset {
            chrome.dismiss(DismissReason::Action);
        } else {
            chrome.open_save();
        }
    }
    if new.clicked() {
        let _ = chrome.reset();
    }

    // Row 2: the preset pill, truncated to fit.
    let pill = Rect::from_min_max(
        egui::pos2(left, bounds.top() + 31.0),
        egui::pos2(right, bounds.top() + 57.0),
    );
    painter.rect(
        pill,
        6.0,
        look.fill_soft,
        Stroke::new(1.0, look.control_edge),
        egui::StrokeKind::Inside,
    );
    let browse = hit(ui, pill, "Browse presets", true);
    let name = chrome.active_preset_name().unwrap_or("Presets").to_owned();
    let mut job = egui::text::LayoutJob::single_section(
        name,
        egui::TextFormat::simple(egui::FontId::proportional(13.0), look.text),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: pill.width() - 40.0,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    painter.galley(
        egui::pos2(pill.left() + 10.0, pill.center().y - galley.size().y / 2.0),
        galley,
        look.text,
    );
    let chevron = Rect::from_center_size(
        egui::pos2(pill.right() - 16.0, pill.center().y),
        egui::vec2(12.0, 12.0),
    );
    painter.line(
        vec![
            chevron.left_center() + egui::vec2(0.0, -2.0),
            chevron.center() + egui::vec2(0.0, 3.0),
            chevron.right_center() + egui::vec2(0.0, -2.0),
        ],
        Stroke::new(1.5, look.text),
    );
    if browse.clicked() {
        if chrome.overlay() == Overlay::PresetBrowser {
            chrome.dismiss(DismissReason::Action);
        } else {
            chrome.open_presets();
        }
    }
    match chrome.overlay() {
        Overlay::PresetBrowser => chrome.set_overlay_trigger(Some(Trigger::from(&browse))),
        Overlay::SavePreset => chrome.set_overlay_trigger(Some(Trigger::from(&save))),
        Overlay::None => chrome.set_overlay_trigger(None),
        Overlay::Account => {}
    }
}

/// The colour of a preset arrow: the layout text when it steps, and the muted text when the
/// preset list is empty. A fixed white would vanish on the light header.
fn chevron_color(has_presets: bool, look: Look) -> Color32 {
    if has_presets {
        look.text
    } else {
        look.text_muted
    }
}

/// A curved arrow: pointing left for undo, mirrored for redo.
fn draw_undo(painter: &egui::Painter, rect: Rect, mirrored: bool, color: Color32) {
    let center = rect.center();
    let sign = if mirrored { -1.0 } else { 1.0 };
    let radius = 5.0;
    let arc_center = center + egui::vec2(0.0, 1.0);
    let points: Vec<egui::Pos2> = (0..=12)
        .map(|step| {
            let angle = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * step as f32 / 12.0;
            arc_center + egui::vec2(sign * radius * angle.cos(), radius * angle.sin())
        })
        .collect();
    let start = points[0];
    let stroke = Stroke::new(1.6, color);
    painter.line(points.clone(), stroke);
    let tail = points[points.len() - 1];
    painter.line_segment([tail, tail + egui::vec2(-sign * 3.0, 0.0)], stroke);
    let tip = start + egui::vec2(-sign * 3.0, 0.0);
    painter.line_segment([start, tip], stroke);
    painter.line_segment([tip, tip + egui::vec2(sign * 3.0, -3.0)], stroke);
    painter.line_segment([tip, tip + egui::vec2(sign * 3.0, 3.0)], stroke);
}

fn hit(ui: &mut egui::Ui, rect: Rect, label: &'static str, enabled: bool) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(label),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(width: f32) -> Rect {
        Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, 64.0))
    }

    /// The width of `blocks` block buttons in one lane.
    fn blocks(count: usize) -> f32 {
        BUTTON.x * count as f32 + BUTTON_GAP * count.saturating_sub(1) as f32
    }

    #[test]
    fn the_plus_follows_the_last_block_of_the_chain() {
        for count in [1, 2, 4] {
            let layout = HeaderLayout::new(header(1400.0), blocks(count));
            assert!(!layout.overflow);
            assert_eq!(layout.strip.max, layout.first_block + blocks(count));
            assert_eq!(
                layout.plus.left(),
                layout.strip.max + PLUS_GAP,
                "{count} blocks"
            );
        }
    }

    #[test]
    fn the_chain_and_its_plus_sit_in_the_middle_of_the_free_zone() {
        let layout = HeaderLayout::new(header(1400.0), blocks(2));
        let zone_right = layout.presets.min - ZONE_GAP;
        let space_left = layout.first_block - STRIP_LEFT;
        let space_right = zone_right - layout.plus.right();
        assert!((space_left - space_right).abs() < 0.01);

        let empty = HeaderLayout::new(header(1400.0), 0.0);
        assert!((empty.plus.center().x - (STRIP_LEFT + zone_right) / 2.0).abs() < 0.01);
    }

    #[test]
    fn the_preset_manager_keeps_to_the_right_edge_and_grows_with_the_window() {
        let narrow = HeaderLayout::new(header(1024.0), blocks(2));
        let wide = HeaderLayout::new(header(1400.0), blocks(2));
        let huge = HeaderLayout::new(header(2400.0), blocks(2));
        assert_eq!(narrow.presets.max, 1024.0 - MARGIN);
        assert_eq!(wide.presets.max, 1400.0 - MARGIN);
        assert_eq!(
            narrow.presets.span(),
            PRESETS_MIN.max(1024.0 * PRESETS_SHARE)
        );
        assert!(wide.presets.span() > narrow.presets.span());
        assert_eq!(huge.presets.span(), PRESETS_MAX);
        for layout in [narrow, wide, huge] {
            assert!(layout.plus.right() < layout.divider);
            assert!(layout.divider < layout.presets.min);
        }
    }

    #[test]
    fn a_long_chain_scrolls_and_keeps_the_plus_at_the_end_of_its_zone() {
        let layout = HeaderLayout::new(header(1024.0), blocks(20));
        assert!(layout.overflow);
        assert!(
            layout.strip.min >= STRIP_LEFT && layout.strip.max < layout.plus.left(),
            "the strip {:?} stays between the name and the +",
            layout.strip
        );
        assert_eq!(layout.plus.right(), layout.presets.min - ZONE_GAP);
        assert_eq!(layout.strip.max + PLUS_GAP, layout.plus.left());
        assert_eq!(layout.first_block, layout.strip.min + ARROW);
    }

    #[test]
    fn a_preset_arrow_uses_the_colours_of_its_layout() {
        let light = Look::stage();
        assert_eq!(
            chevron_color(true, light),
            light.text,
            "an enabled arrow is the ink of the light header, not a fixed white"
        );
        assert_ne!(chevron_color(true, light), Color32::WHITE);
        assert_eq!(chevron_color(false, light), light.text_muted);

        let dark = Look::stage_dark();
        assert_eq!(chevron_color(true, dark), Color32::WHITE);
        assert_eq!(chevron_color(false, dark), dark.text_muted);
    }
}
