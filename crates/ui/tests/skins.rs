//! The embedded gear skins: one per rendered family, clean edges, knobs where the manifest says.

use eframe::egui::{Rect, pos2, vec2};
use ronie_core::gear::{GearCategory, GearFamily};
use ronie_ui::default::skins::{self, KnobControl, SkinLcd, SkinManifest};

const FAMILIES: [GearFamily; 23] = [
    GearFamily::OdGreen,
    GearFamily::OdGold,
    GearFamily::Distortion,
    GearFamily::Fuzz,
    GearFamily::Boost,
    GearFamily::Compressor,
    GearFamily::PedalDefaultDark,
    GearFamily::PedalDefaultLight,
    GearFamily::HeadHighGain,
    GearFamily::HeadModernUs,
    GearFamily::HeadBritish,
    GearFamily::ComboChime,
    GearFamily::ComboClean,
    GearFamily::HeadBass,
    GearFamily::AmpDefaultClean,
    GearFamily::AmpDefaultCrunch,
    GearFamily::AmpDefaultHiGain,
    GearFamily::Cab412Modern,
    GearFamily::Cab412Vintage,
    GearFamily::Cab212,
    GearFamily::Cab112,
    GearFamily::Cab810,
    GearFamily::CabDefault,
];

#[test]
fn every_rendered_family_has_a_skin() {
    for family in FAMILIES {
        let id = family.skin_id().expect("rendered family");
        let manifest = skins::manifest(id).unwrap_or_else(|| panic!("{id} has no manifest"));
        assert_eq!(manifest.id, id);
        assert!(skins::body_image(id).is_some(), "{id} has no body");
    }
}

#[test]
fn families_without_a_render_have_no_skin() {
    for family in [
        GearFamily::FullRig,
        GearFamily::Outboard,
        GearFamily::Space,
        GearFamily::Unknown,
    ] {
        assert!(family.skin_id().and_then(skins::manifest).is_none());
    }
    assert!(skins::manifest("no-such-skin").is_none());
}

#[test]
fn bodies_are_sharp_enough_for_a_300_point_stage_at_2x() {
    for family in FAMILIES {
        let id = family.skin_id().unwrap();
        let body = skins::body_image(id).unwrap();
        let manifest = skins::manifest(id).unwrap();
        assert_eq!(
            body.size,
            [manifest.width as usize, manifest.height as usize],
            "{id}"
        );
        assert!(body.size[1] >= 600, "{id} is {:?}", body.size);
    }
}

#[test]
fn bodies_have_no_haze_at_their_edges() {
    for family in FAMILIES {
        let id = family.skin_id().unwrap();
        let body = skins::body_image(id).unwrap();
        let [w, h] = body.size;
        let alpha = |x: usize, y: usize| body.pixels[y * w + x].a();
        for x in 0..w {
            for y in [0, 1, h - 2, h - 1] {
                assert!(
                    alpha(x, y) <= 2,
                    "{id} has alpha {} at ({x}, {y})",
                    alpha(x, y)
                );
            }
        }
        for y in 0..h {
            for x in [0, 1, w - 2, w - 1] {
                assert!(
                    alpha(x, y) <= 2,
                    "{id} has alpha {} at ({x}, {y})",
                    alpha(x, y)
                );
            }
        }
    }
}

#[test]
fn pedals_and_amps_have_three_live_knobs_and_cabs_none() {
    for family in FAMILIES {
        let id = family.skin_id().unwrap();
        let manifest = skins::manifest(id).unwrap();
        let mut controls: Vec<_> = manifest.knobs.iter().map(|knob| knob.control).collect();
        controls.sort();
        if family.category() == GearCategory::Cab {
            assert!(controls.is_empty(), "{id}");
        } else {
            assert_eq!(
                controls,
                [KnobControl::Gain, KnobControl::Output, KnobControl::Mix],
                "{id}"
            );
        }
    }
}

#[test]
fn knobs_sit_inside_the_body_and_their_strips_hold_every_frame() {
    for family in FAMILIES {
        let id = family.skin_id().unwrap();
        let manifest = skins::manifest(id).unwrap();
        for knob in &manifest.knobs {
            let half = knob.side / 2.0;
            assert!(knob.center[0] - half >= 0.0 && knob.center[0] + half <= manifest.width as f32);
            assert!(
                knob.center[1] - half >= 0.0 && knob.center[1] + half <= manifest.height as f32
            );
            let strip = skins::knob_strip(id, knob.control).expect("strip");
            let rows = knob.frames.div_ceil(knob.columns) as usize;
            assert_eq!(
                strip.size,
                [
                    knob.columns as usize * knob.side as usize,
                    rows * knob.side as usize
                ],
                "{id} {:?}",
                knob.control
            );
        }
    }
}

#[test]
fn every_pedal_skin_has_an_lcd_inside_its_body() {
    let pedals = [
        "od-green",
        "od-gold",
        "dist-black",
        "fuzz",
        "boost",
        "comp",
        "default-dark",
        "default-light",
    ];
    for family in FAMILIES {
        if family.category() == GearCategory::Pedal {
            assert!(pedals.contains(&family.skin_id().unwrap()));
        }
    }
    for id in pedals {
        let manifest = skins::manifest(id).unwrap();
        let lcd = manifest
            .lcd
            .as_ref()
            .unwrap_or_else(|| panic!("{id} has no LCD"));
        let [w, h] = lcd.size;
        assert!(w > 0.0 && h > 0.0, "{id} LCD is {w} x {h}");
        let [x, y] = lcd.center;
        assert!(
            x - w / 2.0 >= 0.0 && x + w / 2.0 <= manifest.width as f32,
            "{id} LCD spans x {} to {}",
            x - w / 2.0,
            x + w / 2.0
        );
        assert!(
            y - h / 2.0 >= 0.0 && y + h / 2.0 <= manifest.height as f32,
            "{id} LCD spans y {} to {}",
            y - h / 2.0,
            y + h / 2.0
        );
    }
}

#[test]
fn amps_and_cabs_have_no_lcd() {
    for family in FAMILIES {
        if family.category() == GearCategory::Pedal {
            continue;
        }
        let id = family.skin_id().unwrap();
        assert!(skins::manifest(id).unwrap().lcd.is_none(), "{id}");
    }
}

#[test]
fn an_lcd_rect_scales_with_the_body() {
    let manifest = SkinManifest {
        id: "test".to_owned(),
        width: 600,
        height: 800,
        knobs: Vec::new(),
        lcd: Some(SkinLcd {
            center: [300.0, 200.0],
            size: [90.0, 30.0],
        }),
    };
    let lcd = manifest.lcd.as_ref().unwrap();
    let full = Rect::from_min_size(pos2(10.0, 20.0), vec2(600.0, 800.0));
    let rect = lcd.rect_in(full, &manifest);
    assert_eq!(rect.center(), pos2(310.0, 220.0));
    assert_eq!(rect.size(), vec2(90.0, 30.0));
    let half = Rect::from_min_size(pos2(100.0, 50.0), vec2(300.0, 400.0));
    let rect = lcd.rect_in(half, &manifest);
    assert_eq!(rect.center(), pos2(250.0, 150.0));
    assert_eq!(rect.size(), vec2(45.0, 15.0));
}

#[test]
fn a_frame_uv_walks_the_strip_row_by_row() {
    let manifest = skins::manifest("od-green").unwrap();
    let knob = &manifest.knobs[0];
    let first = knob.frame_uv(0.0);
    assert_eq!(first.min, eframe::egui::pos2(0.0, 0.0));
    let last = knob.frame_uv(1.0);
    assert!((last.max.x - 1.0).abs() < 1e-6 && (last.max.y - 1.0).abs() < 1e-6);
    let ninth = knob.frame_uv(8.0 / 31.0);
    assert!((ninth.min.x).abs() < 1e-6 && (ninth.min.y - 0.25).abs() < 1e-6);
}

#[test]
fn the_render_scripts_print_no_maker_or_model_names() {
    let scripts = [
        include_str!("../../../tools/render/pedals.py"),
        include_str!("../../../tools/render/gear.py"),
    ];
    let banned = [
        "big muff",
        "rat 2",
        "dyna comp",
        "centaur",
        "klon",
        "ep boost",
        "tube screamer",
        "ts808",
        "5150",
        "rectifier",
        "jcm",
        "plexi",
        "ac30",
        "ac15",
        "twin reverb",
        "deluxe",
        "svt",
        "greenback",
        "v30",
        "marshall",
        "vox",
        "fender",
        "mesa",
        "ampeg",
        "celestion",
        "boss",
        "ibanez",
        "evh",
        "peavey",
        "electro-harmonix",
    ];
    for script in scripts {
        let quoted: Vec<String> = script
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_lowercase)
            .collect();
        for name in banned {
            assert!(
                !quoted.iter().any(|text| text.contains(name)),
                "a render script prints \"{name}\""
            );
        }
    }
}

#[test]
fn floor_shadows_stay_faint_on_the_light_stage() {
    for family in FAMILIES {
        let id = family.skin_id().unwrap();
        let body = skins::body_image(id).unwrap();
        let darkest_shadow = body
            .pixels
            .iter()
            // Shadow pixels are black; the colour is read unmultiplied so that a dark,
            // half-covered edge of the gear itself is not taken for shadow.
            .map(|pixel| pixel.to_srgba_unmultiplied())
            .filter(|[r, g, b, a]| *a < 200 && (*r).max(*g).max(*b) < 4)
            .map(|[_, _, _, a]| a)
            .max()
            .unwrap_or(0);
        assert!(
            darkest_shadow <= 96,
            "{id} has a floor shadow of alpha {darkest_shadow}"
        );
    }
}

#[test]
fn floor_shadows_fade_out_before_the_frame_edge() {
    for family in FAMILIES {
        let id = family.skin_id().unwrap();
        let body = skins::body_image(id).unwrap();
        let [w, h] = body.size;
        let band = (w.min(h) as f32 * 0.02) as usize;
        let mut strongest = 0;
        for y in 0..h {
            for x in 0..w {
                let near_edge = x < band || y < band || x >= w - band || y >= h - band;
                // Shadow pixels are black; the colour is read unmultiplied so that a dark,
                // half-covered edge of the gear itself is not taken for shadow.
                let [r, g, b, a] = body.pixels[y * w + x].to_srgba_unmultiplied();
                let is_shadow = a < 200 && r.max(g).max(b) < 4;
                if near_edge && is_shadow {
                    strongest = strongest.max(a);
                }
            }
        }
        assert!(
            strongest <= 32,
            "{id} has a floor shadow of alpha {strongest} at its frame edge"
        );
    }
}

/// Text shapes that one pass of `paint` adds.
fn painted_texts(paint: impl FnOnce(&eframe::egui::Painter)) -> Vec<String> {
    use eframe::egui;

    let context = egui::Context::default();
    ronie_ui::fonts::Fonts::install(&context);
    let mut texts = Vec::new();
    let mut paint = Some(paint);
    let mut output = context.run_ui(egui::RawInput::default(), |ui| {
        if let Some(paint) = paint.take() {
            paint(ui.painter());
        }
    });
    output.textures_delta.clear();
    for clipped in output.shapes {
        collect_texts(&clipped.shape, &mut texts);
    }
    texts
}

fn collect_texts(shape: &eframe::egui::Shape, texts: &mut Vec<String>) {
    match shape {
        eframe::egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
        eframe::egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_texts(shape, texts);
            }
        }
        _ => {}
    }
}

fn lcd_manifest(lcd: Option<SkinLcd>) -> SkinManifest {
    SkinManifest {
        id: "test".to_owned(),
        width: 600,
        height: 800,
        knobs: Vec::new(),
        lcd,
    }
}

#[test]
fn a_skin_without_an_lcd_draws_none() {
    let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(225.0, 300.0));
    let manifest = lcd_manifest(None);
    let mut drawn = Some(Rect::NOTHING);
    let texts = painted_texts(|painter| {
        drawn = skins::draw_lcd(painter, &manifest, body, "DRIVE 5");
    });
    assert_eq!(drawn, None);
    assert!(texts.is_empty(), "{texts:?}");

    let manifest = lcd_manifest(Some(SkinLcd {
        center: [300.0, 200.0],
        size: [240.0, 60.0],
    }));
    let texts = painted_texts(|painter| {
        drawn = skins::draw_lcd(painter, &manifest, body, "DRIVE 5");
    });
    assert!(drawn.is_some());
    assert!(texts.iter().any(|text| text == "DRIVE 5"), "{texts:?}");
}

#[test]
fn lcd_text_is_uppercase() {
    assert_eq!(skins::lcd_text("Drive 5 (bright)"), "DRIVE 5 (BRIGHT)");
}

#[test]
fn lcd_text_shrinks_then_truncates_inside_the_glass() {
    let glass = Rect::from_min_size(pos2(0.0, 0.0), vec2(40.0, 12.0));
    let texts = painted_texts(|painter| {
        skins::paint_lcd(painter, glass, "A VERY LONG CAPTURE NAME INDEED");
    });
    let shown = texts.first().expect("text").clone();
    assert!(texts.iter().all(|text| *text == shown), "{texts:?}");
    assert!(
        "A VERY LONG CAPTURE NAME INDEED".starts_with(&shown) && !shown.is_empty(),
        "{shown}"
    );
    assert!(!shown.contains('…'), "no ellipsis");
    assert!(shown.len() < 31, "truncated: {shown}");
}
