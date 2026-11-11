//! Lane geometry, drop targeting, and stereo controls.

use std::sync::LazyLock;

use eframe::egui::{self, Color32, Rect, Sense, Stroke};
use ronie_core::{ChainSide, params::ParameterId};

use super::chain::ChainGallery;
use crate::widgets::KnobSpec;

/// Routing-line continuation for an insert tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routing {
    /// No connector.
    None,
    /// Connector from the left.
    Left,
    /// Connector to the right.
    Right,
    /// Connectors on both sides.
    Both,
}

/// Computes one lane's pixel width, excluding edge fades.
#[must_use]
pub fn lane_width(count: usize, tile_size: i32) -> i32 {
    if count == 0 {
        0
    } else {
        count as i32 * tile_size + (count as i32 - 1) * 24
    }
}

/// Center of the connector gap before a tile.
#[must_use]
pub fn gap_center_x(gap_index: usize, tile_size: i32) -> f32 {
    gap_index as f32 * (tile_size + 24) as f32 - 12.0
}

/// Selects insert-tile connector lines by position.
#[must_use]
pub fn routing_for(index: usize, count: usize) -> Routing {
    if count <= 1 {
        Routing::None
    } else if index == 0 {
        Routing::Right
    } else if index + 1 == count {
        Routing::Left
    } else {
        Routing::Both
    }
}

/// Maps a lane-local x coordinate to an insertion index.
#[must_use]
pub fn slot_at(x: f32, count: usize, tile_size: i32) -> usize {
    if count == 0 {
        return 0;
    }
    let pitch = (tile_size + 24) as f32;
    let slot = (x / pitch).floor().max(0.0) as usize;
    let slot = slot.min(count - 1);
    if x >= lane_width(count, tile_size) as f32 {
        return count;
    }
    let center = slot as f32 * pitch + tile_size as f32 / 2.0;
    slot + usize::from(x > center)
}

/// Renders the stereo pan, solo, polarity, link, and lane-swap rail.
pub fn show_pan_rail(ui: &mut egui::Ui, gallery: &mut ChainGallery, mono_sum: bool) {
    let rail = ui.available_rect_before_wrap();
    let painter = ui.painter_at(rail);
    let center_x = rail.left() + 28.0;
    let region_height = (rail.height() - 28.0) / 2.0;
    let slack = ((region_height - 89.0 - 10.0) / 2.0).round();
    let left_top = rail.top() + slack;
    let pill = Rect::from_min_size(
        egui::pos2(rail.left(), rail.top() + region_height),
        egui::vec2(56.0, 28.0),
    );
    let right_top = pill.bottom() + (10.0 + slack).round();
    for (side, top) in [(ChainSide::Left, left_top), (ChainSide::Right, right_top)] {
        let wrap = Rect::from_min_size(egui::pos2(rail.left(), top), egui::vec2(56.0, 89.0));
        let mut pan_ui = ui.new_child(egui::UiBuilder::new().max_rect(wrap));
        pan_ui.add_enabled_ui(!mono_sum, |ui| {
            show_pan(ui, gallery, side);
        });
        let chips = Rect::from_min_size(
            egui::pos2(center_x - 20.0, top + 61.0 + 8.0),
            egui::vec2(40.0, 20.0),
        );
        show_rail_chips(&mut pan_ui, gallery, side, chips);
    }
    let left_bottom = left_top + 89.0;
    painter.line_segment(
        [
            egui::pos2(center_x, left_bottom + 10.0),
            egui::pos2(center_x, pill.top()),
        ],
        Stroke::new(1.0, crate::theme::palette::HAIRLINE),
    );
    painter.line_segment(
        [
            egui::pos2(center_x, pill.bottom()),
            egui::pos2(center_x, right_top - 10.0),
        ],
        Stroke::new(1.0, crate::theme::palette::HAIRLINE),
    );
    painter.rect_stroke(
        pill,
        14.0,
        Stroke::new(1.0, crate::theme::palette::HAIRLINE),
        egui::StrokeKind::Inside,
    );
    if mono_sum {
        painter.text(
            pill.center(),
            egui::Align2::CENTER_CENTER,
            "MONO",
            egui::FontId::monospace(11.0),
            crate::theme::palette::TEXT_SECONDARY,
        );
    } else {
        let linked = gallery.parameter("chainPanLinked") > 0.5;
        let link_rect =
            Rect::from_min_size(pill.min + egui::vec2(6.0, 4.0), egui::Vec2::splat(20.0));
        let link = ui.interact(link_rect, ui.scope_id().with("pan-link"), Sense::click());
        link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Link pans"));
        if link.clicked() {
            let _ = gallery.set_pan_linked(!linked);
        }
        draw_rail_icon(ui.painter(), link_rect, false, linked);
        let swap_rect =
            Rect::from_min_size(pill.min + egui::vec2(30.0, 4.0), egui::Vec2::splat(20.0));
        let swap = ui.interact(swap_rect, ui.scope_id().with("swap-chains"), Sense::click());
        swap.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Swap chains"));
        if swap.clicked() {
            let _ = gallery.swap_chains();
        }
        draw_rail_icon(ui.painter(), swap_rect, true, true);
    }
}

fn show_rail_chips(ui: &mut egui::Ui, gallery: &mut ChainGallery, side: ChainSide, rect: Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, crate::theme::palette::CONTROL_FILL);
    for index in 0..2 {
        let cell = Rect::from_min_size(
            rect.min + egui::vec2(index as f32 * 20.0, 0.0),
            egui::vec2(20.0, 20.0),
        );
        let (label, active) = if index == 0 {
            (
                "S",
                gallery.parameter(match side {
                    ChainSide::Left => "chainSoloLeft",
                    ChainSide::Right => "chainSoloRight",
                }) > 0.5,
            )
        } else {
            (
                "Ø",
                gallery.parameter(match side {
                    ChainSide::Left => "chainInvertLeft",
                    ChainSide::Right => "chainInvertRight",
                }) > 0.5,
            )
        };
        let response = ui.interact(
            cell,
            ui.scope_id().with(("rail-chip", side.as_str(), index)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                if index == 0 {
                    "Solo"
                } else {
                    "Invert polarity"
                },
            )
        });
        if active {
            painter.rect_filled(cell, 2.0, crate::theme::palette::BLAZE);
        }
        painter.text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::monospace(12.0),
            if active {
                Color32::BLACK
            } else {
                crate::theme::palette::TEXT_SECONDARY
            },
        );
        if response.clicked() {
            if index == 0 {
                let _ = gallery.toggle_solo(side);
            } else {
                let _ = gallery.toggle_invert(side);
            }
        }
    }
}

fn draw_rail_icon(painter: &egui::Painter, rect: Rect, swap: bool, enabled: bool) {
    let color = if enabled {
        Color32::WHITE
    } else {
        crate::theme::palette::TEXT_MUTED
    };
    let stroke = Stroke::new(7.0 / 6.0, color);
    let origin = rect.center() - egui::Vec2::splat(7.0);
    let point =
        |x: f64, y: f64| origin + egui::vec2((x * 14.0 / 24.0) as f32, (y * 14.0 / 24.0) as f32);
    if swap {
        painter.add(egui::epaint::PathShape::line(
            vec![point(21.0, 16.0), point(17.0, 20.0), point(13.0, 16.0)],
            stroke,
        ));
        painter.line_segment([point(17.0, 20.0), point(17.0, 4.0)], stroke);
        painter.add(egui::epaint::PathShape::line(
            vec![point(3.0, 8.0), point(7.0, 4.0), point(11.0, 8.0)],
            stroke,
        ));
        painter.line_segment([point(7.0, 4.0), point(7.0, 20.0)], stroke);
    } else {
        painter.add(egui::epaint::PathShape::line(
            vec![
                point(10.0, 13.0),
                point(10.72837, 13.78286),
                point(11.60655, 14.39294),
                point(12.59437, 14.80234),
                point(13.64665, 14.99233),
                point(14.71527, 14.95423),
                point(15.75135, 14.68978),
                point(16.70750, 14.21107),
                point(17.54, 13.54),
                point(20.54, 10.54),
                point(21.58750, 8.91949),
                point(21.94337, 7.02299),
                point(21.55460, 5.13297),
                point(20.47909, 3.53091),
                point(18.87703, 2.45540),
                point(16.98701, 2.06663),
                point(15.09051, 2.42250),
                point(13.47, 3.47),
                point(11.74, 5.17),
            ],
            stroke,
        ));
        painter.add(egui::epaint::PathShape::line(
            vec![
                point(14.0, 11.0),
                point(13.27163, 10.21714),
                point(12.39345, 9.60706),
                point(11.40563, 9.19766),
                point(10.35335, 9.00767),
                point(9.28473, 9.04577),
                point(8.24865, 9.31022),
                point(7.29250, 9.78893),
                point(6.46, 10.46),
                point(3.46, 13.46),
                point(2.38060, 15.08658),
                point(2.0, 17.0),
                point(2.38060, 18.91342),
                point(3.46447, 20.53553),
                point(5.08658, 21.61940),
                point(7.0, 22.0),
                point(8.91342, 21.61940),
                point(10.53553, 20.53553),
                point(12.25, 18.83),
            ],
            stroke,
        ));
    }
}

/// The knobs of the two lane pans, each with the travel of its half of the range.
static PAN_SPECS: LazyLock<[Option<KnobSpec>; 2]> = LazyLock::new(|| {
    [("chainPanLeft", 0.0, 0.5), ("chainPanRight", 0.5, 1.0)].map(|(id, low, high)| {
        KnobSpec::parameter(ParameterId(id)).map(|spec| spec.with_travel(low, high))
    })
});

fn show_pan(ui: &mut egui::Ui, gallery: &mut ChainGallery, side: ChainSide) {
    let size = egui::vec2(56.0, 63.0);
    let (_, space) = ui.allocate_space(size);
    let rect = ui.layout().align_size_within_rect(size, space);
    let (parameter, label, index) = match side {
        ChainSide::Left => ("chainPanLeft", "Pan L", 0),
        ChainSide::Right => ("chainPanRight", "Pan R", 1),
    };
    let Some(spec) = &PAN_SPECS[index] else {
        return;
    };
    // The ID comes from the side, so it does not change with the widgets before the knob.
    let id = ui.scope_id().with(("pan", side.as_str()));
    let value = f64::from(gallery.parameter(parameter));
    let output = crate::widgets::knob::show(ui, id, rect, spec, label, value);
    gallery.pan_knob_frame(side, ui.ctx().cumulative_pass_nr(), output.edit);
    let face = Rect::from_min_size(
        egui::pos2(rect.center().x - 18.0, rect.top()),
        egui::Vec2::splat(36.0),
    );
    crate::widgets::knob::paint_pan_knob(
        ui.painter(),
        face,
        spec.position(output.shown) as f32,
        side == ChainSide::Left,
    );
    ui.painter().text(
        egui::pos2(rect.center().x, rect.top() + 52.5),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(14.0),
        Color32::from_rgb(235, 235, 245),
    );
}
