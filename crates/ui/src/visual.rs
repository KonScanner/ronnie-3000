//! Deterministic visual comparison and worst-tile reporting.

use std::path::Path;

use eframe::egui::{Color32, ColorImage};

/// Maximum allowed global mismatch ratio for Default scenarios.
pub const GLOBAL_MISMATCH_MAX: f64 = 0.019;
/// Maximum allowed mismatch ratio in one 64 px tile.
pub const WORST_TILE_MAX: f64 = 0.24;
/// Maximum per-channel difference absorbed by comparison.
pub const CHANNEL_TOLERANCE: u8 = 24;
/// Tile edge length in pixels.
pub const TILE_SIZE: usize = 64;

/// Pixel and densest-tile comparison results.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualComparison {
    /// Number of pixels outside the channel tolerance.
    pub mismatched: usize,
    /// Mismatched pixels divided by all pixels.
    pub mismatch_ratio: f64,
    /// Mismatch share in the densest 64 px tile.
    pub worst_tile_ratio: f64,
    /// Top-left coordinate of the densest tile.
    pub worst_tile: [usize; 2],
}

/// Compares two equal-sized images and optionally writes a red-highlight diff.
///
/// # Errors
/// Returns dimension or diff-write errors.
pub fn compare_images(
    reference: &ColorImage,
    candidate: &ColorImage,
    diff_path: Option<&Path>,
) -> Result<VisualComparison, String> {
    if reference.size != candidate.size {
        return Err(format!(
            "size mismatch: reference {}x{}, candidate {}x{}",
            reference.size[0], reference.size[1], candidate.size[0], candidate.size[1]
        ));
    }
    let [width, height] = reference.size;
    if width == 0 || height == 0 {
        return Err("image has no pixels".to_owned());
    }
    let tiles_x = width.div_ceil(TILE_SIZE);
    let tiles_y = height.div_ceil(TILE_SIZE);
    let mut tile_hits = vec![0_usize; tiles_x * tiles_y];
    let mut diff = diff_path.map(|_| image::RgbaImage::new(width as u32, height as u32));
    let mut mismatched = 0;
    for (index, (left, right)) in reference.pixels.iter().zip(&candidate.pixels).enumerate() {
        let delta = |channel: usize| left.to_array()[channel].abs_diff(right.to_array()[channel]);
        let bad = (0..3).map(delta).max().unwrap_or(0) > CHANNEL_TOLERANCE;
        let x = index % width;
        let y = index / width;
        if bad {
            mismatched += 1;
            tile_hits[(y / TILE_SIZE) * tiles_x + x / TILE_SIZE] += 1;
        }
        if let Some(image) = &mut diff {
            let grey = (u16::from(left.r()) * 60 / 255 + 40) as u8;
            let pixel = if bad {
                image::Rgba([255, 45, 45, 255])
            } else {
                image::Rgba([grey, grey, grey, 255])
            };
            image.put_pixel(x as u32, y as u32, pixel);
        }
    }
    let mut worst_tile_ratio = 0.0;
    let mut worst_tile = [0, 0];
    for (index, hits) in tile_hits.into_iter().enumerate() {
        let tile_x = index % tiles_x;
        let tile_y = index / tiles_x;
        let tile_width = TILE_SIZE.min(width - tile_x * TILE_SIZE);
        let tile_height = TILE_SIZE.min(height - tile_y * TILE_SIZE);
        let ratio = hits as f64 / (tile_width * tile_height) as f64;
        if ratio > worst_tile_ratio {
            worst_tile_ratio = ratio;
            worst_tile = [tile_x * TILE_SIZE, tile_y * TILE_SIZE];
        }
    }
    if let (Some(path), Some(image)) = (diff_path, diff) {
        image.save(path).map_err(|error| error.to_string())?;
    }
    Ok(VisualComparison {
        mismatched,
        mismatch_ratio: mismatched as f64 / (width * height) as f64,
        worst_tile_ratio,
        worst_tile,
    })
}

/// Creates a solid image for deterministic visual tests.
#[must_use]
pub fn solid_image(size: [usize; 2], color: Color32) -> ColorImage {
    ColorImage::new(size, vec![color; size[0] * size[1]])
}
