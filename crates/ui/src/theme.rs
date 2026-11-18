//! Shared dark theme tokens for every native surface.

/// Colours taken from the Ronnie logo: chrome lettering over a flame ramp.
pub mod palette {
    use eframe::egui::Color32;

    /// Deep red at the cool edge of the flames.
    pub const EMBER: Color32 = Color32::from_rgb(0x90, 0x14, 0x10);
    /// Saturated flame red. Errors and clipping use this colour.
    pub const FIRE: Color32 = Color32::from_rgb(0xd8, 0x1e, 0x10);
    /// Orange-red flame. Active controls and accents use this colour.
    pub const FLAME: Color32 = Color32::from_rgb(0xf0, 0x50, 0x10);
    /// Orange flame. Warnings, links, and lit chips use this colour.
    pub const BLAZE: Color32 = Color32::from_rgb(0xf5, 0x96, 0x14);
    /// Hot center of the flames.
    pub const HOT: Color32 = Color32::from_rgb(0xff, 0xd8, 0x70);
    /// Silver of the logo lettering.
    pub const CHROME: Color32 = Color32::from_rgb(0xd0, 0xd0, 0xd4);
    /// Unlit tuner bars, a burnt-out ember.
    pub const COOLED: Color32 = Color32::from_rgb(0x2a, 0x10, 0x0c);

    /// Fill of a floating panel, a card, or a tile body.
    pub const PANEL: Color32 = Color32::from_rgb(21, 21, 23);
    /// Fill of a raised surface, such as a chip, a placeholder, or a knob face.
    pub const SURFACE_RAISED: Color32 = Color32::from_rgb(28, 28, 30);
    /// Solid edge of a floating menu, a field, or a card.
    pub const PANEL_EDGE: Color32 = Color32::from_rgb(63, 63, 70);
    /// Translucent hairline border.
    pub const HAIRLINE: Color32 = Color32::from_rgba_unmultiplied_const(84, 84, 88, 166);
    /// Fill of the model list.
    pub const LIST_FILL: Color32 = Color32::from_rgb(57, 57, 61);

    /// Solid grey for muted labels, line icons, and placeholder avatars.
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(141, 141, 147);
    /// Dim grey for captions, hints, and labels that are not selected or not enabled.
    pub const TEXT_SUBTLE: Color32 = Color32::from_rgb(93, 93, 97);
    /// Translucent label for secondary copy, such as status text and empty-state text.
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgba_unmultiplied_const(235, 235, 245, 153);
    /// Faint label on a graph axis or a graph marker.
    pub const GRAPH_LABEL: Color32 = Color32::from_rgba_unmultiplied_const(235, 235, 245, 89);
    /// Faint light fill for a selected pill or an unlit meter cell.
    pub const TINT_FILL: Color32 = Color32::from_rgba_unmultiplied_const(235, 235, 245, 46);
    /// Translucent fill of a control track or a pill.
    pub const CONTROL_FILL: Color32 = Color32::from_rgba_unmultiplied_const(120, 120, 128, 92);
    /// Light fill of a row or an icon button under the pointer.
    pub const HOVER_FILL: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 20);
    /// Fill of an icon plate on a tile under the pointer.
    pub const PLATE_HOVER: Color32 = Color32::from_rgb(70, 70, 76);
    /// Dark veil on artwork, under an icon plate or over a tile that is not ready.
    pub const ART_SCRIM: Color32 = Color32::from_black_alpha(166);
    /// Dark veil over a card or a toggle that is disabled or locked.
    pub const DISABLED_VEIL: Color32 = Color32::from_black_alpha(140);
    /// Dim layer behind a modal or over a busy card.
    pub const BACKDROP_DIM: Color32 = Color32::from_black_alpha(128);
    /// Veil over a faceplate knob that is switched off.
    pub const KNOB_VEIL: Color32 = Color32::from_rgba_unmultiplied_const(28, 28, 30, 140);
    /// Light metal of a knob cap highlight and of its pointer edge.
    pub const KNOB_METAL: Color32 = Color32::from_rgb(169, 169, 169);

    /// The one colour outside the logo: a perfectly tuned note.
    pub const IN_TUNE: Color32 = Color32::from_rgb(0x22, 0xd0, 0x5a);
    /// Tuner note that is close to the target.
    pub const TUNE_NEAR: Color32 = BLAZE;
    /// Tuner note that is far from the target.
    pub const TUNE_FAR: Color32 = FIRE;

    /// Maps 0 (quiet) to 1 (hot) along the flames of the logo.
    #[must_use]
    pub fn fire_ramp(position: f32) -> Color32 {
        const STOPS: [(f32, Color32); 5] = [
            (0.0, EMBER),
            (0.35, FIRE),
            (0.6, FLAME),
            (0.82, BLAZE),
            (1.0, HOT),
        ];
        let position = position.clamp(0.0, 1.0);
        for pair in STOPS.windows(2) {
            let [(from_at, from), (to_at, to)] = [pair[0], pair[1]];
            if position <= to_at {
                let fraction = (position - from_at) / (to_at - from_at);
                let mix = |a: u8, b: u8| {
                    (f32::from(a) + (f32::from(b) - f32::from(a)) * fraction).round() as u8
                };
                return Color32::from_rgb(
                    mix(from.r(), to.r()),
                    mix(from.g(), to.g()),
                    mix(from.b(), to.b()),
                );
            }
        }
        HOT
    }
}

/// Compatibility palette and common control dimensions.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Root black.
    pub black: [u8; 4],
    /// Primary white.
    pub white: [u8; 4],
    /// Card body background.
    pub surface: [u8; 4],
    /// Raised controls.
    pub surface_raised: [u8; 4],
    /// Floating panel background.
    pub panel: [u8; 4],
    /// Muted text and pressed fills.
    pub muted: [u8; 4],
    /// Subtle labels.
    pub subtle: [u8; 4],
    /// Hairline borders.
    pub border: [u8; 4],
    /// Link accent.
    pub link: [u8; 4],
    /// Cool logo accent.
    pub brand_ember: [u8; 4],
    /// Warm logo accent.
    pub brand_blaze: [u8; 4],
    /// Hot logo accent.
    pub brand_fire: [u8; 4],
    /// Floating-panel corner radius.
    pub panel_corner: f32,
    /// Icon glyph size inside a chrome box.
    pub icon_size: i32,
    /// Standard icon-button hit target.
    pub icon_box_size: i32,
    /// Primary knob footprint.
    pub knob_primary: i32,
    /// Secondary knob footprint.
    pub knob_secondary: i32,
    /// Disabled control opacity.
    pub disabled_opacity: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            black: [0, 0, 0, 255],
            white: [255, 255, 255, 255],
            surface: [0x15, 0x15, 0x17, 0xff],
            surface_raised: [0x1c, 0x1c, 0x1e, 0xff],
            panel: [0x14, 0x14, 0x16, 0xff],
            muted: [235, 235, 245, 153],
            subtle: [235, 235, 245, 102],
            border: [84, 84, 88, 166],
            link: palette::BLAZE.to_array(),
            brand_ember: palette::EMBER.to_array(),
            brand_blaze: palette::BLAZE.to_array(),
            brand_fire: palette::FIRE.to_array(),
            panel_corner: 14.0,
            icon_size: 14,
            icon_box_size: 20,
            knob_primary: 48,
            knob_secondary: 36,
            disabled_opacity: 0.45,
        }
    }
}

use eframe::egui;

/// Colours for one layout, looked up at run time so that a layout can turn light or dark.
///
/// The reference layout uses [`Look::reference`]; the Stage layout installs
/// [`Look::stage`] or [`Look::stage_dark`] before it draws. Code that opts in reads the
/// active look with [`Look::get`]; everything else keeps the constant [`palette`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    /// Which layout and which of light and dark this look is for.
    pub kind: LookKind,
    /// Window background behind every panel.
    pub background: egui::Color32,
    /// Radial light on the stage: centre, 70 %, and edge colours; `None` for a flat stage.
    pub spotlight: Option<[egui::Color32; 3]>,
    /// Raised panels such as the global controls, chips, cards and the skin list.
    pub panel: egui::Color32,
    /// A raised panel or a pill button under the pointer.
    pub panel_hover: egui::Color32,
    /// Hairlines and panel edges.
    pub edge: egui::Color32,
    /// Edge of a chip, a pill button, a field or a switch track.
    pub control_edge: egui::Color32,
    /// Edge of a card such as the EQ card, the MICS panel or the capture pill.
    pub card_edge: egui::Color32,
    /// Strong edge: an empty slot, or a chip under the pointer.
    pub edge_strong: egui::Color32,
    /// Soft fill of a chip, a switch track, a field, or a chosen row.
    pub fill_soft: egui::Color32,
    /// Soft fill of a field that is not enabled.
    pub fill_disabled: egui::Color32,
    /// A chip, such as an EQ band chip, under the pointer.
    pub chip_hover: egui::Color32,
    /// A list row under the pointer.
    pub row_hover: egui::Color32,
    /// Translucent wash over an icon button under the pointer.
    pub wash: egui::Color32,
    /// Primary text.
    pub text: egui::Color32,
    /// Secondary text such as values and captions.
    pub text_muted: egui::Color32,
    /// Body copy, such as a description, and text actions at rest.
    pub text_body: egui::Color32,
    /// A tab or a word that is not chosen.
    pub text_idle: egui::Color32,
    /// Text or a glyph that cannot be used now.
    pub text_subtle: egui::Color32,
    /// Fill of a selected solid control, such as an open slot or a pressed pill.
    pub solid: egui::Color32,
    /// Content on [`Look::solid`].
    pub solid_content: egui::Color32,
    /// Text and icon of a destructive action.
    pub danger: egui::Color32,
    /// Edge of a destructive action.
    pub danger_edge: egui::Color32,
    /// Fill of a floating black menu.
    pub menu: egui::Color32,
    /// Shadow under a popover such as the skin list.
    pub popover_shadow: egui::Color32,
    /// Outline of a floating menu, where it must stand out from the window.
    pub menu_outline: Option<egui::Color32>,
    /// Fill of an empty slot on the stage.
    pub slot_fill: egui::Color32,
    /// The utility bar at the bottom of the window.
    pub utility: egui::Color32,
    /// Hairline along the top of the utility bar.
    pub utility_hairline: Option<egui::Color32>,
    /// Text on the utility bar.
    pub utility_text: egui::Color32,
    /// Accent for active states.
    pub accent: egui::Color32,
    /// Chain icon of the selected block.
    pub icon_selected: egui::Color32,
    /// Chain icon of an active block.
    pub icon_on: egui::Color32,
    /// Chain icon of a bypassed block.
    pub icon_bypassed: egui::Color32,
    /// Wash over a control that is switched off.
    pub veil: egui::Color32,
    /// Track of a panel knob.
    pub knob_track: egui::Color32,
    /// Value arc of a panel knob.
    pub knob_arc: egui::Color32,
    /// Cap of a panel knob.
    pub knob_cap: egui::Color32,
    /// Edge of the cap of a panel knob.
    pub knob_cap_edge: egui::Color32,
    /// Highlight on the cap of a panel knob.
    pub knob_cap_highlight: egui::Color32,
    /// Shadow under the cap of a panel knob.
    pub knob_shadow: egui::Color32,
    /// Pointer of a panel knob.
    pub knob_pointer: egui::Color32,
    /// Label over a panel knob.
    pub knob_label: egui::Color32,
    /// Value under a panel knob.
    pub knob_value: egui::Color32,
}

/// The layout that a [`Look`] is for, and whether it is light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookKind {
    /// The reference layout, always dark.
    Reference,
    /// The light Stage layout.
    StageLight,
    /// The dark Stage layout.
    StageDark,
}

impl Look {
    /// Whether this is a Stage look, light or dark, and not the reference look.
    #[must_use]
    pub const fn is_stage(self) -> bool {
        matches!(self.kind, LookKind::StageLight | LookKind::StageDark)
    }

    /// Whether the window is dark.
    #[must_use]
    pub const fn is_dark(self) -> bool {
        matches!(self.kind, LookKind::Reference | LookKind::StageDark)
    }

    /// Today's dark colours.
    #[must_use]
    pub const fn reference() -> Self {
        Self {
            kind: LookKind::Reference,
            background: egui::Color32::BLACK,
            spotlight: None,
            panel: palette::SURFACE_RAISED,
            panel_hover: palette::PLATE_HOVER,
            edge: palette::HAIRLINE,
            control_edge: palette::PANEL_EDGE,
            card_edge: palette::PANEL_EDGE,
            edge_strong: palette::TEXT_SUBTLE,
            fill_soft: palette::CONTROL_FILL,
            fill_disabled: egui::Color32::from_rgb(19, 19, 20),
            chip_hover: palette::HAIRLINE,
            row_hover: palette::HOVER_FILL,
            wash: palette::HOVER_FILL,
            text: egui::Color32::WHITE,
            text_muted: palette::TEXT_MUTED,
            text_body: palette::CHROME,
            text_idle: palette::TEXT_MUTED,
            text_subtle: palette::TEXT_SUBTLE,
            solid: egui::Color32::WHITE,
            solid_content: egui::Color32::BLACK,
            danger: palette::FIRE,
            danger_edge: palette::EMBER,
            menu: palette::PANEL,
            popover_shadow: egui::Color32::from_black_alpha(140),
            menu_outline: Some(palette::PANEL_EDGE),
            slot_fill: palette::CONTROL_FILL,
            utility: egui::Color32::BLACK,
            utility_hairline: None,
            utility_text: egui::Color32::WHITE,
            accent: palette::FIRE,
            icon_selected: egui::Color32::WHITE,
            icon_on: palette::TEXT_MUTED,
            icon_bypassed: palette::TEXT_SUBTLE,
            veil: palette::KNOB_VEIL,
            knob_track: palette::SURFACE_RAISED,
            knob_arc: palette::FLAME,
            knob_cap: palette::SURFACE_RAISED,
            knob_cap_edge: egui::Color32::BLACK,
            knob_cap_highlight: palette::KNOB_METAL,
            knob_shadow: egui::Color32::from_black_alpha(153),
            knob_pointer: egui::Color32::WHITE,
            knob_label: palette::CHROME,
            knob_value: palette::TEXT_MUTED,
        }
    }

    /// The light Stage colours.
    #[must_use]
    pub const fn stage() -> Self {
        let ink = egui::Color32::from_rgb(0x11, 0x11, 0x11);
        let control_edge = egui::Color32::from_rgb(0xcf, 0xcf, 0xcc);
        Self {
            kind: LookKind::StageLight,
            background: egui::Color32::from_rgb(0xf3, 0xf3, 0xf1),
            spotlight: None,
            panel: egui::Color32::from_rgb(0xfb, 0xfb, 0xfa),
            panel_hover: egui::Color32::WHITE,
            edge: egui::Color32::from_rgb(0xd9, 0xd9, 0xd6),
            control_edge,
            card_edge: egui::Color32::from_rgb(0xdc, 0xdc, 0xd9),
            edge_strong: egui::Color32::from_rgb(0xa9, 0xa9, 0xa6),
            fill_soft: egui::Color32::from_rgb(0xe5, 0xe5, 0xe2),
            fill_disabled: egui::Color32::from_rgb(0xec, 0xec, 0xea),
            chip_hover: egui::Color32::from_rgb(0xd9, 0xd9, 0xd6),
            row_hover: egui::Color32::from_rgb(0xef, 0xef, 0xec),
            wash: egui::Color32::from_black_alpha(13),
            text: ink,
            text_muted: egui::Color32::from_rgb(0x5f, 0x5f, 0x5c),
            text_body: egui::Color32::from_rgb(0x33, 0x33, 0x33),
            text_idle: egui::Color32::from_rgb(0x6b, 0x6b, 0x68),
            text_subtle: egui::Color32::from_rgb(0xb4, 0xb4, 0xb0),
            solid: ink,
            solid_content: egui::Color32::WHITE,
            danger: egui::Color32::from_rgb(0xb3, 0x26, 0x1e),
            danger_edge: egui::Color32::from_rgb(0xe4, 0xb4, 0xad),
            menu: egui::Color32::from_rgb(0x0b, 0x0b, 0x0b),
            popover_shadow: egui::Color32::from_black_alpha(31),
            menu_outline: None,
            slot_fill: egui::Color32::from_rgba_premultiplied(89, 89, 89, 89),
            utility: egui::Color32::from_rgb(0x0b, 0x0b, 0x0b),
            utility_hairline: None,
            utility_text: egui::Color32::from_rgb(0xf2, 0xf2, 0xf2),
            accent: egui::Color32::from_rgb(0xff, 0x6a, 0x1a),
            icon_selected: ink,
            icon_on: egui::Color32::from_rgb(0x6b, 0x6b, 0x68),
            icon_bypassed: egui::Color32::from_rgb(0xb9, 0xb9, 0xb6),
            veil: egui::Color32::from_rgba_premultiplied(0xa7, 0xa7, 0xa6, 0xaa),
            knob_track: egui::Color32::from_rgb(0xe1, 0xe1, 0xde),
            knob_arc: ink,
            knob_cap: egui::Color32::from_rgb(0xe6, 0xe6, 0xe4),
            knob_cap_edge: egui::Color32::from_rgb(0xbd, 0xbd, 0xba),
            knob_cap_highlight: egui::Color32::from_rgba_unmultiplied_const(0xf7, 0xf7, 0xf6, 140),
            knob_shadow: egui::Color32::from_black_alpha(31),
            knob_pointer: ink,
            knob_label: egui::Color32::from_rgb(0x22, 0x22, 0x22),
            knob_value: egui::Color32::from_rgb(0x22, 0x22, 0x22),
        }
    }

    /// The dark Stage colours: the light layout in the palette of the reference layout.
    #[must_use]
    pub const fn stage_dark() -> Self {
        let hairline = egui::Color32::from_rgb(0x2c, 0x2c, 0x2f);
        let control_edge = palette::PANEL_EDGE;
        Self {
            kind: LookKind::StageDark,
            background: egui::Color32::BLACK,
            spotlight: Some([
                egui::Color32::from_rgb(0x1e, 0x1e, 0x21),
                egui::Color32::from_rgb(0x0b, 0x0b, 0x0c),
                egui::Color32::BLACK,
            ]),
            panel: palette::SURFACE_RAISED,
            panel_hover: hairline,
            edge: hairline,
            control_edge,
            card_edge: control_edge,
            edge_strong: egui::Color32::from_rgb(0x4a, 0x4a, 0x50),
            fill_soft: hairline,
            fill_disabled: egui::Color32::from_rgb(0x16, 0x16, 0x18),
            chip_hover: egui::Color32::from_rgb(0x3a, 0x3a, 0x3e),
            row_hover: egui::Color32::from_rgb(0x24, 0x24, 0x27),
            wash: egui::Color32::from_rgba_premultiplied(18, 18, 18, 18),
            text: egui::Color32::WHITE,
            text_muted: palette::TEXT_MUTED,
            text_body: palette::CHROME,
            text_idle: palette::TEXT_MUTED,
            text_subtle: palette::TEXT_SUBTLE,
            solid: egui::Color32::WHITE,
            solid_content: egui::Color32::BLACK,
            danger: egui::Color32::from_rgb(0xff, 0x6a, 0x5e),
            danger_edge: egui::Color32::from_rgb(0x6b, 0x21, 0x1b),
            menu: palette::PANEL,
            popover_shadow: egui::Color32::from_black_alpha(140),
            menu_outline: Some(control_edge),
            slot_fill: egui::Color32::from_rgba_premultiplied(8, 8, 8, 8),
            utility: egui::Color32::BLACK,
            utility_hairline: Some(hairline),
            utility_text: egui::Color32::from_rgb(0xf2, 0xf2, 0xf2),
            accent: palette::FLAME,
            icon_selected: egui::Color32::WHITE,
            icon_on: palette::TEXT_MUTED,
            icon_bypassed: palette::TEXT_SUBTLE,
            veil: egui::Color32::from_rgba_unmultiplied_const(0x1c, 0x1c, 0x1e, 0xaa),
            knob_track: hairline,
            knob_arc: palette::FLAME,
            knob_cap: egui::Color32::from_rgb(0x1a, 0x1a, 0x1c),
            knob_cap_edge: egui::Color32::BLACK,
            knob_cap_highlight: egui::Color32::from_rgba_unmultiplied_const(0x3c, 0x3c, 0x3e, 140),
            knob_shadow: egui::Color32::from_black_alpha(153),
            knob_pointer: egui::Color32::WHITE,
            knob_label: palette::CHROME,
            knob_value: palette::TEXT_MUTED,
        }
    }

    fn id() -> egui::Id {
        egui::Id::unique("ronnie-look")
    }

    /// Makes `look` the active look for this context.
    pub fn install(ctx: &egui::Context, look: Self) {
        ctx.data_mut(|data| data.insert_temp(Self::id(), look));
    }

    /// The active look, or the reference look when none was installed.
    #[must_use]
    pub fn get(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp(Self::id()))
            .unwrap_or_else(Self::reference)
    }
}
