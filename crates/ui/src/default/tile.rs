//! Tone and insert tile rendering with loading, failure, meter, and drop states.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, TextureHandle};
use ronie_core::ChainBlock;

use crate::{
    backend::BlockMeter,
    theme::palette,
    widgets::{KnobEdit, KnobSpec, knob::GAIN_RANGE_DB},
};

use super::{
    block::{HeaderIcon, draw_header_icon},
    chain::TileVisualState,
    lane::Routing,
};

/// Tile corner radius in design points.
const TILE_RADIUS: f32 = 16.0;
/// Side of the knob face of the tile Gain control.
const GAIN_KNOB: f32 = 28.0;
/// Space between a tile edge and its Gain control.
const GAIN_MARGIN: f32 = 6.0;

/// One action returned by a tile interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileAction {
    /// Open detail or tone selection.
    Open,
    /// Toggle block enable state.
    Power,
    /// Replace this tone through tone selection.
    Swap,
    /// Remove the block.
    Remove,
    /// Retry the failed model load.
    Retry,
}

/// Rendered tile and its pointer response.
pub struct TileFrame {
    /// Allocated tile rectangle.
    pub rect: Rect,
    /// Pointer response for drag and context-menu orchestration.
    pub response: egui::Response,
    /// At most one action selected during this frame.
    pub action: Option<TileAction>,
    /// An edit of the normalized input gain from the Gain knob.
    pub gain: Option<KnobEdit>,
    /// Whether this frame showed the Gain knob. A tone that loads or that failed has none.
    pub gain_shown: bool,
}

/// Gear fallback identities used when artwork is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GearGlyph {
    /// Amplifier and speaker cabinet.
    FullRig,
    /// Amplifier head.
    Amp,
    /// Speaker cabinet.
    Cabinet,
    /// Stompbox.
    Pedal,
    /// Rack equipment.
    Outboard,
    /// Impulse-response space.
    Space,
    /// Unknown equipment.
    Generic,
    /// A recognised gear family, such as a chime combo or a vintage 4x12.
    Family(ronie_core::gear::GearFamily),
}

/// Resolves a persisted gear string to its fallback glyph.
#[must_use]
pub fn gear_glyph(gear: &str) -> GearGlyph {
    match gear {
        "amp+cab" | "full-rig" => GearGlyph::FullRig,
        "amp" => GearGlyph::Amp,
        "cab" | "cabinet" => GearGlyph::Cabinet,
        "pedal" => GearGlyph::Pedal,
        "outboard" => GearGlyph::Outboard,
        "space" | "spaces" | "ir" => GearGlyph::Space,
        _ => GearGlyph::Generic,
    }
}

/// Resolves a tone, and optionally its active model, to its glyph.
///
/// A capture recognised by name or metadata shows its gear family. Anything else keeps the
/// category glyph from the tone's `gear` value.
#[must_use]
pub fn tone_glyph(tone: &ronie_core::Tone, model: Option<&ronie_core::ToneModel>) -> GearGlyph {
    use ronie_core::gear::{GearFamily as F, GearHints, MatchSource, identify};

    let hints = GearHints::from_tone(tone, model);
    let identity = identify(&hints);
    let recognised = matches!(
        identity.source,
        MatchSource::NamMetadata | MatchSource::Catalog | MatchSource::Title
    );
    let has_icon = matches!(
        identity.family,
        F::OdGreen
            | F::OdGold
            | F::Distortion
            | F::Fuzz
            | F::Boost
            | F::Compressor
            | F::HeadHighGain
            | F::HeadModernUs
            | F::HeadBritish
            | F::ComboChime
            | F::ComboClean
            | F::HeadBass
            | F::Cab412Modern
            | F::Cab412Vintage
            | F::Cab212
            | F::Cab112
            | F::Cab810
    );
    if recognised && has_icon {
        GearGlyph::Family(identity.family)
    } else {
        gear_glyph(hints.gear.as_deref().unwrap_or_default())
    }
}

/// Resolves a chain block to its glyph, using the block's active model.
#[must_use]
pub fn block_glyph(block: &ChainBlock) -> GearGlyph {
    let Some(tone) = block.tone.as_ref() else {
        return GearGlyph::Generic;
    };
    let model = block
        .active_model_id
        .and_then(|id| tone.models.iter().find(|model| model.id == id));
    tone_glyph(tone, model)
}

/// Maps dB from the -60 floor to a bounded linear meter fraction.
#[must_use]
pub fn meter_fraction(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Centered UV crop for object-fit cover rendering.
#[must_use]
pub fn cover_uv(source: [usize; 2], target: [usize; 2]) -> Rect {
    if source.contains(&0) || target.contains(&0) {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let source_aspect = source[0] as f32 / source[1] as f32;
    let target_aspect = target[0] as f32 / target[1] as f32;
    if source_aspect > target_aspect {
        let width = target_aspect / source_aspect;
        let inset = (1.0 - width) / 2.0;
        Rect::from_min_max(Pos2::new(inset, 0.0), Pos2::new(inset + width, 1.0))
    } else {
        let height = source_aspect / target_aspect;
        let inset = (1.0 - height) / 2.0;
        Rect::from_min_max(Pos2::new(0.0, inset), Pos2::new(1.0, inset + height))
    }
}

/// Renders one tile and returns its pointer response and at most one action.
pub fn show(
    ui: &mut egui::Ui,
    block: &ChainBlock,
    size: i32,
    state: TileVisualState,
    meter: Option<&BlockMeter>,
    artwork: Option<&TextureHandle>,
    routing: Routing,
) -> TileFrame {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(size as f32, size as f32),
        Sense::click_and_drag(),
    );
    let accessible_name = if block.is_insert {
        "Add Tone".to_owned()
    } else {
        block.tone.as_ref().map_or_else(
            || format!("Tone block {}", block.id.0),
            |tone| tone.title.clone(),
        )
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, accessible_name.clone())
    });
    let painter = ui.painter_at(rect);
    let border = if state == TileVisualState::DropArmed {
        Color32::from_rgba_unmultiplied(
            palette::FLAME.r(),
            palette::FLAME.g(),
            palette::FLAME.b(),
            128,
        )
    } else if block.is_insert {
        Color32::from_rgba_unmultiplied(141, 141, 147, 166)
    } else {
        Color32::from_rgb(70, 70, 76)
    };
    let background = if block.is_insert {
        palette::SURFACE_RAISED
    } else {
        palette::PANEL
    };
    painter.rect_filled(rect, CornerRadius::same(16), background);

    if state == TileVisualState::DropArmed {
        draw_upload(&painter, rect);
    } else if block.is_insert {
        draw_insert(&painter, rect, routing);
    } else {
        if let Some(texture) = artwork {
            let tint = if block.params.enabled && state == TileVisualState::Ready {
                Color32::WHITE
            } else {
                Color32::from_white_alpha(89)
            };
            egui::Image::from_texture(texture)
                .uv(cover_uv(
                    texture.size(),
                    [rect.width() as usize, rect.height() as usize],
                ))
                .tint(tint)
                .corner_radius(CornerRadius::same(16))
                .paint_at(ui, rect);
        } else {
            draw_gear(&painter, rect, block_glyph(block));
            if !block.params.enabled || state != TileVisualState::Ready {
                painter.rect_filled(rect, CornerRadius::same(16), palette::ART_SCRIM);
            }
        }
    }

    if artwork.is_none() {
        draw_energy_glow(&painter, rect, meter);
    }
    if block.is_insert || state == TileVisualState::DropArmed {
        dashed_border(&painter, rect, border);
    }

    let mut action = None;
    let mut gain = None;
    let mut gain_shown = false;
    if !block.is_insert && state != TileVisualState::DropArmed {
        match state {
            TileVisualState::Loading => draw_loading(ui, rect),
            TileVisualState::Failed => {
                let group_top = rect.center().y - 24.0;
                painter.text(
                    egui::pos2(rect.center().x, group_top),
                    egui::Align2::CENTER_TOP,
                    "Download failed",
                    egui::FontId::proportional(11.0),
                    Color32::from_rgba_unmultiplied(255, 255, 255, 230),
                );
                let badge = Rect::from_min_size(
                    egui::pos2(rect.center().x - 35.0, group_top + 21.0),
                    egui::vec2(70.0, 26.0),
                );
                painter.rect_filled(badge, CornerRadius::same(13), Color32::WHITE);
                let icon = badge.left_center() + egui::vec2(17.0, 0.0);
                let arc = (0..=20)
                    .map(|step| {
                        let angle = (45.0 + step as f32 * 270.0 / 20.0).to_radians();
                        icon + egui::vec2(angle.cos(), angle.sin()) * 5.0
                    })
                    .collect();
                painter.add(egui::Shape::line(arc, Stroke::new(1.2, Color32::BLACK)));
                painter.line_segment(
                    [icon + egui::vec2(1.0, -7.0), icon + egui::vec2(5.0, -5.0)],
                    Stroke::new(1.2, Color32::BLACK),
                );
                painter.line_segment(
                    [icon + egui::vec2(1.0, -7.0), icon + egui::vec2(1.0, -2.5)],
                    Stroke::new(1.2, Color32::BLACK),
                );
                painter.text(
                    badge.center() + egui::vec2(7.0, 0.0),
                    egui::Align2::CENTER_CENTER,
                    "Retry",
                    egui::FontId::proportional(12.0),
                    Color32::BLACK,
                );
                let retry = ui.interact(
                    badge,
                    ui.scope_id().with(("retry", &block.id.0)),
                    Sense::click(),
                );
                retry.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, "Retry model load")
                });
                if retry.clicked() {
                    action = Some(TileAction::Retry);
                }
            }
            TileVisualState::Ready | TileVisualState::DropArmed => {}
        }
        draw_meter(&painter, rect, meter);
        // A tone that loads or that failed to load has no gain to set. A bypassed tone keeps it.
        let knob = (state == TileVisualState::Ready).then(|| gain_control(ui, block, rect));
        gain_shown = knob.is_some();
        gain = knob.as_ref().and_then(|(_, change)| *change);
        // The knob is a part of the tile, so the pointer on it keeps the quick actions.
        let knob_in_use = knob
            .as_ref()
            .is_some_and(|(knob, _)| knob.contains_pointer() || knob.dragged());
        if knob_in_use || shows_quick_actions(ui, &response) {
            action = action.or_else(|| draw_quick_actions(ui, block, rect));
        }
    }
    if action.is_none() && response.clicked() {
        action = Some(TileAction::Open);
    }
    TileFrame {
        rect,
        response,
        action,
        gain,
        gain_shown,
    }
}

/// The block input gain in decibels, for a normalized value.
#[must_use]
pub fn gain_db(normalized: f64) -> f64 {
    (normalized.clamp(0.0, 1.0) - 0.5) * 2.0 * GAIN_RANGE_DB
}

/// The text of a gain in decibels, for example "+6.0 dB", "-2.5 dB", or "0.0 dB".
///
/// The text is the same as the text of a gain knob, from [`crate::widgets::format_db`].
#[must_use]
pub fn format_gain_db(normalized: f64) -> String {
    crate::widgets::format_db(gain_db(normalized))
}

/// The small Gain knob at the bottom left of a tone tile.
///
/// Its own response is on top of the tile, so a drag on it moves no tile and a click on it
/// opens no tone page. The label shows the value in decibels during a drag and while the knob
/// has keyboard focus. The gestures are those of each knob, from [`crate::widgets::knob`].
///
/// The label shows the value of this frame, with the edit of this frame. The area that takes
/// the pointer has the size of the label before the edit, and the next frame makes it the same.
fn gain_control(
    ui: &mut egui::Ui,
    block: &ChainBlock,
    tile: Rect,
) -> (egui::Response, Option<KnobEdit>) {
    let value = block.params.input_gain;
    let id = ui.scope_id().with(("tile-gain", &block.id.0));
    let shows_value = ui.ctx().dragged_id() == Some(id) || ui.memory(|memory| memory.has_focus(id));
    let (_, area) = gain_label(ui, tile, shows_value, value);
    let title = block
        .tone
        .as_ref()
        .map_or(block.id.0.as_str(), |tone| tone.title.as_str());
    let spec = KnobSpec::block_gain();
    let output = crate::widgets::knob::show(ui, id, area, &spec, &format!("{title} gain"), value);
    let shows_value = output.response.dragged() || output.response.has_focus();
    let (galley, plate) = gain_label(ui, tile, shows_value, output.shown);
    let painter = ui.painter_at(tile);
    painter.rect_filled(plate, CornerRadius::same(10), palette::ART_SCRIM);
    let face = Rect::from_min_size(
        plate.min + egui::vec2(4.0, 4.0),
        egui::Vec2::splat(GAIN_KNOB),
    );
    crate::widgets::knob::paint_knob(
        &painter,
        face,
        spec.position(output.shown) as f32,
        spec.bipolar(),
    );
    painter.galley(
        egui::pos2(face.right() + 6.0, plate.center().y - galley.size().y / 2.0),
        galley,
        Color32::WHITE,
    );
    (output.response, output.edit)
}

/// The text of the Gain label and the plate under the knob and the label.
fn gain_label(
    ui: &egui::Ui,
    tile: Rect,
    shows_value: bool,
    value: f64,
) -> (std::sync::Arc<egui::Galley>, Rect) {
    let text = if shows_value {
        format!("Gain {}", format_gain_db(value))
    } else {
        "Gain".to_owned()
    };
    let galley = ui.ctx().fonts_mut(|fonts| {
        fonts.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE)
    });
    let plate = Rect::from_min_size(
        egui::pos2(
            tile.left() + GAIN_MARGIN,
            tile.bottom() - GAIN_MARGIN - GAIN_KNOB - 8.0,
        ),
        egui::vec2(
            GAIN_KNOB + 8.0 + 6.0 + galley.size().x + 10.0,
            GAIN_KNOB + 8.0,
        ),
    );
    (galley, plate)
}

/// Whether the quick actions of this tile must exist in this frame.
///
/// This is not `Response::hovered`. In the frame of a click, egui reports only the clicked widget
/// as hovered. A button that exists only while the tile is hovered is then absent in the one frame
/// that delivers its click.
///
/// No tile shows them during a drag. The carried tile follows the pointer in the drag preview, and
/// its buttons are of no use while it moves.
fn shows_quick_actions(ui: &egui::Ui, response: &egui::Response) -> bool {
    ui.ctx().dragged_id().is_none() && response.contains_pointer()
}

fn draw_quick_actions(ui: &mut egui::Ui, block: &ChainBlock, rect: Rect) -> Option<TileAction> {
    const BUTTON: f32 = 28.0;
    const MARGIN: f32 = 4.0;
    let strip = Rect::from_min_size(rect.min, egui::vec2(rect.width(), BUTTON + 2.0 * MARGIN));
    let painter = ui.painter_at(strip);
    painter.rect_filled(
        strip,
        CornerRadius {
            nw: 16,
            ne: 16,
            sw: 0,
            se: 0,
        },
        Color32::from_black_alpha(90),
    );
    let button = |offset: egui::Vec2| {
        Rect::from_min_size(strip.left_top() + offset, egui::Vec2::splat(BUTTON))
    };
    let power = button(egui::vec2(MARGIN, MARGIN));
    let remove = button(egui::vec2(rect.width() - MARGIN - BUTTON, MARGIN));
    let swap = button(egui::vec2(rect.width() - 2.0 * (MARGIN + BUTTON), MARGIN));
    let power_color = if block.params.enabled {
        Color32::WHITE
    } else {
        palette::TEXT_MUTED
    };
    let mut action = None;
    for (area, label, icon, color, picked, hover_fill) in [
        (
            power,
            "Block power",
            HeaderIcon::Power,
            power_color,
            TileAction::Power,
            palette::PLATE_HOVER,
        ),
        (
            swap,
            "Swap tone",
            HeaderIcon::Swap,
            Color32::WHITE,
            TileAction::Swap,
            palette::PLATE_HOVER,
        ),
        (
            remove,
            "Remove block",
            HeaderIcon::Remove,
            Color32::WHITE,
            TileAction::Remove,
            palette::FIRE,
        ),
    ] {
        let response = ui.interact(
            area,
            ui.scope_id().with((label, &block.id.0)),
            Sense::click(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        // A plate keeps the icon readable on bright artwork.
        let fill = if response.hovered() {
            hover_fill
        } else {
            palette::ART_SCRIM
        };
        painter.rect_filled(area, CornerRadius::same(8), fill);
        draw_header_icon(&painter, area, icon, color, false);
        if response.clicked() {
            action = Some(picked);
        }
    }
    action
}

fn draw_loading(ui: &egui::Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    for x in [-12.0, 0.0, 12.0] {
        painter.circle_filled(rect.center() + egui::vec2(x, 0.0), 3.0, palette::CHROME);
    }
}

fn draw_meter(painter: &egui::Painter, rect: Rect, meter: Option<&BlockMeter>) {
    if meter.is_some_and(|meter| meter.output_db >= 0.0) {
        painter.circle_filled(
            rect.right_bottom() - egui::vec2(13.0, 13.0),
            5.0,
            palette::FIRE,
        );
    }
}

fn draw_insert(painter: &egui::Painter, rect: Rect, routing: Routing) {
    let center = rect.center();
    let radius = if rect.width() <= 160.0 { 20.0 } else { 24.0 };
    let y = center.y;
    if matches!(routing, Routing::Left | Routing::Both) {
        painter.line_segment(
            [
                egui::pos2(rect.left() + 2.0, y),
                egui::pos2(center.x - radius + 4.0, y),
            ],
            Stroke::new(2.0, Color32::WHITE),
        );
    }
    if matches!(routing, Routing::Right | Routing::Both) {
        painter.line_segment(
            [
                egui::pos2(center.x + radius - 4.0, y),
                egui::pos2(rect.right() - 2.0, y),
            ],
            Stroke::new(2.0, Color32::WHITE),
        );
    }
    painter.circle_stroke(center, radius - 4.0, Stroke::new(2.0, Color32::WHITE));
    painter.line_segment(
        [
            center - egui::vec2(radius * 0.45, 0.0),
            center + egui::vec2(radius * 0.45, 0.0),
        ],
        Stroke::new(2.0, Color32::WHITE),
    );
    painter.line_segment(
        [
            center - egui::vec2(0.0, radius * 0.45),
            center + egui::vec2(0.0, radius * 0.45),
        ],
        Stroke::new(2.0, Color32::WHITE),
    );
}

fn draw_upload(painter: &egui::Painter, rect: Rect) {
    let center = rect.center();
    let color = palette::TEXT_MUTED;
    let stroke = Stroke::new(2.0, color);
    painter.line_segment(
        [
            center + egui::vec2(0.0, 16.0),
            center - egui::vec2(0.0, 14.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center - egui::vec2(0.0, 14.0),
            center - egui::vec2(10.0, 2.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center - egui::vec2(0.0, 14.0),
            center + egui::vec2(10.0, -2.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center + egui::vec2(-16.0, 18.0),
            center + egui::vec2(16.0, 18.0),
        ],
        stroke,
    );
}

fn draw_energy_glow(painter: &egui::Painter, rect: Rect, meter: Option<&BlockMeter>) {
    let Some(meter) = meter else {
        return;
    };
    let db = meter.output_db;
    let unit = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let gate = ((db + 50.0) / 10.0).clamp(0.0, 1.0);
    let color = palette::fire_ramp(unit);
    let level = [color.r(), color.g(), color.b()];
    let presence = gate * unit;
    if presence <= 0.0 {
        return;
    }
    let scale = rect.width() / 224.0;
    let blur = (2.0 + 16.0 * presence) * scale;
    painter.add(inner_glow(rect, blur, level, 0.375 * presence));
}

/// One smooth inner glow, because stacked translucent strokes show hard rings.
fn inner_glow(rect: Rect, blur: f32, rgb: [u8; 3], peak_alpha: f32) -> egui::Shape {
    const RINGS: u32 = 8;
    let mut mesh = egui::Mesh::default();
    let mut ring_len = 0;
    for ring in 0..RINGS {
        let fraction = ring as f32 / (RINGS - 1) as f32;
        let falloff = 1.0 - fraction;
        let alpha = (peak_alpha * falloff * falloff * 255.0).round() as u8;
        let color = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], alpha);
        let inset = blur * fraction;
        let points = rounded_rect_ring(rect.shrink(inset), (TILE_RADIUS - inset).max(0.0));
        ring_len = points.len() as u32;
        for point in points {
            mesh.colored_vertex(point, color);
        }
    }
    for ring in 0..RINGS - 1 {
        for index in 0..ring_len {
            let next = (index + 1) % ring_len;
            let outer = ring * ring_len;
            let inner = (ring + 1) * ring_len;
            mesh.add_triangle(outer + index, outer + next, inner + index);
            mesh.add_triangle(inner + index, outer + next, inner + next);
        }
    }
    egui::Shape::mesh(mesh)
}

/// Clockwise outline of a rounded rectangle, with a fixed point count for any radius.
fn rounded_rect_ring(rect: Rect, radius: f32) -> Vec<egui::Pos2> {
    const CORNER_SEGMENTS: usize = 6;
    let corners = [
        (rect.right_top() + egui::vec2(-radius, radius), -90.0_f32),
        (rect.right_bottom() + egui::vec2(-radius, -radius), 0.0),
        (rect.left_bottom() + egui::vec2(radius, -radius), 90.0),
        (rect.left_top() + egui::vec2(radius, radius), 180.0),
    ];
    let mut points = Vec::with_capacity(corners.len() * (CORNER_SEGMENTS + 1));
    for (center, start_degrees) in corners {
        for segment in 0..=CORNER_SEGMENTS {
            let degrees = start_degrees + 90.0 * segment as f32 / CORNER_SEGMENTS as f32;
            let (sin, cos) = degrees.to_radians().sin_cos();
            points.push(center + radius * egui::vec2(cos, sin));
        }
    }
    points
}

/// Paints a gear glyph centred in `rect`, sized for a tile of that width.
pub fn draw_gear(painter: &egui::Painter, rect: Rect, glyph: GearGlyph) {
    let glyph_rect = Rect::from_center_size(
        rect.center(),
        egui::Vec2::splat(64.0 * rect.width() / 224.0),
    );
    draw_glyph(painter, glyph_rect, glyph, palette::TEXT_MUTED);
}

/// Paints a gear glyph filling `glyph_rect` in `color`.
pub fn draw_glyph(painter: &egui::Painter, glyph_rect: Rect, glyph: GearGlyph, color: Color32) {
    let center = glyph_rect.center();
    let scale = glyph_rect.width() / 64.0;
    match glyph {
        GearGlyph::FullRig => super::gear::draw(painter, glyph_rect, "full-rig", color),
        GearGlyph::Amp => super::gear::draw(painter, glyph_rect, "amp", color),
        GearGlyph::Cabinet => super::gear::draw(painter, glyph_rect, "cab", color),
        GearGlyph::Pedal => super::gear::draw(painter, glyph_rect, "pedal", color),
        GearGlyph::Outboard => super::gear::draw(painter, glyph_rect, "outboard", color),
        GearGlyph::Space => super::gear::draw(painter, glyph_rect, "space", color),
        GearGlyph::Family(family) => super::gear::draw_family(painter, glyph_rect, family, color),
        GearGlyph::Generic => {
            let stroke = Stroke::new((2.0 * scale).max(1.0), color);
            for (index, y) in [-14.0, 0.0, 14.0].into_iter().enumerate() {
                let width = (48.0 - index as f32 * 10.0) * scale;
                painter.line_segment(
                    [
                        center - egui::vec2(width, y * scale),
                        center + egui::vec2(width, y * scale),
                    ],
                    stroke,
                );
            }
        }
    }
}

fn dashed_border(painter: &egui::Painter, rect: Rect, color: Color32) {
    const STROKE_WIDTH: f32 = 2.0;
    // Inset by half the stroke so that the dashes follow the rounded tile fill.
    let inset = STROKE_WIDTH / 2.0;
    let mut outline = rounded_rect_ring(rect.shrink(inset), TILE_RADIUS - inset);
    if let Some(first) = outline.first().copied() {
        outline.push(first);
    }
    painter.extend(egui::Shape::dashed_line(
        &outline,
        Stroke::new(STROKE_WIDTH, color),
        7.0,
        5.0,
    ));
}
