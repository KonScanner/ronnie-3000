//! Tone-card artwork, metadata, and actions.

use eframe::egui::{self, Color32, Rect, Sense, TextureHandle};
use ronie_core::{Tone, ToneId};

use super::tile::{cover_uv, draw_gear, tone_glyph};
use crate::theme::palette;

/// Side of the square that takes a click on the favorite heart.
const HEART_HIT_SIDE: f32 = 28.0;
/// Width of the drawn heart.
const HEART_WIDTH: f32 = 16.0;
/// Line width of the drawn heart.
const HEART_STROKE: f32 = 1.25;
/// Straight parts in each curve of the outline of the heart.
const HEART_CURVE_STEPS: usize = 8;

/// Action selected on a tone card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneCardAction {
    /// Open this tone for loading.
    Open,
    /// Toggle this tone's favorite state.
    Favorite(ToneId),
}

pub(super) struct ToneCardPaint<'a> {
    pub(super) artwork: Option<&'a TextureHandle>,
    pub(super) avatar_texture: Option<&'a TextureHandle>,
    pub(super) scale: f32,
    pub(super) content_height: f32,
    pub(super) pending_tone: Option<ToneId>,
    pub(super) busy_backdrop: Option<&'a TextureHandle>,
    /// Whether the picked card paints its busy state. It waits while the window captures it.
    pub(super) busy_overlay: bool,
    /// Favorite state of the tone for a signed-in user. `None` draws no heart.
    pub(super) favorite: Option<bool>,
}

/// Renders one measured browser card and returns at most one action.
pub(super) fn show_at(
    ui: &mut egui::Ui,
    rect: Rect,
    tone: &Tone,
    paint: ToneCardPaint<'_>,
) -> Option<ToneCardAction> {
    let ToneCardPaint {
        artwork,
        avatar_texture,
        scale,
        content_height,
        pending_tone,
        busy_backdrop,
        busy_overlay,
        favorite,
    } = paint;
    let picked = pending_tone == Some(tone.id);
    let disabled = pending_tone.is_some();
    let response = ui.allocate_rect(
        rect,
        if disabled {
            Sense::hover()
        } else {
            Sense::click()
        },
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tone.title.clone()));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 12.0 * scale, palette::PANEL);
    let inner_height = content_height - 24.0 * scale;
    let image_y = rect.top() + 12.0 * scale + ((inner_height - 112.0 * scale) / 2.0).round();
    let image_rect = Rect::from_min_size(
        egui::pos2(rect.left() + 12.0 * scale, image_y),
        egui::Vec2::splat(112.0 * scale),
    );
    painter.rect_filled(image_rect, 8.0 * scale, palette::SURFACE_RAISED);
    if let Some(texture) = artwork {
        egui::Image::from_texture(texture)
            .uv(cover_uv(texture.size(), [112, 112]))
            .corner_radius(8.0 * scale)
            .paint_at(ui, image_rect);
    } else {
        draw_gear(&painter, image_rect, tone_glyph(tone, None));
    }

    let text_x = rect.left() + 140.0 * scale;
    let available_width = (rect.right() - 12.0 * scale - text_x).max(20.0);
    let title = title_galley(ui, tone, available_width, scale);
    let title_height = title_height(ui, tone, available_width, scale);
    let body_line = crate::fonts::normal_line_height(13.0) as f32 * scale;
    let gear_height = body_line.max(18.0 * scale);
    let stats_height = body_line.max(14.0 * scale);
    let creator_height = if tone.raw.get("user").is_some() {
        30.0 * scale
    } else {
        0.0
    };
    let column_height = title_height + 16.0 * scale + gear_height + stats_height + creator_height;
    let title_y = rect.top() + 12.0 * scale + (inner_height - column_height) / 2.0;
    let gear_y = title_y + title_height + 8.0 * scale;
    let stats_y = gear_y + gear_height + 8.0 * scale;
    let creator_y = stats_y + stats_height + 8.0 * scale;
    let title_painter = painter.with_clip_rect(Rect::from_min_size(
        egui::pos2(text_x, title_y),
        egui::vec2(available_width, title_height),
    ));
    if text_width(ui, &tone.title, 14.0 * scale) > available_width {
        title_painter.galley(egui::pos2(text_x, title_y + scale), title, Color32::WHITE);
    } else {
        title_painter.text(
            egui::pos2(text_x, title_y + scale),
            egui::Align2::LEFT_TOP,
            tone.title.as_str(),
            egui::FontId::new(14.0 * scale, egui::FontFamily::Name("arimo-bold".into())),
            Color32::WHITE,
        );
    }
    let gear = gear_label(tone);
    painter.text(
        egui::pos2(text_x, gear_y + gear_height / 2.0),
        egui::Align2::LEFT_CENTER,
        gear,
        egui::FontId::proportional(13.0 * scale),
        muted(),
    );
    let badge_x = text_x + text_width(ui, gear, 13.0 * scale) + 10.0 * scale;
    let nam = tone.kind == ronie_core::ModelKind::Nam;
    let badge = Rect::from_min_size(
        egui::pos2(
            badge_x.round(),
            (gear_y + (gear_height - 18.0 * scale) / 2.0).round(),
        ),
        egui::vec2(if nam { 34.0 } else { 27.0 } * scale, 18.0 * scale),
    );
    painter.rect_filled(badge, 2.0 * scale, Color32::from_rgb(161, 161, 170));
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        if nam { "NAM" } else { "IR" },
        egui::FontId::monospace(12.0 * scale),
        Color32::BLACK,
    );
    if nam && model_count(tone) > 0 {
        let mark = Rect::from_center_size(
            egui::pos2(badge.right() + 19.0 * scale, gear_y + gear_height / 2.0),
            egui::Vec2::splat(18.0 * scale),
        );
        super::brand::draw_architecture_chip(&painter, mark);
    }

    let downloads = tone
        .raw
        .get("downloads_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let models = model_count(tone);
    let mut stat_x = text_x;
    for (index, count) in [downloads, models].into_iter().enumerate() {
        stat_icon(
            &painter,
            egui::pos2(stat_x + 7.0 * scale, stats_y + stats_height / 2.0),
            index,
            scale,
        );
        let count = compact_count(count);
        painter.text(
            egui::pos2(stat_x + 20.0 * scale, stats_y + stats_height / 2.0),
            egui::Align2::LEFT_CENTER,
            &count,
            egui::FontId::proportional(13.0 * scale),
            muted(),
        );
        stat_x += (20.0 + 16.0) * scale + text_width(ui, &count, 13.0 * scale);
    }

    // The end of the statistics row is the one place that a long title or name cannot reach.
    let heart_clicked = favorite.is_some_and(|favorite| {
        let center = egui::pos2(
            rect.right() - 12.0 * scale - HEART_HIT_SIDE * scale / 2.0,
            stats_y + stats_height / 2.0,
        );
        favorite_heart(ui, &painter, tone, center, favorite, scale, !disabled)
    });

    if let Some(user) = tone.raw.get("user") {
        let name = user
            .get("display_name")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
            .or_else(|| user.get("username").and_then(serde_json::Value::as_str))
            .unwrap_or("Unknown");
        let avatar = Rect::from_center_size(
            egui::pos2(text_x + 11.0 * scale, creator_y + 11.0 * scale),
            egui::Vec2::splat(22.0 * scale),
        );
        if let Some(texture) = avatar_texture {
            egui::Image::from_texture(texture)
                .corner_radius(11.0 * scale)
                .paint_at(ui, avatar);
        } else {
            let painter = painter.with_clip_rect(avatar);
            let color = Color32::from_rgb(117, 117, 122);
            painter.circle_filled(
                avatar.center() - egui::vec2(0.0, 3.0 * scale),
                4.0 * scale,
                color,
            );
            painter.circle_filled(
                avatar.center() + egui::vec2(0.0, 8.0 * scale),
                8.0 * scale,
                color,
            );
        }
        let name_end = avatar.right() + 8.0 * scale + text_width(ui, name, 13.0 * scale);
        painter.text(
            egui::pos2(avatar.right() + 8.0 * scale, avatar.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(13.0 * scale),
            muted(),
        );
        let verified = user
            .get("is_verified")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if verified {
            let center = egui::pos2(name_end + 12.125 * scale, avatar.center().y);
            super::brand::draw_verified_badge(
                &painter,
                Rect::from_center_size(center, egui::vec2(12.25, 14.0) * scale),
            );
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as i64);
        let ago = tone.raw["published_at"]
            .as_str()
            .map(|published| time_ago_at(published, now))
            .unwrap_or_default();
        if !ago.is_empty() {
            let ago_x = if verified {
                name_end + 24.25 * scale
            } else {
                name_end + text_width(ui, " · ", 13.0 * scale)
            };
            painter.text(
                egui::pos2(ago_x, avatar.center().y),
                egui::Align2::LEFT_CENTER,
                ago,
                egui::FontId::proportional(13.0 * scale),
                muted(),
            );
        }
    }
    if disabled && !picked {
        painter.rect_filled(rect, 12.0 * scale, palette::DISABLED_VEIL);
    }
    if picked && busy_overlay {
        if let Some(backdrop) = busy_backdrop {
            egui::Image::from_texture(backdrop)
                .uv(busy_backdrop_uv(ui, rect))
                .corner_radius(12.0 * scale)
                .paint_at(ui, rect);
        }
        painter.rect_filled(rect, 12.0 * scale, palette::BACKDROP_DIM);
        for (index, alpha) in [110, 165, 230].into_iter().enumerate() {
            painter.circle_filled(
                rect.center() + egui::vec2((index as f32 - 1.0) * 12.0 * scale, 0.0),
                4.0 * scale,
                Color32::from_rgba_unmultiplied(228, 228, 231, alpha),
            );
        }
    }
    if heart_clicked {
        return Some(ToneCardAction::Favorite(tone.id));
    }
    (!disabled && response.clicked()).then_some(ToneCardAction::Open)
}

/// The accessible name of the heart of one tone. A title can come from a server.
fn heart_label(title: &str, favorite: bool) -> String {
    let title = super::banners::one_line_text(title);
    match (favorite, title.is_empty()) {
        (true, false) => format!("Remove {title} from favorites"),
        (false, false) => format!("Add {title} to favorites"),
        (true, true) => "Remove from favorites".to_owned(),
        (false, true) => "Add to favorites".to_owned(),
    }
}

/// Draws the favorite control over the card, and returns whether it took a click.
///
/// The control registers after the card, so that egui gives it a click on the shared area.
fn favorite_heart(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    tone: &Tone,
    center: egui::Pos2,
    favorite: bool,
    scale: f32,
    enabled: bool,
) -> bool {
    let response = ui.interact(
        Rect::from_center_size(center, egui::Vec2::splat(HEART_HIT_SIDE * scale)),
        ui.scope_id().with("tone-favorite").with(tone.id.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            enabled,
            heart_label(&tone.title, favorite),
        )
    });
    let lit = enabled && (response.hovered() || response.has_focus());
    let outline = heart_outline(center, HEART_WIDTH * scale);
    let color = if favorite {
        palette::FLAME
    } else if lit {
        palette::BLAZE
    } else {
        palette::CHROME
    };
    if favorite {
        // A heart is not convex, but each ray from this center meets its outline one time.
        let mut fill = egui::epaint::Mesh::default();
        fill.colored_vertex(center, color);
        for point in &outline {
            fill.colored_vertex(*point, color);
        }
        let last = outline.len() as u32;
        for index in 1..=last {
            fill.add_triangle(0, index, index % last + 1);
        }
        painter.add(egui::Shape::mesh(fill));
    }
    painter.add(egui::Shape::closed_line(
        outline,
        egui::Stroke::new(HEART_STROKE * scale, color),
    ));
    enabled && response.clicked()
}

/// The outline of a heart of this width, clockwise from its notch, around `center`.
fn heart_outline(center: egui::Pos2, width: f32) -> Vec<egui::Pos2> {
    // The right half in a box 20 units across. Its height runs from -8.5 to 8.5.
    const RIGHT_HALF: [[[f32; 2]; 4]; 3] = [
        [[0.0, -6.1], [1.1, -7.5], [2.8, -8.5], [4.6, -8.5]],
        [[4.6, -8.5], [7.7, -8.5], [10.0, -6.1], [10.0, -3.1]],
        [[10.0, -3.1], [10.0, 0.9], [6.2, 4.4], [0.0, 8.5]],
    ];
    let unit = width / 20.0;
    let mut right = vec![center + egui::vec2(RIGHT_HALF[0][0][0], RIGHT_HALF[0][0][1]) * unit];
    for [start, control_a, control_b, end] in RIGHT_HALF {
        for step in 1..=HEART_CURVE_STEPS {
            let t = step as f32 / HEART_CURVE_STEPS as f32;
            let rest = 1.0 - t;
            let along = |axis: usize| {
                rest * rest * rest * start[axis]
                    + 3.0 * rest * rest * t * control_a[axis]
                    + 3.0 * rest * t * t * control_b[axis]
                    + t * t * t * end[axis]
            };
            right.push(center + egui::vec2(along(0), along(1)) * unit);
        }
    }
    // The notch and the tip are on the middle line, so the left half does not repeat them.
    let left = right[1..right.len() - 1]
        .iter()
        .rev()
        .map(|point| egui::pos2(2.0 * center.x - point.x, point.y))
        .collect::<Vec<_>>();
    right.extend(left);
    right
}

fn busy_backdrop_uv(ui: &egui::Ui, rect: Rect) -> Rect {
    let screen = ui.ctx().viewport_rect();
    let global_rect = ui
        .ctx()
        .layer_transform_to_global(ui.layer_id())
        .map_or(rect, |transform| transform * rect);
    Rect::from_min_max(
        egui::pos2(
            (global_rect.left() - screen.left()) / screen.width(),
            (global_rect.top() - screen.top()) / screen.height(),
        ),
        egui::pos2(
            (global_rect.right() - screen.left()) / screen.width(),
            (global_rect.bottom() - screen.top()) / screen.height(),
        ),
    )
}

pub(super) fn content_height_for(ui: &egui::Ui, tone: &Tone, card_width: f32, scale: f32) -> f32 {
    let available_width = (card_width - 152.0 * scale).max(20.0);
    let title_height = title_height(ui, tone, available_width, scale);
    let body_line = crate::fonts::normal_line_height(13.0) as f32 * scale;
    let gear_height = body_line.max(18.0 * scale);
    let stats_height = body_line.max(14.0 * scale);
    let creator_height = if tone.raw.get("user").is_some() {
        30.0 * scale
    } else {
        0.0
    };
    let column_height = title_height + 16.0 * scale + gear_height + stats_height + creator_height;
    24.0 * scale + (112.0 * scale).max(column_height)
}

fn title_height(ui: &egui::Ui, tone: &Tone, available_width: f32, scale: f32) -> f32 {
    if text_width(ui, &tone.title, 14.0 * scale) > available_width {
        title_galley(ui, tone, available_width, scale).rows.len() as f32 * 18.2 * scale
    } else {
        18.2 * scale
    }
}

fn title_galley(
    ui: &egui::Ui,
    tone: &Tone,
    available_width: f32,
    scale: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(
        tone.title.clone(),
        egui::FontId::new(14.0 * scale, egui::FontFamily::Name("arimo-bold".into())),
        Color32::WHITE,
        available_width,
    );
    job.wrap.max_rows = 2;
    job.sections[0].format.line_height = Some(18.2 * scale);
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

pub(crate) fn model_count(tone: &Tone) -> u64 {
    let field = if tone.kind == ronie_core::ModelKind::Nam {
        "a2_models_count"
    } else {
        "models_count"
    };
    tone.raw
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(tone.models.len() as u64)
}

fn time_ago_at(published: &str, now: i64) -> String {
    let Some(published) = parse_iso8601(published) else {
        return String::new();
    };
    let seconds = (now - published) / 1_000;
    const MINUTE: i64 = 60;
    const HOUR: i64 = MINUTE * 60;
    const DAY: i64 = HOUR * 24;
    const WEEK: i64 = DAY * 7;
    const MONTH: i64 = WEEK * 4;
    const YEAR: i64 = MONTH * 12;
    if seconds < 10 {
        "now".to_owned()
    } else if seconds < MINUTE {
        format!("{seconds}s")
    } else if seconds < HOUR {
        format!("{}m", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}h", seconds / HOUR)
    } else if seconds < WEEK {
        format!("{}d", seconds / DAY)
    } else if seconds < MONTH {
        format!("{}w", seconds / WEEK)
    } else if seconds < YEAR {
        format!("{}mo", seconds / MONTH)
    } else {
        format!("{}y", seconds / YEAR)
    }
}

fn parse_iso8601(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b' '))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }
    let number = |start, end| value.get(start..end)?.parse::<i64>().ok();
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let zone = bytes
        .iter()
        .enumerate()
        .skip(19)
        .find_map(|(index, byte)| matches!(byte, b'Z' | b'+' | b'-').then_some(index))?;
    let offset = match bytes.get(zone)? {
        b'Z' => 0,
        sign => {
            let hours = number(zone + 1, zone + 3)?;
            let minutes = number(zone + 4, zone + 6)?;
            if hours > 23 || minutes > 59 || bytes.get(zone + 3) != Some(&b':') {
                return None;
            }
            let offset = (hours * 60 + minutes) * 60;
            if *sign == b'+' { offset } else { -offset }
        }
    };
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1_000 - offset * 1_000)
}

fn gear_label(tone: &Tone) -> &'static str {
    match tone
        .raw
        .get("gear")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
    {
        "amp" => "Amp Head",
        "amp+cab" => "Amp + Cab",
        "cab" => "Cabinet",
        "pedal" => "Pedal",
        "outboard" => "Outboard",
        "space" => "Spaces",
        _ => "Experimental",
    }
}

fn muted() -> Color32 {
    palette::TEXT_SECONDARY
}

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                Color32::WHITE,
            )
            .size()
            .x
    })
}

pub(crate) fn compact_count(value: u64) -> String {
    if value < 10_000 {
        let digits = value.to_string();
        let mut output = String::new();
        for (index, character) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                output.push(',');
            }
            output.push(character);
        }
        return output;
    }
    if value >= 1_000_000 {
        return format!("{:.1}M", value as f64 / 1_000_000.0);
    }
    let scaled = value as f64 / 1000.0;
    if scaled >= 10.0 {
        return format!("{scaled:.0}K");
    }
    format!("{scaled:.1}K")
}

fn stat_icon(painter: &egui::Painter, center: egui::Pos2, index: usize, scale: f32) {
    let stroke = egui::Stroke::new(7.0 / 6.0 * scale, muted());
    let point = |x: f32, y: f32| center + egui::vec2(x * scale, y * scale);
    if index == 0 {
        painter.line_segment([point(0.0, -5.25), point(0.0, 1.75)], stroke);
        painter.add(egui::Shape::line(
            vec![
                point(5.25, 1.75),
                point(5.25, 3.5),
                point(4.6667, 4.6667),
                point(3.5, 5.25),
                point(-3.5, 5.25),
                point(-4.6667, 4.6667),
                point(-5.25, 3.5),
                point(-5.25, 1.75),
            ],
            stroke,
        ));
        painter.add(egui::Shape::line(
            vec![
                point(-35.0 / 12.0, -7.0 / 6.0),
                point(0.0, 1.75),
                point(35.0 / 12.0, -7.0 / 6.0),
            ],
            stroke,
        ));
    } else {
        painter.add(egui::Shape::line(
            vec![
                point(14.0 / 3.0, 14.0 / 3.0),
                point(35.0 / 6.0, 3.5),
                point(35.0 / 6.0, -7.0 / 3.0),
                point(14.0 / 3.0, -3.5),
                point(0.0583, -3.5),
                point(-0.9333, -3.9667),
                point(-1.4, -4.725),
                point(-2.37, -5.25),
                point(-14.0 / 3.0, -5.25),
                point(-35.0 / 6.0, -4.0833),
                point(-35.0 / 6.0, 14.0 / 3.0),
                point(-14.0 / 3.0, 14.0 / 3.0),
            ],
            stroke,
        ));
        painter.line_segment(
            [
                point(-35.0 / 6.0, -7.0 / 6.0),
                point(35.0 / 6.0, -7.0 / 6.0),
            ],
            stroke,
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_heart_label_names_the_tone_and_needs_no_title() {
        assert_eq!(
            super::heart_label("Green Muff", false),
            "Add Green Muff to favorites"
        );
        assert_eq!(
            super::heart_label("Green Muff", true),
            "Remove Green Muff from favorites"
        );
        assert_eq!(
            super::heart_label("  Bad\u{202E}\n\tTitle\u{FEFF} ", true),
            "Remove Bad Title from favorites"
        );
        assert_eq!(super::heart_label("\u{200B} \n", false), "Add to favorites");
        assert_eq!(super::heart_label("", true), "Remove from favorites");
    }

    #[test]
    fn the_center_of_the_heart_sees_each_outline_point_in_angle_order() {
        let center = eframe::egui::Pos2::ZERO;
        let outline = super::heart_outline(center, 16.0);
        let angles = outline
            .iter()
            .map(|point| (point.x.atan2(-point.y) + std::f32::consts::TAU) % std::f32::consts::TAU)
            .collect::<Vec<_>>();
        assert!(
            angles.windows(2).all(|pair| pair[1] > pair[0]),
            "a fan from the center must not fold: {angles:?}"
        );
        let widest = outline
            .iter()
            .map(|point| point.x.abs())
            .fold(0.0, f32::max);
        assert!((widest - 8.0).abs() < 0.1, "{widest}");
    }

    #[test]
    fn relative_time_matches_the_reference_thresholds() {
        let now = super::parse_iso8601("2026-11-03T00:00:00Z").expect("valid timestamp");
        assert_eq!(super::time_ago_at("2026-06-01T12:00:00Z", now), "5mo");
        assert_eq!(super::time_ago_at("not-a-date", now), "");
    }

    #[test]
    fn model_count_uses_the_compatible_total_for_each_format() {
        let nam = ronie_core::Tone::from_value(serde_json::json!({
            "format": "nam",
            "models_count": 12,
            "a2_models_count": 6,
            "models": []
        }))
        .expect("NAM tone");
        let ir = ronie_core::Tone::from_value(serde_json::json!({
            "format": "ir",
            "models_count": 24,
            "a2_models_count": 0,
            "models": []
        }))
        .expect("IR tone");
        assert_eq!(super::model_count(&nam), 6);
        assert_eq!(super::model_count(&ir), 24);
    }

    #[test]
    fn wrapped_titles_use_the_css_line_height() {
        let context = eframe::egui::Context::default();
        crate::fonts::Fonts::install(&context);
        let input = eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            let tone = ronie_core::Tone::from_value(serde_json::json!({
                "title": "A wrapped card title",
                "format": "nam",
                "models": []
            }))
            .expect("tone");
            let galley = super::title_galley(ui, &tone, 100.0, 1.0);
            assert_eq!(galley.rows.len(), 2);
            assert!((super::title_height(ui, &tone, 100.0, 1.0) - 36.4).abs() < f32::EPSILON);
            for row in &galley.rows {
                let glyph = row.glyphs.first().expect("title glyph");
                assert!((glyph.line_height - 18.2).abs() < f32::EPSILON);
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn busy_backdrop_uv_maps_card_to_global_before_sampling() {
        for transform in [
            eframe::egui::emath::TSTransform::IDENTITY,
            eframe::egui::emath::TSTransform {
                translation: eframe::egui::vec2(12.0, 28.0),
                scaling: 0.625,
            },
        ] {
            let context = eframe::egui::Context::default();
            let layer = eframe::egui::LayerId::new(
                eframe::egui::Order::Background,
                eframe::egui::Id::unique("busy-backdrop-uv"),
            );
            context.set_transform_layer(layer, transform);
            let screen = eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(1600.0, 900.0),
            );
            let card = eframe::egui::Rect::from_min_max(
                eframe::egui::pos2(100.0, 200.0),
                eframe::egui::pos2(300.0, 400.0),
            );
            let actual_uv = std::cell::Cell::new(eframe::egui::Rect::NOTHING);
            let mut output = context.run_ui(
                eframe::egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    let child = ui.new_child(
                        eframe::egui::UiBuilder::new()
                            .layer_id(layer)
                            .max_rect(screen),
                    );
                    actual_uv.set(super::busy_backdrop_uv(&child, card));
                },
            );
            output.textures_delta.clear();
            let global_card = transform * card;
            let expected = [
                global_card.left() / screen.width(),
                global_card.top() / screen.height(),
                global_card.right() / screen.width(),
                global_card.bottom() / screen.height(),
            ];
            let actual = [
                actual_uv.get().left(),
                actual_uv.get().top(),
                actual_uv.get().right(),
                actual_uv.get().bottom(),
            ];
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!((actual - expected).abs() < 1.0e-6);
            }
        }
    }
}
