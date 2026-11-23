//! Reference-compatible cover cropping and fixed-point resampling.

use eframe::egui::ColorImage;

pub(crate) fn cover(source: &ColorImage, width: u32, height: u32) -> ColorImage {
    let mut bytes = Vec::with_capacity(source.pixels.len() * 4);
    for pixel in &source.pixels {
        bytes.extend_from_slice(&pixel.to_array());
    }
    let Some(source) =
        image::RgbaImage::from_raw(source.width() as u32, source.height() as u32, bytes)
    else {
        return ColorImage::default();
    };
    let covered = cover_rgba(&source, width, height);
    ColorImage::from_rgba_unmultiplied(
        [covered.width() as usize, covered.height() as usize],
        &covered.into_raw(),
    )
}

pub(crate) fn cover_rgba(source: &image::RgbaImage, width: u32, height: u32) -> image::RgbaImage {
    if source.width() == 0 || source.height() == 0 || width == 0 || height == 0 {
        return image::RgbaImage::new(width, height);
    }
    let source_width = source.width() as f32;
    let source_height = source.height() as f32;
    let ratio = (source_width / width as f32).min(source_height / height as f32);
    let crop_width = ((width as f32 * ratio).round() as u32).clamp(1, source.width());
    let crop_height = ((height as f32 * ratio).round() as u32).clamp(1, source.height());
    if crop_width == source.width() && crop_height == source.height() {
        return resample(source, width, height);
    }
    let image = image::imageops::crop_imm(
        source,
        (source.width() - crop_width) / 2,
        (source.height() - crop_height) / 2,
        crop_width,
        crop_height,
    )
    .to_image();
    resample(&image, width, height)
}

fn resample(source: &image::RgbaImage, width: u32, height: u32) -> image::RgbaImage {
    if source.width() < 2 * width || source.height() < 2 * height {
        return resize(source, width, height);
    }
    let mut image = resize(
        source,
        (source.width() / 2).max(1),
        (source.height() / 2).max(1),
    );
    while image.width() >= 2 * width && image.height() >= 2 * height {
        image = resize(
            &image,
            (image.width() / 2).max(1),
            (image.height() / 2).max(1),
        );
    }
    if image.width() == width && image.height() == height {
        image
    } else {
        resize(&image, width, height)
    }
}

fn resize(source: &image::RgbaImage, width: u32, height: u32) -> image::RgbaImage {
    if source.width() == width && source.height() == height {
        return source.clone();
    }
    if source.width() == 0 || source.height() == 0 || width == 0 || height == 0 {
        return image::RgbaImage::new(width, height);
    }
    if source.width() == width.saturating_mul(2) && source.height() == height.saturating_mul(2) {
        let source_pixels = source.as_raw();
        let source_stride = source.width() as usize * 4;
        let output_stride = width as usize * 4;
        let mut output = image::RgbaImage::new(width, height);
        let output_pixels = output.as_mut();
        for (source_rows, output_row) in source_pixels
            .chunks_exact(source_stride * 2)
            .zip(output_pixels.chunks_exact_mut(output_stride))
        {
            let (top, bottom) = source_rows.split_at(source_stride);
            for ((top, bottom), output) in top
                .as_chunks::<8>()
                .0
                .iter()
                .zip(bottom.as_chunks::<8>().0.iter())
                .zip(output_row.as_chunks_mut::<4>().0.iter_mut())
            {
                for channel in 0..4 {
                    let sum = u32::from(top[channel])
                        + u32::from(top[4 + channel])
                        + u32::from(bottom[channel])
                        + u32::from(bottom[4 + channel]);
                    output[channel] = ((sum + 2) / 4) as u8;
                }
            }
        }
        return output;
    }
    let x_map = axis_map(source.width(), width);
    let y_map = axis_map(source.height(), height);
    let source_pixels = source.as_raw();
    let source_stride = source.width() as usize * 4;
    let mut output = image::RgbaImage::new(width, height);
    let output_pixels = output.as_mut();
    for (y, (y0, y1, sub_y)) in y_map.into_iter().enumerate() {
        let top = y0 as usize * source_stride;
        let bottom = y1 as usize * source_stride;
        for (x, (x0, x1, sub_x)) in x_map.iter().copied().enumerate() {
            let left = x0 as usize * 4;
            let right = x1 as usize * 4;
            let weights = [
                (256 - sub_x) * (256 - sub_y),
                sub_x * (256 - sub_y),
                sub_x * sub_y,
                (256 - sub_x) * sub_y,
            ];
            let indices = [top + left, top + right, bottom + right, bottom + left];
            for channel in 0..4 {
                let mut value = 32_768_u64;
                for (weight, index) in weights.iter().zip(indices) {
                    value += weight * u64::from(source_pixels[index + channel]);
                }
                output_pixels[(y * width as usize + x) * 4 + channel] = (value >> 16) as u8;
            }
        }
    }
    output
}

fn axis_map(source: u32, target: u32) -> Vec<(u32, u32, u64)> {
    debug_assert!(source > 0 && target > 0);
    let inverse = 1.0 / (target as f32 / source as f32);
    let first = (0.5 * inverse * 256.0) as i32;
    let last = ((target as f32 + 0.5) * inverse * 256.0) as i32;
    let steps = target as i32;
    let delta = last - first;
    let mut step = delta / steps;
    let mut remainder = delta % steps;
    let mut modulo = remainder;
    let mut coordinate = first - 128;
    if modulo <= 0 {
        modulo += steps;
        remainder += steps;
        step -= 1;
    }
    modulo -= steps;
    (0..target)
        .map(|_| {
            let start = coordinate >> 8;
            let first = start.clamp(0, source as i32 - 1) as u32;
            let second = (first + 1).min(source - 1);
            let mapped = (first, second, (coordinate & 255) as u64);
            modulo += remainder;
            coordinate += step;
            if modulo > 0 {
                modulo -= steps;
                coordinate += 1;
            }
            mapped
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::axis_map;

    #[test]
    fn fractional_reduction_uses_reference_coordinate_sequence() {
        let actual: Vec<_> = axis_map(512, 173)
            .into_iter()
            .take(12)
            .map(|(first, _, subpixel)| (first, subpixel))
            .collect();
        assert_eq!(
            actual,
            [
                (0, 250),
                (3, 240),
                (6, 229),
                (9, 219),
                (12, 209),
                (15, 198),
                (18, 188),
                (21, 178),
                (24, 167),
                (27, 157),
                (30, 147),
                (33, 136),
            ]
        );
    }
}
