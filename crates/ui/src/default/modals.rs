//! Shared modal backdrop, accessibility, and stacking behavior.

use eframe::egui::{self, Color32, ColorImage};

use crate::accessibility::{AccessibleNode, AccessibleRole};

const REFRESH_MS: i64 = 125;

/// A surface that shows a blurred, clean image of the window behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackdropUser {
    /// A system dialog.
    Dialog,
    /// A tone card that waits for its load.
    BusyCard,
}

/// Schedules clean backdrop captures and rejects frames containing a dialog.
///
/// A capture stalls the window, because it reads the frame back from the GPU and blurs it. The
/// production shell therefore takes one capture when a surface opens, and no capture otherwise.
#[derive(Debug, Default)]
pub struct ModalCapture {
    last_request_ms: Option<i64>,
    in_flight: bool,
    served: Option<BackdropUser>,
}

impl ModalCapture {
    /// Starts a capture when no dialog is visible and the refresh interval elapsed.
    ///
    /// Only a script uses this, to get clean backdrops before it opens a surface.
    pub fn request_due(&mut self, now_ms: i64, modal_visible: bool) -> bool {
        if modal_visible || self.in_flight {
            return false;
        }
        let due = self
            .last_request_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= REFRESH_MS);
        if due {
            self.last_request_ms = Some(now_ms);
            self.in_flight = true;
        }
        due
    }

    /// Starts one capture when `user` is new. `None` means that no surface needs a backdrop.
    pub fn request_for(&mut self, user: Option<BackdropUser>, now_ms: i64) -> bool {
        if user != self.served {
            // The surface that waited for an image is gone, so its image has no use.
            if self.served.take().is_some() {
                self.in_flight = false;
            }
        }
        if user.is_none() || self.served.is_some() || self.in_flight {
            return false;
        }
        self.served = user;
        self.last_request_ms = Some(now_ms);
        self.in_flight = true;
        true
    }

    /// Completes a capture, accepting it only when the frame has no dialog.
    ///
    /// The capture of a surface that opened is always clean, also when it arrives after the
    /// surface stopped its wait.
    pub fn complete(&mut self, modal_visible: bool) -> bool {
        let was_in_flight = std::mem::take(&mut self.in_flight);
        was_in_flight && (!modal_visible || self.served.is_some())
    }

    /// Whether a capture has not arrived.
    #[must_use]
    pub const fn in_flight(&self) -> bool {
        self.in_flight
    }
}

/// Half-resolution, two-pixel blurred snapshot with a 50% black scrim.
#[derive(Debug, Default)]
pub struct ModalScrim {
    last_refresh_ms: Option<i64>,
}

impl ModalScrim {
    /// Returns whether a UI-thread snapshot is due and records its schedule.
    pub fn refresh_due(&mut self, now_ms: i64) -> bool {
        let due = self
            .last_refresh_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= REFRESH_MS);
        if due {
            self.last_refresh_ms = Some(now_ms);
        }
        due
    }

    /// Downscales and blurs a backdrop on the UI thread.
    #[must_use]
    pub fn blur_snapshot(image: &ColorImage) -> ColorImage {
        let width = (image.size[0] / 2).max(1);
        let height = (image.size[1] / 2).max(1);
        let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
        for pixel in &image.pixels {
            bytes.extend_from_slice(&pixel.to_array());
        }
        let Some(source) =
            image::RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, bytes)
        else {
            return ColorImage::new([1, 1], vec![crate::theme::palette::BACKDROP_DIM]);
        };
        let resized = image::imageops::resize(
            &source,
            width as u32,
            height as u32,
            image::imageops::FilterType::Triangle,
        );
        let mut blurred = triple_box_blur(&resized, 2);
        for pixel in blurred.pixels_mut() {
            pixel.0[3] = 255;
        }
        ColorImage::from_rgba_unmultiplied([width, height], &blurred.into_raw())
    }

    /// Accessible dialog metadata with a stable ID.
    #[must_use]
    pub fn accessible_node(title: &str) -> AccessibleNode {
        AccessibleNode::new(
            format!("modal-{title}"),
            AccessibleRole::Dialog,
            title.to_owned(),
        )
    }

    /// Paints a prepared backdrop and intercepts background pointer input.
    pub fn paint(&self, ui: &mut egui::Ui, texture: Option<&egui::TextureHandle>) {
        let rect = ui.max_rect();
        let response = ui.allocate_rect(rect, egui::Sense::CLICK);
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Modal backdrop"));
        if let Some(texture) = texture {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        }
        ui.painter()
            .rect_filled(rect, 0.0, crate::theme::palette::BACKDROP_DIM);
    }
}

fn triple_box_blur(source: &image::RgbaImage, radius: i32) -> image::RgbaImage {
    let width = source.width() as usize;
    let height = source.height() as usize;
    let mut a = source.clone().into_raw();
    let mut b = vec![0; a.len()];
    let sigma = radius as f32;
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let ideal_count =
        (12.0 * sigma * sigma - 3.0 * lower.pow(2) as f32 - 12.0 * lower as f32 - 9.0)
            / (-4.0 * lower as f32 - 4.0);
    let count = ideal_count.round() as i32;
    for pass in 0..3 {
        let radius = if pass < count { lower } else { upper } - 1;
        if radius <= 0 {
            continue;
        }
        for y in 0..height {
            box_line(
                &a[y * width * 4..],
                &mut b[y * width * 4..],
                width,
                1,
                radius,
            );
        }
        for x in 0..width {
            box_line(&b[x * 4..], &mut a[x * 4..], height, width, radius);
        }
    }
    image::RgbaImage::from_raw(source.width(), source.height(), a).unwrap_or_else(|| source.clone())
}

fn box_line(source: &[u8], destination: &mut [u8], count: usize, stride: usize, radius: i32) {
    let bytes = stride * 4;
    let scale = 1.0 / (2 * radius + 1) as f32;
    for channel in 0..4 {
        let source = &source[channel..];
        let destination = &mut destination[channel..];
        let first = i32::from(source[0]);
        let last = i32::from(source[(count - 1) * bytes]);
        let mut sum = (radius + 1) * first;
        for index in 0..radius {
            sum += i32::from(source[index.min(count as i32 - 1) as usize * bytes]);
        }
        for index in 0..count {
            let add = if index as i32 + radius < count as i32 {
                i32::from(source[(index as i32 + radius) as usize * bytes])
            } else {
                last
            };
            let drop = if index as i32 > radius {
                i32::from(source[(index as i32 - radius - 1) as usize * bytes])
            } else {
                first
            };
            sum += add - drop;
            destination[index * bytes] = (sum as f32 * scale).round() as u8;
        }
    }
}

/// System-modal kinds that share one overlay layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKind {
    /// Version update notice.
    Update,
    /// Offline or secure-connection recovery.
    Connection,
}

/// Z-order and background-input policy for simultaneous system modals.
#[derive(Debug, Default)]
pub struct ModalStack {
    order: Vec<ModalKind>,
}

impl ModalStack {
    /// Places update below connection, matching the Reference front-to-back order.
    pub fn restack(&mut self, update_visible: bool, connection_visible: bool) {
        self.order.clear();
        if update_visible {
            self.order.push(ModalKind::Update);
        }
        if connection_visible {
            self.order.push(ModalKind::Connection);
        }
    }

    /// Bottom-to-top modal order.
    #[must_use]
    pub fn z_order(&self) -> &[ModalKind] {
        &self.order
    }

    /// Whether keyboard, pointer, and scrolling input must stay in the modal layer.
    #[must_use]
    pub fn blocks_background_input(&self) -> bool {
        !self.order.is_empty()
    }
}
