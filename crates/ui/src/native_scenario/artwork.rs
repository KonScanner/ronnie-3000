//! Local fixture artwork for deterministic captures. Nothing here reaches the network.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::egui;
use ronie_core::{Page, Tone};

use crate::{
    backend::UiBackend,
    default::{BlockView, ChromeController},
};

/// Fixture artwork that a capture puts on the tiles, the detail view, and the browser.
pub(super) struct FixtureArtwork {
    /// Directory that holds the fixture images.
    pub(super) root: PathBuf,
    /// Pixels per design point of the capture.
    pub(super) scale: f32,
    /// Backend that names the tones on the tiles.
    pub(super) backend: Arc<dyn UiBackend>,
    /// Catalog page whose cards take artwork.
    pub(super) catalog: Option<Page<Tone>>,
}

/// Loads the artwork of a controller that already shows its named state.
pub(super) fn install(
    artwork: &FixtureArtwork,
    context: &egui::Context,
    chrome: &mut ChromeController,
) {
    let root = artwork.root.as_path();
    let backend = &artwork.backend;
    let scale = artwork.scale;
    let state = backend.chain_state(0);
    let meters = backend.meter_levels();
    if chrome.session().signed_in() {
        let target = (24.0 * scale).round().max(1.0) as usize;
        if let Some(avatar) =
            load_fixture_texture(context, root, "avatar", [target, target], None, scale)
        {
            chrome.set_account_avatar(avatar.clone());
            if let Some(browser) = chrome.browser_mut() {
                browser.set_profile_avatar(avatar);
            }
        }
    }
    let tile = if state.chain.stereo_enabled {
        160.0
    } else {
        224.0
    };
    let tile_target = [
        (tile * scale).round().max(1.0) as usize,
        (tile * scale).round().max(1.0) as usize,
    ];
    let mut gallery_textures = std::collections::BTreeMap::new();
    for block in state.chain.left.iter().chain(&state.chain.right) {
        let Some(tone) = &block.tone else {
            continue;
        };
        let Some(url) = tone.raw["images"]
            .as_array()
            .and_then(|images| images.first())
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let meter = meters
            .blocks
            .iter()
            .find(|meter| meter.block_id == block.id);
        let texture = gallery_textures
            .entry(url.to_owned())
            .or_insert_with(|| load_fixture_texture(context, root, url, tile_target, meter, scale));
        if let Some(texture) = texture {
            chrome
                .gallery_mut()
                .set_artwork(block.id.clone(), texture.clone());
            if let Some(detail) = chrome.detail_mut()
                && detail.block().id == block.id
            {
                let detail_side = if detail.view() == BlockView::Info {
                    160.0
                } else {
                    192.0
                };
                let detail_target = [
                    (detail_side * scale).round().max(1.0) as usize,
                    (detail_side * scale).round().max(1.0) as usize,
                ];
                if let Some(detail_texture) =
                    load_fixture_texture(context, root, url, detail_target, None, scale)
                {
                    detail.set_artwork(detail_texture);
                }
                if let Some(avatar_url) = tone.raw["user"]["avatar_url"].as_str()
                    && let Some(avatar) =
                        load_fixture_texture(context, root, avatar_url, [64, 64], None, scale)
                {
                    detail.set_avatar(avatar);
                }
            }
        }
    }
    if let Some(page) = &artwork.catalog {
        let browser_scale = scale;
        let target = [
            (112.0 * browser_scale).ceil().max(1.0) as usize,
            (112.0 * browser_scale).ceil().max(1.0) as usize,
        ];
        let avatar_target = [
            (22.0 * browser_scale).ceil().max(1.0) as usize,
            (22.0 * browser_scale).ceil().max(1.0) as usize,
        ];
        let mut browser_textures = std::collections::BTreeMap::new();
        for tone in &page.data {
            let Some(url) = tone.raw["images"]
                .as_array()
                .and_then(|images| images.first())
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let texture = browser_textures
                .entry(url.to_owned())
                .or_insert_with(|| load_fixture_texture(context, root, url, target, None, scale));
            if let Some(browser) = chrome.browser_mut() {
                if let Some(texture) = texture {
                    browser.set_artwork(tone.id, texture.clone());
                }
                if let Some(avatar_url) = tone.raw["user"]["avatar_url"].as_str()
                    && let Some(avatar) =
                        load_fixture_texture(context, root, avatar_url, avatar_target, None, scale)
                {
                    browser.set_avatar(tone.id, avatar);
                }
            }
        }
    }
}

fn load_fixture_texture(
    context: &egui::Context,
    root: &Path,
    url: &str,
    target: [usize; 2],
    meter: Option<&crate::backend::BlockMeter>,
    scale: f32,
) -> Option<egui::TextureHandle> {
    let filename = url.rsplit('/').next()?;
    let mut path = root.join(filename);
    if path.extension().is_none() {
        path.set_extension("jpg");
    }
    let source = match image::open(path) {
        Ok(image) => image.to_rgba8(),
        Err(_) if filename.starts_with("avatar") => placeholder_avatar(filename),
        Err(_) => return None,
    };
    let cache_side = if filename.starts_with("avatar") {
        64
    } else {
        512
    };
    let cached = cover_artwork(&source, cache_side, cache_side);
    let mut image = cover_artwork(&cached, target[0] as u32, target[1] as u32);
    if let Some(meter) = meter {
        apply_energy_glow(&mut image, meter.output_db, scale, 16.0);
    }
    let size = [image.width() as usize, image.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &image.into_raw());
    Some(context.load_texture(
        format!("fixture-artwork-{url}-{}x{}", target[0], target[1]),
        image,
        egui::TextureOptions::LINEAR,
    ))
}

fn placeholder_avatar(key: &str) -> image::RgbaImage {
    let hash = key.bytes().fold(7_u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    let hue = (hash % 360) as f32 / 360.0;
    let hue2 = ((hash % 360 + 40) % 360) as f32 / 360.0;
    let from = hsl(hue, 0.45, 0.45);
    let to = hsl(hue2, 0.55, 0.30);
    let disc = hsl(hue2, 0.50, 0.35);
    let cx = 120.0 + (hash % 160) as f32;
    let cy = 90.0 + (hash % 120) as f32;
    let mut image = image::RgbaImage::new(400, 400);
    for y in 0..400_u32 {
        for x in 0..400_u32 {
            let fraction = (x + y) as f32 / 798.0;
            let mut color = [0.0_f32; 3];
            for channel in 0..3 {
                color[channel] = from[channel] + (to[channel] - from[channel]) * fraction;
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                if dx * dx + dy * dy < 90.0 * 90.0 {
                    color[channel] = color[channel] * 0.65 + disc[channel] * 0.35;
                }
            }
            image.put_pixel(
                x,
                y,
                image::Rgba([
                    color[0].round() as u8,
                    color[1].round() as u8,
                    color[2].round() as u8,
                    255,
                ]),
            );
        }
    }
    placeholder_label(&mut image, key);
    image
}

fn placeholder_label(image: &mut image::RgbaImage, key: &str) {
    const GLYPHS: &[(char, [u8; 7])] = &[
        ('A', [14, 17, 17, 31, 17, 17, 17]),
        ('B', [30, 17, 17, 30, 17, 17, 30]),
        ('C', [15, 16, 16, 16, 16, 16, 15]),
        ('D', [30, 17, 17, 17, 17, 17, 30]),
        ('E', [31, 16, 16, 30, 16, 16, 31]),
        ('F', [31, 16, 16, 30, 16, 16, 16]),
        ('G', [15, 16, 16, 23, 17, 17, 15]),
        ('H', [17, 17, 17, 31, 17, 17, 17]),
        ('I', [31, 4, 4, 4, 4, 4, 31]),
        ('J', [7, 2, 2, 2, 2, 18, 12]),
        ('K', [17, 18, 20, 24, 20, 18, 17]),
        ('L', [16, 16, 16, 16, 16, 16, 31]),
        ('M', [17, 27, 21, 21, 17, 17, 17]),
        ('N', [17, 25, 21, 19, 17, 17, 17]),
        ('O', [14, 17, 17, 17, 17, 17, 14]),
        ('P', [30, 17, 17, 30, 16, 16, 16]),
        ('Q', [14, 17, 17, 17, 21, 18, 13]),
        ('R', [30, 17, 17, 30, 20, 18, 17]),
        ('S', [15, 16, 16, 14, 1, 1, 30]),
        ('T', [31, 4, 4, 4, 4, 4, 4]),
        ('U', [17, 17, 17, 17, 17, 17, 14]),
        ('V', [17, 17, 17, 17, 17, 10, 4]),
        ('W', [17, 17, 17, 21, 21, 27, 17]),
        ('X', [17, 17, 10, 4, 10, 17, 17]),
        ('Y', [17, 17, 10, 4, 4, 4, 4]),
        ('Z', [31, 1, 2, 4, 8, 16, 31]),
    ];
    let label = key.replace(['-', '_'], "  ").to_uppercase();
    let width = label.chars().count() as i32 * 24 - 4;
    let origin_x = (400 - width) / 2;
    for (index, character) in label.chars().enumerate() {
        let glyph = GLYPHS
            .iter()
            .find_map(|(letter, rows)| (*letter == character).then_some(*rows));
        let Some(rows) = glyph else {
            continue;
        };
        for (row, bits) in rows.into_iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) == 0 {
                    continue;
                }
                for dy in 0..4 {
                    for dx in 0..4 {
                        let x = origin_x + index as i32 * 24 + column * 4 + dx;
                        let y = 184 + row as i32 * 4 + dy;
                        let pixel = image.get_pixel_mut(x as u32, y as u32);
                        for channel in &mut pixel.0[..3] {
                            *channel = (u16::from(*channel) * 18 / 100 + 209) as u8;
                        }
                    }
                }
            }
        }
    }
}

fn hsl(hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
    fn channel(p: f32, q: f32, mut hue: f32) -> f32 {
        if hue < 0.0 {
            hue += 1.0;
        }
        if hue > 1.0 {
            hue -= 1.0;
        }
        if hue < 1.0 / 6.0 {
            p + (q - p) * 6.0 * hue
        } else if hue < 0.5 {
            q
        } else if hue < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - hue) * 6.0
        } else {
            p
        }
    }
    if saturation == 0.0 {
        return [lightness * 255.0; 3];
    }
    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    [
        channel(p, q, hue + 1.0 / 3.0) * 255.0,
        channel(p, q, hue) * 255.0,
        channel(p, q, hue - 1.0 / 3.0) * 255.0,
    ]
}

fn cover_artwork(source: &image::RgbaImage, width: u32, height: u32) -> image::RgbaImage {
    crate::services::bitmap::cover_rgba(source, width, height)
}

fn apply_energy_glow(image: &mut image::RgbaImage, db: f32, scale: f32, corner: f32) {
    let unit = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let gate = ((db + 50.0) / 10.0).clamp(0.0, 1.0);
    let fire = crate::theme::palette::fire_ramp(unit);
    let level = [fire.r(), fire.g(), fire.b()];
    let mix = |dark: u8, bright: u8| {
        (f32::from(dark) + (f32::from(bright) - f32::from(dark)) * gate).round() as u8
    };
    let colour = [mix(34, level[0]), mix(34, level[1]), mix(34, level[2])];
    let presence = gate * unit;
    if presence <= 0.0 {
        return;
    }
    let blur = (2.0 + 16.0 * presence) * scale;
    let alpha = 0.75 * presence;
    let (width, height) = image.dimensions();
    let band = (blur.ceil() as u32).min(width / 2).min(height / 2);
    let radius = corner * scale;
    let corner_size = radius.ceil() as u32;
    for y in 0..height {
        for x in 0..width {
            let cx = x.min(width - 1 - x);
            let cy = y.min(height - 1 - y);
            let mut distance = cx.min(cy);
            if cx < corner_size && cy < corner_size {
                let dx = (radius - (cx as f32 + 0.5)).max(0.0);
                let dy = (radius - (cy as f32 + 0.5)).max(0.0);
                let corner_distance = (radius - (dx * dx + dy * dy).sqrt()).max(0.0);
                distance = distance.min(corner_distance as u32);
            }
            if distance >= band {
                continue;
            }
            let falloff = 1.0 - (distance as f32 + 0.5) / blur;
            if falloff <= 0.0 {
                continue;
            }
            let strength = alpha * 0.5 * falloff * falloff / 256.0;
            let pixel = image.get_pixel_mut(x, y);
            for channel in 0..3 {
                let base = f32::from(pixel[channel]);
                pixel[channel] =
                    (base + (255.0 - base) * f32::from(colour[channel]) * strength) as u8;
            }
        }
    }
}
