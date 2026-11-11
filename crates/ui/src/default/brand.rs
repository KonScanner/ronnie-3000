//! Ronnie logo and the small brand marks that use its colours.

use eframe::egui::{self, Color32, Rect};

use crate::theme::palette;

pub(super) fn draw_launcher_icon(ui: &egui::Ui, rect: Rect) {
    static IMAGE: std::sync::OnceLock<egui::ColorImage> = std::sync::OnceLock::new();
    let image = IMAGE.get_or_init(|| {
        let image = image::load_from_memory(include_bytes!("../../../../assets/ronnie-header.png"))
            .expect("embedded launcher icon")
            .to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        egui::ColorImage::from_rgba_unmultiplied(size, &image.into_raw())
    });
    let id = egui::Id::unique("ronnie-launcher-icon");
    let texture = ui
        .ctx()
        .data_mut(|data| data.get_temp::<egui::TextureHandle>(id));
    let texture = texture.unwrap_or_else(|| {
        let texture = ui.ctx().load_texture(
            "ronnie-launcher-icon",
            image.clone(),
            // The 512 px art draws far smaller, so it needs mipmaps to stay free of aliasing.
            egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)),
        );
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    });
    // `paint_at` fills the rectangle exactly, so the aspect ratio must be fitted here.
    egui::Image::from_texture(&texture).paint_at(ui, fit_centered(rect, texture.size_vec2()));
}

/// Largest centered rectangle inside `bounds` with the aspect ratio of `image`.
fn fit_centered(bounds: Rect, image: egui::Vec2) -> Rect {
    let scale = (bounds.width() / image.x).min(bounds.height() / image.y);
    Rect::from_center_size(bounds.center(), image * scale)
}

/// Plain text chip for the A2 model architecture, in the logo's chrome colour.
pub(super) fn draw_architecture_chip(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, rect.height() / 6.0, palette::CHROME);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "A2",
        egui::FontId::monospace(rect.height() * 0.62),
        Color32::BLACK,
    );
}

pub(super) fn draw_verified_badge(painter: &egui::Painter, rect: Rect) {
    let point =
        |x: f32, y: f32| rect.min + egui::vec2(x * rect.width() / 17.5, y * rect.height() / 20.0);
    for (points, color) in [
        (
            [
                (17.5, 5.88),
                (5.47, 20.0),
                (0.0, 12.96),
                (2.83, 11.81),
                (5.58, 15.34),
                (11.6, 8.27),
            ],
            palette::EMBER,
        ),
        (
            [
                (17.5, 0.0),
                (5.47, 14.12),
                (0.0, 7.08),
                (2.83, 5.93),
                (5.58, 9.46),
                (11.6, 2.38),
            ],
            palette::FIRE,
        ),
        (
            [
                (17.5, 2.94),
                (5.47, 17.06),
                (0.0, 10.02),
                (2.83, 8.87),
                (5.58, 12.4),
                (11.6, 5.32),
            ],
            palette::BLAZE,
        ),
    ] {
        painter.add(egui::Shape::convex_polygon(
            points.into_iter().map(|(x, y)| point(x, y)).collect(),
            color,
            egui::Stroke::NONE,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_art_keeps_its_aspect_in_a_wide_rectangle() {
        let bounds = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(48.0, 24.0));
        let fitted = fit_centered(bounds, egui::vec2(512.0, 512.0));
        assert_eq!(fitted.size(), egui::vec2(24.0, 24.0));
        assert_eq!(fitted.center(), bounds.center());
    }
}
