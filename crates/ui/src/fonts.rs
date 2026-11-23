//! Embedded Arimo and Roboto Mono assets with CSS-compatible metrics.

/// One embedded font file and its style role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontFace {
    /// Arimo regular body text.
    ArimoRegular,
    /// Arimo bold body text.
    ArimoBold,
    /// Arimo italic body text.
    ArimoItalic,
    /// Arimo bold italic body text.
    ArimoBoldItalic,
    /// Roboto Mono regular chrome.
    RobotoMonoRegular,
    /// Roboto Mono bold chrome.
    RobotoMonoBold,
}

/// Stateless access to the immutable embedded font collection.
pub struct Fonts;

impl Fonts {
    /// All six embedded faces.
    #[must_use]
    pub const fn embedded() -> [FontFace; 6] {
        [
            FontFace::ArimoRegular,
            FontFace::ArimoBold,
            FontFace::ArimoItalic,
            FontFace::ArimoBoldItalic,
            FontFace::RobotoMonoRegular,
            FontFace::RobotoMonoBold,
        ]
    }

    /// Returns one immutable TrueType font file.
    #[must_use]
    pub fn bytes(face: FontFace) -> &'static [u8] {
        match face {
            FontFace::ArimoRegular => include_bytes!("../assets/fonts/Arimo-Regular.ttf"),
            FontFace::ArimoBold => include_bytes!("../assets/fonts/Arimo-Bold.ttf"),
            FontFace::ArimoItalic => include_bytes!("../assets/fonts/Arimo-Italic.ttf"),
            FontFace::ArimoBoldItalic => include_bytes!("../assets/fonts/Arimo-BoldItalic.ttf"),
            FontFace::RobotoMonoRegular => {
                include_bytes!("../assets/fonts/RobotoMono-Regular.ttf")
            }
            FontFace::RobotoMonoBold => include_bytes!("../assets/fonts/RobotoMono-Bold.ttf"),
        }
    }

    /// Registers every embedded face and the default egui families on one context.
    pub fn install(context: &eframe::egui::Context) {
        use eframe::egui::{FontData, FontDefinitions, FontFamily};

        let mut definitions = FontDefinitions::empty();
        for face in Self::embedded() {
            definitions.font_data.insert(
                font_name(face).to_owned(),
                std::sync::Arc::new(FontData::from_static(Self::bytes(face))),
            );
            definitions.families.insert(
                FontFamily::Name(font_name(face).into()),
                vec![font_name(face).to_owned()],
            );
        }
        definitions.families.insert(
            FontFamily::Proportional,
            vec![font_name(FontFace::ArimoRegular).to_owned()],
        );
        definitions.families.insert(
            FontFamily::Monospace,
            vec![font_name(FontFace::RobotoMonoRegular).to_owned()],
        );
        context.set_fonts(definitions);
    }
}

fn font_name(face: FontFace) -> &'static str {
    match face {
        FontFace::ArimoRegular => "arimo",
        FontFace::ArimoBold => "arimo-bold",
        FontFace::ArimoItalic => "arimo-italic",
        FontFace::ArimoBoldItalic => "arimo-bold-italic",
        FontFace::RobotoMonoRegular => "roboto-mono",
        FontFace::RobotoMonoBold => "roboto-mono-bold",
    }
}

/// Blink-compatible normal line height for Arimo body text.
#[must_use]
pub fn normal_line_height(px: f32) -> i32 {
    (px * 0.9052).round() as i32 + (px * 0.2119).round() as i32 + (px * 0.0327).round() as i32
}

/// Baseline within a CSS line box, including floored half-leading.
#[must_use]
pub fn css_baseline(px: f32, line_height_px: f32) -> f32 {
    let ascent = (px * 0.9052).round();
    let descent = (px * 0.2119).round();
    ((line_height_px - ascent - descent) * 0.5).floor() + ascent
}

/// Extra kerning in CSS em units converted to a ratio of the requested size.
#[must_use]
pub fn tracking_px(px: f32, em: f32) -> f32 {
    px * em
}
