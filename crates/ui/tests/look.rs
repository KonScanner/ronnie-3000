//! The runtime look: light Stage colours and the dark reference colours.

use eframe::egui::{self, Color32};
use ronie_ui::theme::{Look, LookKind, palette};

fn luminance(color: Color32) -> f64 {
    let channel = |value: u8| {
        let c = f64::from(value) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

fn contrast(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[test]
fn stage_look_matches_the_spec() {
    let look = Look::stage();
    assert_eq!(look.background, Color32::from_rgb(0xf3, 0xf3, 0xf1));
    assert_eq!(look.panel, Color32::from_rgb(0xfb, 0xfb, 0xfa));
    assert_eq!(look.edge, Color32::from_rgb(0xd9, 0xd9, 0xd6));
    assert_eq!(look.text, Color32::from_rgb(0x11, 0x11, 0x11));
    assert_eq!(look.text_muted, Color32::from_rgb(0x5f, 0x5f, 0x5c));
    assert_eq!(look.utility, Color32::from_rgb(0x0b, 0x0b, 0x0b));
    assert_eq!(look.utility_text, Color32::from_rgb(0xf2, 0xf2, 0xf2));
    assert_eq!(look.accent, Color32::from_rgb(0xff, 0x6a, 0x1a));
    assert_eq!(look.icon_selected, Color32::from_rgb(0x11, 0x11, 0x11));
    assert_eq!(look.icon_on, Color32::from_rgb(0x6b, 0x6b, 0x68));
    assert_eq!(look.icon_bypassed, Color32::from_rgb(0xb9, 0xb9, 0xb6));
}

#[test]
fn get_falls_back_to_reference() {
    let context = egui::Context::default();
    assert_eq!(Look::get(&context), Look::reference());
}

#[test]
fn install_then_get_round_trips() {
    let context = egui::Context::default();
    Look::install(&context, Look::stage());
    assert_eq!(Look::get(&context), Look::stage());
    Look::install(&context, Look::reference());
    assert_eq!(Look::get(&context), Look::reference());
}

#[test]
fn stage_text_is_readable() {
    let look = Look::stage();
    for ground in [look.background, look.panel] {
        assert!(contrast(look.text, ground) >= 4.5);
        assert!(contrast(look.text_muted, ground) >= 4.5);
    }
    assert!(contrast(look.utility_text, look.utility) >= 4.5);
}

#[test]
fn reference_look_keeps_todays_black_chrome() {
    let look = Look::reference();
    assert_eq!(look.background, Color32::BLACK);
    assert_eq!(look.utility, Color32::BLACK);
}

#[test]
fn reference_tokens_keep_the_palette() {
    // The reference layout is today's chrome: every token must be the palette constant that
    // the code used before the Stage layout moved colours into `Look`. A wrong constant in
    // `Look::reference()` fails here, so the Linux tests prove the claim of no visible change.
    let look = Look::reference();
    let rgb = |r, g, b| Color32::from_rgb(r, g, b);
    assert_eq!(look.kind, LookKind::Reference);
    assert_eq!(look.background, Color32::BLACK);
    assert_eq!(look.spotlight, None);
    assert_eq!(look.panel, palette::SURFACE_RAISED);
    assert_eq!(look.panel_hover, palette::PLATE_HOVER);
    assert_eq!(look.edge, palette::HAIRLINE);
    assert_eq!(look.control_edge, palette::PANEL_EDGE);
    assert_eq!(look.card_edge, palette::PANEL_EDGE);
    assert_eq!(look.edge_strong, palette::TEXT_SUBTLE);
    assert_eq!(look.fill_soft, palette::CONTROL_FILL);
    assert_eq!(look.fill_disabled, rgb(19, 19, 20));
    assert_eq!(look.chip_hover, palette::HAIRLINE);
    assert_eq!(look.row_hover, palette::HOVER_FILL);
    assert_eq!(look.wash, palette::HOVER_FILL);
    assert_eq!(look.text, Color32::WHITE);
    assert_eq!(look.text_muted, palette::TEXT_MUTED);
    assert_eq!(look.text_body, palette::CHROME);
    assert_eq!(look.text_idle, palette::TEXT_MUTED);
    assert_eq!(look.text_subtle, palette::TEXT_SUBTLE);
    assert_eq!(look.solid, Color32::WHITE);
    assert_eq!(look.solid_content, Color32::BLACK);
    assert_eq!(look.danger, palette::FIRE);
    assert_eq!(look.danger_edge, palette::EMBER);
    assert_eq!(look.menu, palette::PANEL);
    assert_eq!(look.popover_shadow, Color32::from_black_alpha(140));
    assert_eq!(look.menu_outline, Some(palette::PANEL_EDGE));
    assert_eq!(look.slot_fill, palette::CONTROL_FILL);
    assert_eq!(look.utility, Color32::BLACK);
    assert_eq!(look.utility_hairline, None);
    assert_eq!(look.utility_text, Color32::WHITE);
    assert_eq!(look.accent, palette::FIRE);
    assert_eq!(look.icon_selected, Color32::WHITE);
    assert_eq!(look.icon_on, palette::TEXT_MUTED);
    assert_eq!(look.icon_bypassed, palette::TEXT_SUBTLE);
    assert_eq!(look.veil, palette::KNOB_VEIL);
    assert_eq!(look.knob_track, palette::SURFACE_RAISED);
    assert_eq!(look.knob_arc, palette::FLAME);
    assert_eq!(look.knob_cap, palette::SURFACE_RAISED);
    assert_eq!(look.knob_cap_edge, Color32::BLACK);
    assert_eq!(look.knob_cap_highlight, palette::KNOB_METAL);
    assert_eq!(look.knob_shadow, Color32::from_black_alpha(153));
    assert_eq!(look.knob_pointer, Color32::WHITE);
    assert_eq!(look.knob_label, palette::CHROME);
    assert_eq!(look.knob_value, palette::TEXT_MUTED);
}

#[test]
fn stage_dark_uses_the_reference_palette() {
    let look = Look::stage_dark();
    let rgb = |r, g, b| Color32::from_rgb(r, g, b);
    assert_eq!(look.kind, LookKind::StageDark);
    assert_eq!(look.background, Color32::BLACK);
    assert_eq!(look.panel, rgb(0x1c, 0x1c, 0x1e));
    assert_eq!(look.accent, ronie_ui::theme::palette::FLAME);
    assert_eq!(look.text, Color32::WHITE);
    assert_eq!(look.text_muted, rgb(0x8d, 0x8d, 0x93));
    assert_eq!(look.text_body, rgb(0xd0, 0xd0, 0xd4));
    assert_eq!(look.text_subtle, rgb(0x5d, 0x5d, 0x61));
    assert_eq!(look.text_idle, rgb(0x8d, 0x8d, 0x93));
    assert_eq!(look.edge, rgb(0x2c, 0x2c, 0x2f));
    assert_eq!(look.control_edge, rgb(0x3f, 0x3f, 0x46));
    assert_eq!(look.card_edge, rgb(0x3f, 0x3f, 0x46));
    assert_eq!(look.fill_soft, rgb(0x2c, 0x2c, 0x2f));
    assert_eq!(look.solid, Color32::WHITE);
    assert_eq!(look.solid_content, Color32::BLACK);
    assert_eq!(look.danger, rgb(0xff, 0x6a, 0x5e));
    assert_eq!(look.danger_edge, rgb(0x6b, 0x21, 0x1b));
    assert_eq!(look.menu, rgb(0x15, 0x15, 0x17));
    assert_eq!(look.menu_outline, Some(rgb(0x3f, 0x3f, 0x46)));
    assert_eq!(look.utility, Color32::BLACK);
    assert_eq!(look.utility_hairline, Some(rgb(0x2c, 0x2c, 0x2f)));
    assert_eq!(look.knob_track, rgb(0x2c, 0x2c, 0x2f));
    assert_eq!(look.knob_arc, ronie_ui::theme::palette::FLAME);
    assert_eq!(look.icon_selected, Color32::WHITE);
    assert_eq!(look.icon_on, rgb(0x8d, 0x8d, 0x93));
    assert_eq!(look.icon_bypassed, rgb(0x5d, 0x5d, 0x61));
    assert_eq!(
        look.spotlight,
        Some([rgb(0x1e, 0x1e, 0x21), rgb(0x0b, 0x0b, 0x0c), Color32::BLACK])
    );
    for ground in [look.background, look.panel, look.menu] {
        assert!(contrast(look.text, ground) >= 4.5);
        assert!(contrast(look.text_muted, ground) >= 4.5);
    }
}

#[test]
fn light_stage_tokens_match_the_old_constants() {
    let look = Look::stage();
    let rgb = |r, g, b| Color32::from_rgb(r, g, b);
    assert_eq!(look.kind, LookKind::StageLight);
    assert_eq!(look.spotlight, None);
    assert_eq!(look.fill_soft, rgb(0xe5, 0xe5, 0xe2));
    assert_eq!(look.fill_disabled, rgb(0xec, 0xec, 0xea));
    assert_eq!(look.row_hover, rgb(0xef, 0xef, 0xec));
    assert_eq!(look.wash, Color32::from_black_alpha(13));
    assert_eq!(look.panel_hover, Color32::WHITE);
    assert_eq!(look.control_edge, rgb(0xcf, 0xcf, 0xcc));
    assert_eq!(look.card_edge, rgb(0xdc, 0xdc, 0xd9));
    assert_eq!(look.edge_strong, rgb(0xa9, 0xa9, 0xa6));
    assert_eq!(look.text_body, rgb(0x33, 0x33, 0x33));
    assert_eq!(look.text_idle, rgb(0x6b, 0x6b, 0x68));
    assert_eq!(look.text_subtle, rgb(0xb4, 0xb4, 0xb0));
    assert_eq!(look.solid, rgb(0x11, 0x11, 0x11));
    assert_eq!(look.solid_content, Color32::WHITE);
    assert_eq!(look.danger, rgb(0xb3, 0x26, 0x1e));
    assert_eq!(look.danger_edge, rgb(0xe4, 0xb4, 0xad));
    assert_eq!(look.menu, rgb(0x0b, 0x0b, 0x0b));
    assert_eq!(look.menu_outline, None);
    assert_eq!(look.utility_hairline, None);
    assert_eq!(look.slot_fill, Color32::from_white_alpha(89));
    assert_eq!(look.knob_track, rgb(0xe1, 0xe1, 0xde));
    assert_eq!(look.knob_arc, rgb(0x11, 0x11, 0x11));
    assert_eq!(look.knob_cap, rgb(0xe6, 0xe6, 0xe4));
    assert_eq!(look.knob_cap_edge, rgb(0xbd, 0xbd, 0xba));
    assert_eq!(
        look.knob_cap_highlight,
        Color32::from_rgba_unmultiplied(0xf7, 0xf7, 0xf6, 140)
    );
    assert_eq!(look.knob_shadow, Color32::from_black_alpha(31));
    assert_eq!(look.knob_pointer, rgb(0x11, 0x11, 0x11));
    assert_eq!(look.knob_label, rgb(0x22, 0x22, 0x22));
    assert_eq!(look.knob_value, rgb(0x22, 0x22, 0x22));
}

#[test]
fn a_hovered_chip_has_its_own_token() {
    let rgb = |r, g, b| Color32::from_rgb(r, g, b);
    let dark = Look::stage_dark();
    assert_eq!(dark.chip_hover, rgb(0x3a, 0x3a, 0x3e));
    assert_ne!(dark.chip_hover, dark.fill_soft, "a hovered chip stands out");
    // Light and reference keep the edge colour that they hovered with before.
    assert_eq!(Look::stage().chip_hover, rgb(0xd9, 0xd9, 0xd6));
    assert_eq!(Look::reference().chip_hover, Look::reference().edge);
}

#[test]
fn each_look_knows_its_layout_and_shade() {
    let reference = Look::reference();
    assert!(!reference.is_stage() && reference.is_dark());
    let light = Look::stage();
    assert!(light.is_stage() && !light.is_dark());
    assert_eq!(light.popover_shadow, Color32::from_black_alpha(31));
    let dark = Look::stage_dark();
    assert!(dark.is_stage() && dark.is_dark());
    assert_eq!(dark.popover_shadow, Color32::from_black_alpha(140));
}
