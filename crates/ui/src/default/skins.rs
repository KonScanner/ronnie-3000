//! Rendered gear skins: the embedded renders from `tools/render`, decoded once per context, and
//! drawn with knobs that turn.
//!
//! Each skin has a transparent body, a filmstrip of 32 frames for each live knob, and a manifest
//! that says where each knob (and, on a pedal, the blank LCD glass) sits on the body, in body
//! pixels.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions};
use ronnie_core::{ChainBlock, gear::block_identity};

/// The block setting a live knob turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KnobControl {
    /// "GAIN IN": the block's input gain.
    Gain,
    /// "OUTPUT": the block's output gain.
    Output,
    /// "MIX": the block's dry/wet mix.
    Mix,
}

impl KnobControl {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "gain" => Some(Self::Gain),
            "output" => Some(Self::Output),
            "mix" => Some(Self::Mix),
            _ => None,
        }
    }
}

/// One live knob on a skin.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinKnob {
    /// The setting it turns.
    pub control: KnobControl,
    /// The text printed on the render, such as "GAIN IN".
    pub label: String,
    /// Centre of the knob's square crop, in body pixels.
    pub center: [f32; 2],
    /// Side of the square crop, in body pixels.
    pub side: f32,
    /// Number of frames, from position 0 to position 1.
    pub frames: u32,
    /// Frames per row in the strip.
    pub columns: u32,
}

impl SkinKnob {
    /// The UV rectangle of the strip frame nearest `position`, a knob position from 0 to 1.
    #[must_use]
    pub fn frame_uv(&self, position: f32) -> Rect {
        let last = self.frames.saturating_sub(1);
        let index = (position.clamp(0.0, 1.0) * last as f32).round() as u32;
        let columns = self.columns.max(1);
        let rows = self.frames.div_ceil(columns).max(1);
        let (col, row) = (index % columns, index / columns);
        let size = egui::vec2(1.0 / columns as f32, 1.0 / rows as f32);
        let min = Pos2::new(col as f32 * size.x, row as f32 * size.y);
        Rect::from_min_size(min, size)
    }

    /// Where the knob's crop sits when the body is drawn in `body`.
    #[must_use]
    pub fn rect_in(&self, body: Rect, manifest: &SkinManifest) -> Rect {
        let scale = body.width() / manifest.width as f32;
        Rect::from_center_size(
            body.min + egui::vec2(self.center[0], self.center[1]) * scale,
            egui::Vec2::splat(self.side * scale),
        )
    }
}

/// The blank LCD glass on a pedal skin, where the app draws the active capture's name.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinLcd {
    /// Centre of the glass, in body pixels.
    pub center: [f32; 2],
    /// Width and height of the glass's axis-aligned bounds, in body pixels.
    pub size: [f32; 2],
}

impl SkinLcd {
    /// Where the glass sits when the body is drawn in `body`.
    #[must_use]
    pub fn rect_in(&self, body: Rect, manifest: &SkinManifest) -> Rect {
        let scale = body.width() / manifest.width as f32;
        Rect::from_center_size(
            body.min + egui::vec2(self.center[0], self.center[1]) * scale,
            egui::vec2(self.size[0], self.size[1]) * scale,
        )
    }
}

/// The blank name plate on a default amp or cab skin, where the app prints the active capture's
/// name in the plate's ink.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinPlate {
    /// Centre of the plate, in body pixels.
    pub center: [f32; 2],
    /// Width and height of the plate, in body pixels.
    pub size: [f32; 2],
    /// Colour of the printed name.
    pub ink: Color32,
}

impl SkinPlate {
    /// Where the plate sits when the body is drawn in `body`.
    #[must_use]
    pub fn rect_in(&self, body: Rect, manifest: &SkinManifest) -> Rect {
        let scale = body.width() / manifest.width as f32;
        Rect::from_center_size(
            body.min + egui::vec2(self.center[0], self.center[1]) * scale,
            egui::vec2(self.size[0], self.size[1]) * scale,
        )
    }
}

/// Size of a skin's body, its live knobs and its LCD.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinManifest {
    /// Skin identity, as from [`ronnie_core::gear::GearFamily::skin_id`].
    pub id: String,
    /// Body width in pixels.
    pub width: u32,
    /// Body height in pixels.
    pub height: u32,
    /// Live knobs.
    pub knobs: Vec<SkinKnob>,
    /// The LCD glass, on pedals.
    pub lcd: Option<SkinLcd>,
    /// The name plate, on the default amps and cab.
    pub plate: Option<SkinPlate>,
}

impl SkinManifest {
    fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let number = |value: &serde_json::Value| value.as_f64().map(|n| n as f32);
        let knobs = value
            .get("knobs")?
            .as_array()?
            .iter()
            .map(|knob| {
                let center = knob.get("center")?.as_array()?;
                Some(SkinKnob {
                    control: KnobControl::parse(knob.get("control")?.as_str()?)?,
                    label: knob.get("label")?.as_str()?.to_owned(),
                    center: [number(center.first()?)?, number(center.get(1)?)?],
                    side: number(knob.get("side")?)?,
                    frames: u32::try_from(knob.get("frames")?.as_u64()?).ok()?,
                    columns: u32::try_from(knob.get("columns")?.as_u64()?).ok()?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        // A missing or malformed LCD leaves the skin without one; the body and knobs still draw.
        let pair = |object: &serde_json::Value, key: &str| -> Option<[f32; 2]> {
            let pair = object.get(key)?.as_array()?;
            Some([number(pair.first()?)?, number(pair.get(1)?)?])
        };
        let lcd = value.get("lcd").and_then(|lcd| {
            Some(SkinLcd {
                center: pair(lcd, "center")?,
                size: pair(lcd, "size")?,
            })
        });
        // The same holds for the name plate: a malformed one leaves the skin without a name.
        let plate = value.get("plate").and_then(|plate| {
            let ink = plate
                .get("ink")?
                .as_array()?
                .iter()
                .map(|channel| u8::try_from(channel.as_u64()?).ok())
                .collect::<Option<Vec<_>>>()?;
            let [red, green, blue] = ink[..] else {
                return None;
            };
            Some(SkinPlate {
                center: pair(plate, "center")?,
                size: pair(plate, "size")?,
                ink: Color32::from_rgb(red, green, blue),
            })
        });
        Some(Self {
            id: value.get("id")?.as_str()?.to_owned(),
            width: u32::try_from(value.get("width")?.as_u64()?).ok()?,
            height: u32::try_from(value.get("height")?.as_u64()?).ok()?,
            knobs,
            lcd,
            plate,
        })
    }

    /// The largest rectangle with the body's aspect inside `area`, no taller than `max_height`,
    /// centred in `area`.
    #[must_use]
    pub fn fit(&self, area: Rect, max_height: f32) -> Rect {
        let aspect = self.width as f32 / self.height.max(1) as f32;
        let mut height = area.height().min(max_height);
        let mut width = height * aspect;
        if width > area.width() {
            width = area.width();
            height = width / aspect;
        }
        Rect::from_center_size(area.center(), egui::vec2(width, height))
    }
}

struct SkinAsset {
    id: &'static str,
    manifest: &'static str,
    body: &'static [u8],
    gain: Option<&'static [u8]>,
    output: Option<&'static [u8]>,
    mix: Option<&'static [u8]>,
}

macro_rules! skin_asset {
    ($id:literal) => {
        SkinAsset {
            id: $id,
            manifest: include_str!(concat!("../../assets/skins/", $id, "/skin.json")),
            body: include_bytes!(concat!("../../assets/skins/", $id, "/body.webp")),
            gain: None,
            output: None,
            mix: None,
        }
    };
    ($id:literal, knobs) => {
        SkinAsset {
            gain: Some(include_bytes!(concat!(
                "../../assets/skins/",
                $id,
                "/knob-gain.webp"
            ))),
            output: Some(include_bytes!(concat!(
                "../../assets/skins/",
                $id,
                "/knob-output.webp"
            ))),
            mix: Some(include_bytes!(concat!(
                "../../assets/skins/",
                $id,
                "/knob-mix.webp"
            ))),
            ..skin_asset!($id)
        }
    };
}

const SKINS: &[SkinAsset] = &[
    skin_asset!("od-green", knobs),
    skin_asset!("od-gold", knobs),
    skin_asset!("dist-black", knobs),
    skin_asset!("fuzz", knobs),
    skin_asset!("boost", knobs),
    skin_asset!("comp", knobs),
    skin_asset!("default-dark", knobs),
    skin_asset!("default-light", knobs),
    skin_asset!("highgain", knobs),
    skin_asset!("modern-us", knobs),
    skin_asset!("british", knobs),
    skin_asset!("chime", knobs),
    skin_asset!("clean", knobs),
    skin_asset!("bass", knobs),
    skin_asset!("default-clean", knobs),
    skin_asset!("default-crunch", knobs),
    skin_asset!("default-hi-gain", knobs),
    skin_asset!("cab-412-modern"),
    skin_asset!("cab-412-vintage"),
    skin_asset!("cab-212-chime"),
    skin_asset!("cab-112-clean"),
    skin_asset!("cab-810-bass"),
    skin_asset!("cab-default"),
];

fn asset(id: &str) -> Option<&'static SkinAsset> {
    SKINS.iter().find(|asset| asset.id == id)
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
        .ok()?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

/// The manifest of a skin, if the app has its render.
#[must_use]
pub fn manifest(id: &str) -> Option<SkinManifest> {
    SkinManifest::parse(asset(id)?.manifest)
}

/// The decoded body of a skin.
#[must_use]
pub fn body_image(id: &str) -> Option<ColorImage> {
    decode(asset(id)?.body)
}

/// The decoded filmstrip of one live knob of a skin.
#[must_use]
pub fn knob_strip(id: &str, control: KnobControl) -> Option<ColorImage> {
    let asset = asset(id)?;
    decode(match control {
        KnobControl::Gain => asset.gain?,
        KnobControl::Output => asset.output?,
        KnobControl::Mix => asset.mix?,
    })
}

/// Each skin with a render: its identity, the name the skin picker shows, and the kind of gear
/// it draws. The order is the order of the picker.
const SKIN_NAMES: &[(&str, &str, ronnie_core::gear::GearCategory)] = {
    use ronnie_core::gear::GearCategory::{Amp, Cab, Pedal};
    &[
        ("od-green", "Green overdrive", Pedal),
        ("od-gold", "Gold overdrive", Pedal),
        ("dist-black", "Black distortion", Pedal),
        ("fuzz", "Fuzz", Pedal),
        ("boost", "Boost", Pedal),
        ("comp", "Compressor", Pedal),
        ("default-dark", "Dark pedal", Pedal),
        ("default-light", "Light pedal", Pedal),
        ("highgain", "High-gain head", Amp),
        ("modern-us", "US modern head", Amp),
        ("british", "British head", Amp),
        ("chime", "Chime combo", Amp),
        ("clean", "Clean combo", Amp),
        ("bass", "Bass head", Amp),
        ("default-clean", "Clean amp", Amp),
        ("default-crunch", "Crunch amp", Amp),
        ("default-hi-gain", "Hi-gain amp", Amp),
        ("cab-412-modern", "Modern 4x12", Cab),
        ("cab-412-vintage", "Vintage 4x12", Cab),
        ("cab-212-chime", "2x12 cab", Cab),
        ("cab-112-clean", "1x12 cab", Cab),
        ("cab-810-bass", "8x10 bass cab", Cab),
        ("cab-default", "Cab", Cab),
    ]
};

fn skin_entry(
    id: &str,
) -> Option<&'static (&'static str, &'static str, ronnie_core::gear::GearCategory)> {
    SKIN_NAMES.iter().find(|(each, _, _)| *each == id)
}

/// The name the skin picker shows for a skin, such as "Chime combo"; empty for an unknown skin.
#[must_use]
pub fn friendly_name(id: &str) -> &'static str {
    skin_entry(id).map_or("", |(_, name, _)| name)
}

/// Every skin of one kind of gear, in the order of the picker.
#[must_use]
pub fn skins_in(category: ronnie_core::gear::GearCategory) -> Vec<&'static str> {
    SKIN_NAMES
        .iter()
        .filter(|(id, _, each)| *each == category && asset(id).is_some())
        .map(|(id, _, _)| *id)
        .collect()
}

/// The skin the block's gear family has, ignoring the block's override: what Auto draws.
#[must_use]
pub fn auto_skin_for(block: &ChainBlock) -> Option<BlockSkin> {
    let (_, identity) = block_identity(block)?;
    let family = identity.family;
    BlockSkin::of(asset(family.skin_id()?)?.id, identity.category)
}

/// The kind of gear a tone block holds, by the same identification as its skin; `None` for an
/// insert or an empty block.
#[must_use]
pub fn gear_category(block: &ChainBlock) -> Option<ronnie_core::gear::GearCategory> {
    Some(block_identity(block)?.1.category)
}

/// The skin of a block's gear, when the app has a render for it: the block's chosen skin when
/// it draws the same kind of gear as the automatic one, and otherwise the automatic one.
#[must_use]
pub fn skin_for(block: &ChainBlock) -> Option<BlockSkin> {
    let auto = auto_skin_for(block)?;
    Some(chosen_skin(block, auto).unwrap_or(auto))
}

/// The block's override, when the app has its render and it is of the automatic skin's kind.
#[must_use]
pub fn chosen_skin(block: &ChainBlock, auto: BlockSkin) -> Option<BlockSkin> {
    let (id, _, category) = skin_entry(block.skin.as_deref()?)?;
    if *category != auto.category {
        return None;
    }
    BlockSkin::of(asset(id)?.id, *category)
}

/// The skin a block shows, and the accessible name of its render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockSkin {
    /// Skin identity.
    pub id: &'static str,
    /// "Pedal render", "Amp render" or "Cab render".
    pub label: &'static str,
    /// The kind of gear the render shows.
    pub category: ronnie_core::gear::GearCategory,
}

impl BlockSkin {
    fn of(id: &'static str, category: ronnie_core::gear::GearCategory) -> Option<Self> {
        use ronnie_core::gear::GearCategory;

        let label = match category {
            GearCategory::Pedal => "Pedal render",
            GearCategory::Cab => "Cab render",
            _ => "Amp render",
        };
        Some(Self {
            id,
            label,
            category,
        })
    }
}

/// A skin's textures, uploaded once per context.
pub struct SkinTextures {
    /// Body size and knobs.
    pub manifest: SkinManifest,
    /// The body.
    pub body: TextureHandle,
    /// The filmstrip of each live knob.
    pub strips: Vec<(KnobControl, TextureHandle)>,
}

impl SkinTextures {
    /// The filmstrip of one live knob.
    #[must_use]
    pub fn strip(&self, control: KnobControl) -> Option<&TextureHandle> {
        self.strips
            .iter()
            .find(|(each, _)| *each == control)
            .map(|(_, texture)| texture)
    }
}

type SkinCache = HashMap<&'static str, Option<Arc<SkinTextures>>>;

/// The textures of a skin, decoded and uploaded on first use. `None` when the skin has no render
/// or its files do not decode.
#[must_use]
pub fn textures(ctx: &egui::Context, id: &str) -> Option<Arc<SkinTextures>> {
    let asset = asset(id)?;
    let cache_id = egui::Id::unique("ronnie-skin-textures");
    if let Some(cached) = ctx.data(|data| {
        data.get_temp::<SkinCache>(cache_id)
            .and_then(|cache| cache.get(asset.id).cloned())
    }) {
        return cached;
    }
    let loaded = load(ctx, asset).map(Arc::new);
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<SkinCache>(cache_id)
            .insert(asset.id, loaded.clone());
    });
    loaded
}

fn load(ctx: &egui::Context, asset: &SkinAsset) -> Option<SkinTextures> {
    let manifest = SkinManifest::parse(asset.manifest)?;
    let options = TextureOptions::LINEAR;
    let body = ctx.load_texture(format!("skin-{}", asset.id), decode(asset.body)?, options);
    let strips = manifest
        .knobs
        .iter()
        .filter_map(|knob| {
            let image = knob_strip(asset.id, knob.control)?;
            let name = format!("skin-{}-{:?}", asset.id, knob.control);
            Some((knob.control, ctx.load_texture(name, image, options)))
        })
        .collect();
    Some(SkinTextures {
        manifest,
        body,
        strips,
    })
}

/// Draws a skin's body in `body`, with each live knob at the position `position` gives it.
pub fn paint(
    painter: &egui::Painter,
    skin: &SkinTextures,
    body: Rect,
    position: impl Fn(KnobControl) -> f32,
    tint: Color32,
) {
    let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    painter.image(skin.body.id(), body, full, tint);
    for knob in &skin.manifest.knobs {
        if let Some(strip) = skin.strip(knob.control) {
            painter.image(
                strip.id(),
                knob.rect_in(body, &skin.manifest),
                knob.frame_uv(position(knob.control)),
                tint,
            );
        }
    }
}

/// Colour of the text on a pedal's LCD.
const LCD_INK: Color32 = Color32::from_rgb(0x9e, 0xff, 0xb3);
/// The glow around the LCD text: the ink at a low alpha, drawn in two rings of offset passes,
/// fainter further out.
const LCD_GLOW: [(f32, Color32); 2] = [
    (
        0.7,
        Color32::from_rgba_unmultiplied_const(0x9e, 0xff, 0xb3, 34),
    ),
    (
        1.5,
        Color32::from_rgba_unmultiplied_const(0x9e, 0xff, 0xb3, 14),
    ),
];
/// The dark pane inside the rendered glass, #0b120d at 85 %.
const LCD_PANE: Color32 = Color32::from_rgba_unmultiplied_const(0x0b, 0x12, 0x0d, 217);
/// The smallest LCD text. A longer name is cut, without an ellipsis.
const LCD_MIN_SIZE: f32 = 7.0;
/// What a pedal's LCD shows while its capture downloads.
pub const LCD_LOADING: &str = "\u{b7}\u{b7}\u{b7}";

/// The LCD text of a capture name.
#[must_use]
pub fn lcd_text(model_name: &str) -> String {
    model_name.to_uppercase()
}

/// Draws `text` on the skin's LCD glass when the skin has one, and returns where the glass is.
pub fn draw_lcd(
    painter: &egui::Painter,
    manifest: &SkinManifest,
    body: Rect,
    text: &str,
) -> Option<Rect> {
    let glass = manifest.lcd.as_ref()?.rect_in(body, manifest);
    paint_lcd(painter, glass, text);
    Some(glass)
}

/// Draws a dark pane inside the LCD glass in `rect`, and `text` centred on it, green with a soft
/// glow, at the largest size that fits the width down to 7 pt, and cut at the end when even that
/// is too wide.
pub fn paint_lcd(painter: &egui::Painter, rect: Rect, text: &str) {
    if !rect.is_positive() {
        return;
    }
    // The rendered glass reads mid-grey; a dark pane inside its bezel gives the text contrast.
    painter.rect_filled(
        rect.shrink(1.0),
        (rect.height() * 0.15).clamp(1.0, 3.0),
        LCD_PANE,
    );
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let pad = (rect.height() * 0.2).clamp(1.0, 4.0);
    let room = rect.width() - 2.0 * pad;
    let layout = |text: &str, size: f32| {
        painter.layout_no_wrap(text.to_owned(), egui::FontId::monospace(size), LCD_INK)
    };
    let mut size = (rect.height() * 0.62).max(LCD_MIN_SIZE);
    let mut galley = layout(text, size);
    while galley.size().x > room && size > LCD_MIN_SIZE {
        size = (size - 0.25).max(LCD_MIN_SIZE);
        galley = layout(text, size);
    }
    let mut shown = text.to_owned();
    while galley.size().x > room && shown.chars().count() > 1 {
        shown.pop();
        shown.truncate(shown.trim_end().len());
        galley = layout(&shown, size);
    }
    let pos = rect.center() - galley.size() / 2.0;
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    for (radius, glow) in LCD_GLOW {
        for step in 0..8 {
            let angle = step as f32 * std::f32::consts::FRAC_PI_4;
            let offset = egui::vec2(angle.cos(), angle.sin()) * radius;
            painter.galley_with_override_text_color(pos + offset, galley.clone(), glow);
        }
    }
    painter.galley(pos, galley, LCD_INK);
}

/// The smallest name on a plate. A longer name is cut with an ellipsis.
const PLATE_MIN_SIZE: f32 = 6.0;

/// The plate text of a capture name: printed in capitals, like the other labels of the render.
#[must_use]
pub fn plate_text(model_name: &str) -> String {
    model_name.trim().to_uppercase()
}

/// Prints `text` on the skin's name plate when the skin has one, and returns where the plate is.
///
/// It returns `None` and prints nothing when the skin has no plate, when `text` is blank, or when
/// the plate is too low for the smallest name.
pub fn draw_plate(
    painter: &egui::Painter,
    manifest: &SkinManifest,
    body: Rect,
    text: &str,
) -> Option<Rect> {
    let plate = manifest.plate.as_ref()?;
    let rect = plate.rect_in(body, manifest);
    paint_plate(painter, rect, text, plate.ink).then_some(rect)
}

/// Prints `text` left-aligned in `rect` in `ink`, at the plate's height, smaller down to 6 pt
/// when it is too wide, and cut with an ellipsis when even that does not fit.
///
/// It returns whether it printed: a blank text, or a plate lower than 6 pt, prints nothing.
pub fn paint_plate(painter: &egui::Painter, rect: Rect, text: &str, ink: Color32) -> bool {
    let Some(galley) = plate_galley(painter, rect, text, ink) else {
        return false;
    };
    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
    painter
        .with_clip_rect(rect.expand(1.0).intersect(painter.clip_rect()))
        .galley(pos, galley, ink);
    true
}

/// The one-row layout of `text` for a plate of `rect`, or `None` when nothing fits on it.
fn plate_galley(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    ink: Color32,
) -> Option<Arc<egui::Galley>> {
    let text = text.trim();
    // A 6 pt name in a lower plate loses its top and bottom to the clip.
    if !rect.is_positive() || rect.height() < PLATE_MIN_SIZE || text.is_empty() {
        return None;
    }
    let font = |size: f32| egui::FontId::new(size, egui::FontFamily::Name("arimo-bold".into()));
    let mut size = (rect.height() * 0.8).max(PLATE_MIN_SIZE);
    let width = |size: f32| {
        painter
            .layout_no_wrap(text.to_owned(), font(size), ink)
            .size()
            .x
    };
    while width(size) > rect.width() && size > PLATE_MIN_SIZE {
        size = (size - 0.25).max(PLATE_MIN_SIZE);
    }
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(font(size), ink),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: rect.width(),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    Some(painter.layout_job(job))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#""id": "x", "width": 600, "height": 800, "knobs": []"#;

    #[test]
    fn a_name_plate_is_read_with_its_ink() {
        let manifest = SkinManifest::parse(&format!(
            r#"{{{BASE}, "plate": {{"center": [100, 600.5], "size": [240, 20], "ink": [237, 237, 235]}}}}"#
        ))
        .expect("manifest");
        assert_eq!(
            manifest.plate,
            Some(SkinPlate {
                center: [100.0, 600.5],
                size: [240.0, 20.0],
                ink: Color32::from_rgb(237, 237, 235),
            })
        );
        let body = Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(300.0, 400.0));
        let plate = manifest
            .plate
            .as_ref()
            .expect("plate")
            .rect_in(body, &manifest);
        assert_eq!(plate.center(), egui::pos2(60.0, 300.25));
        assert_eq!(plate.size(), egui::vec2(120.0, 10.0));
        assert_eq!(
            plate_text("  Mesa Dual Rectifier | Rhythm "),
            "MESA DUAL RECTIFIER | RHYTHM"
        );
    }

    /// Runs `paint` in one frame of a context with the app fonts, and returns what it painted.
    fn plate_frame(mut paint: impl FnMut(&egui::Painter)) -> Vec<egui::epaint::ClippedShape> {
        let context = egui::Context::default();
        crate::fonts::Fonts::install(&context);
        let mut output = context.run_ui(egui::RawInput::default(), |ui| paint(ui.painter()));
        output.textures_delta.clear();
        output.shapes
    }

    /// The layout of `text` on a plate of `rect`.
    fn layout_on_plate(rect: Rect, text: &str) -> Option<Arc<egui::Galley>> {
        let mut galley = None;
        let _ = plate_frame(|painter| galley = plate_galley(painter, rect, text, Color32::WHITE));
        galley
    }

    /// The characters of the first row of `galley`, with an ellipsis that cut the text.
    fn shown_text(galley: &egui::Galley) -> String {
        galley
            .rows
            .first()
            .map(|row| row.glyphs.iter().map(|glyph| glyph.chr).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_long_name_shrinks_to_the_smallest_size_and_ends_with_an_ellipsis() {
        let rect = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(60.0, 10.0));
        let galley = layout_on_plate(rect, "MESA DUAL RECTIFIER | RHYTHM CHANNEL").expect("galley");
        assert_eq!(galley.rows.len(), 1, "the name keeps to one row");
        assert!(
            galley.size().x <= rect.width(),
            "{} fits {}",
            galley.size().x,
            rect.width()
        );
        let size = galley
            .job
            .sections
            .first()
            .expect("section")
            .format
            .font_id
            .size;
        assert!((size - PLATE_MIN_SIZE).abs() < f32::EPSILON, "size {size}");
        assert!(
            shown_text(&galley).ends_with('\u{2026}'),
            "{:?} ends with an ellipsis",
            shown_text(&galley)
        );
    }

    #[test]
    fn a_short_name_stays_whole_at_the_plate_height() {
        let rect = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(240.0, 10.0));
        let galley = layout_on_plate(rect, "  JCM800 ").expect("galley");
        assert_eq!(
            shown_text(&galley),
            "JCM800",
            "the name is trimmed and whole"
        );
        let size = galley
            .job
            .sections
            .first()
            .expect("section")
            .format
            .font_id
            .size;
        assert!((size - 8.0).abs() < f32::EPSILON, "size {size}");
    }

    #[test]
    fn a_blank_name_or_a_low_plate_prints_nothing() {
        let plate = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(240.0, 10.0));
        let low = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(240.0, 5.0));
        for (rect, text) in [(plate, ""), (plate, "   "), (low, "JCM800")] {
            assert!(
                layout_on_plate(rect, text).is_none(),
                "{text:?} in {rect:?}"
            );
            let mut printed = true;
            let shapes = plate_frame(|painter| {
                printed = paint_plate(painter, rect, text, Color32::WHITE);
            });
            assert!(!printed, "{text:?} in {rect:?}");
            assert!(shapes.is_empty(), "{text:?} in {rect:?} painted {shapes:?}");
        }
        let manifest = SkinManifest::parse(&format!(
            r#"{{{BASE}, "plate": {{"center": [300, 400], "size": [240, 20], "ink": [1, 2, 3]}}}}"#
        ))
        .expect("manifest");
        let body = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 800.0));
        let mut drawn = Some(Rect::NOTHING);
        let _ = plate_frame(|painter| drawn = draw_plate(painter, &manifest, body, " "));
        assert_eq!(drawn, None, "a blank name gives the plate no label");
    }

    #[test]
    fn the_printed_name_is_clipped_to_its_plate() {
        let rect = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(60.0, 10.0));
        let shapes = plate_frame(|painter| {
            assert!(paint_plate(
                painter,
                rect,
                "MESA DUAL RECTIFIER | RHYTHM CHANNEL",
                Color32::WHITE
            ));
        });
        let text = shapes
            .iter()
            .find(|clipped| matches!(clipped.shape, egui::Shape::Text(_)))
            .expect("the name is painted");
        assert_eq!(text.clip_rect, rect.expand(1.0));
    }

    #[test]
    fn a_malformed_plate_is_dropped_and_the_rest_of_the_manifest_kept() {
        for plate in [
            r#""plate": 3"#,
            r#""plate": {"center": [1, 2], "size": [3, 4]}"#,
            r#""plate": {"center": [1, 2], "size": [3, 4], "ink": [1, 2]}"#,
            r#""plate": {"center": [1, 2], "size": [3, 4], "ink": [1, 2, 300]}"#,
            r#""plate": {"center": [1], "size": [3, 4], "ink": [1, 2, 3]}"#,
        ] {
            let manifest = SkinManifest::parse(&format!("{{{BASE}, {plate}}}"))
                .unwrap_or_else(|| panic!("{plate} dropped the manifest"));
            assert_eq!(manifest.width, 600);
            assert!(manifest.plate.is_none(), "{plate}");
        }
    }

    #[test]
    fn a_malformed_lcd_is_dropped_and_the_rest_of_the_manifest_kept() {
        for lcd in [
            r#""lcd": 3"#,
            r#""lcd": {"center": [1, 2]}"#,
            r#""lcd": {"center": [1], "size": [3, 4]}"#,
            r#""lcd": {"center": [1, 2], "size": ["a", 4]}"#,
        ] {
            let manifest = SkinManifest::parse(&format!("{{{BASE}, {lcd}}}"))
                .unwrap_or_else(|| panic!("{lcd} dropped the manifest"));
            assert_eq!(manifest.width, 600);
            assert!(manifest.lcd.is_none(), "{lcd}");
        }
    }

    #[test]
    fn every_skin_has_a_friendly_name_and_one_kind() {
        use ronnie_core::gear::GearCategory::{Amp, Cab, Pedal};

        for asset in SKINS {
            assert!(!friendly_name(asset.id).is_empty(), "{}", asset.id);
        }
        assert_eq!(SKIN_NAMES.len(), SKINS.len());
        assert_eq!(skins_in(Pedal).len(), 8);
        assert_eq!(skins_in(Amp).len(), 9);
        assert_eq!(skins_in(Cab).len(), 6);
        assert_eq!(friendly_name("chime"), "Chime combo");
        assert_eq!(friendly_name("nope"), "");
    }

    #[test]
    fn a_well_formed_lcd_is_parsed() {
        let text = format!(r#"{{{BASE}, "lcd": {{"center": [1.5, 2], "size": [3, 4.5]}}}}"#);
        let manifest = SkinManifest::parse(&text).unwrap();
        assert_eq!(
            manifest.lcd,
            Some(SkinLcd {
                center: [1.5, 2.0],
                size: [3.0, 4.5],
            })
        );
    }
}
