//! Family-level gear glyphs: recognised captures show their gear family, the rest keep the
//! category glyph.

use eframe::egui;
use ronie_core::Tone;
use ronie_core::gear::GearFamily;
use ronie_ui::default::tile::{GearGlyph, draw_gear, tone_glyph};
use serde_json::json;

fn tone(value: serde_json::Value) -> Tone {
    Tone::from_value(value).expect("tone parses")
}

#[test]
fn a_recognised_catalog_amp_shows_its_family() {
    let ac30 = tone(json!({
        "id": 101, "title": "'02 Vox AC30/6 Top Boost", "format": "nam", "gear": "amp",
        "models": [{"id": 1101, "name": "AC30-6 TB Brl 3"}],
        "makes": [{"id": 11, "name": "Vox"}, {"id": 12, "name": "AC30/6 Top Boost"}]
    }));
    assert_eq!(
        tone_glyph(&ac30, None),
        GearGlyph::Family(GearFamily::ComboChime)
    );
}

#[test]
fn a_recognised_cab_shows_its_speaker_layout() {
    let greenback = tone(json!({
        "id": 103, "title": "UK Greenback - Marshall 1960TV 4x12 M201", "format": "ir", "gear": "cab",
        "models": [{"id": 1103, "name": "SM57 cap edge"}],
        "makes": [{"id": 15, "name": "Marshall"}, {"id": 16, "name": "1960TV"}]
    }));
    assert_eq!(
        tone_glyph(&greenback, None),
        GearGlyph::Family(GearFamily::Cab412Vintage)
    );
}

#[test]
fn an_unrecognised_capture_keeps_its_category_glyph() {
    let amp = tone(json!({
        "id": 200, "title": "bedroom capture", "format": "nam", "gear": "amp",
        "models": [{"id": 1, "name": "one"}]
    }));
    let mystery = tone(json!({
        "id": 201, "title": "something", "format": "nam",
        "models": [{"id": 2, "name": "two"}]
    }));
    assert_eq!(tone_glyph(&amp, None), GearGlyph::Amp);
    assert_eq!(tone_glyph(&mystery, None), GearGlyph::Generic);
}

#[test]
fn local_nam_metadata_picks_the_family() {
    let local = tone(json!({
        "id": 0, "local": true, "title": "my captures", "format": "nam",
        "models": [{
            "id": 7, "name": "lead", "model_url": "file:///tmp/stash/abc-1.nam",
            "gear": "amp", "gear_make": "EVH", "gear_model": "5150 III"
        }]
    }));
    assert_eq!(
        tone_glyph(&local, local.models.first()),
        GearGlyph::Family(GearFamily::HeadHighGain)
    );
}

const DRAWN_FAMILIES: &[GearFamily] = &[
    GearFamily::OdGreen,
    GearFamily::OdGold,
    GearFamily::Distortion,
    GearFamily::Fuzz,
    GearFamily::Boost,
    GearFamily::Compressor,
    GearFamily::HeadHighGain,
    GearFamily::HeadModernUs,
    GearFamily::HeadBritish,
    GearFamily::ComboChime,
    GearFamily::ComboClean,
    GearFamily::HeadBass,
    GearFamily::Cab412Modern,
    GearFamily::Cab212,
    GearFamily::Cab112,
    GearFamily::Cab810,
];

fn painted(glyph: GearGlyph) -> String {
    let context = egui::Context::default();
    let mut output = context.run_ui(egui::RawInput::default(), |ui| {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(224.0, 224.0));
        draw_gear(ui.painter(), rect, glyph);
    });
    output.textures_delta.clear();
    let shapes: Vec<String> = output
        .shapes
        .iter()
        .map(|clipped| format!("{:?}", clipped.shape))
        .collect();
    assert!(!shapes.is_empty(), "{glyph:?} painted nothing");
    shapes.join("\n")
}

#[test]
fn every_family_glyph_paints_and_is_distinct() {
    let drawings: Vec<String> = DRAWN_FAMILIES
        .iter()
        .map(|family| painted(GearGlyph::Family(*family)))
        .collect();
    for (index, drawing) in drawings.iter().enumerate() {
        for (other, other_drawing) in drawings.iter().enumerate().skip(index + 1) {
            assert_ne!(
                drawing, other_drawing,
                "{:?} and {:?} look identical",
                DRAWN_FAMILIES[index], DRAWN_FAMILIES[other]
            );
        }
    }
}

#[test]
fn a_block_uses_its_active_model_metadata() {
    use ronie_core::{ChainBlock, ModelId};
    use ronie_ui::default::tile::block_glyph;

    let pack = tone(json!({
        "id": 0, "local": true, "title": "two amps", "format": "nam",
        "models": [
            {"id": 1, "name": "clean", "model_url": "file:///tmp/stash/a-1.nam",
             "gear": "amp", "gear_make": "Fender", "gear_model": "Twin Reverb"},
            {"id": 2, "name": "lead", "model_url": "file:///tmp/stash/b-1.nam",
             "gear": "amp", "gear_make": "Peavey", "gear_model": "6505+"}
        ]
    }));
    let block = ChainBlock::tone("block-1", pack, ModelId(2));
    assert_eq!(
        block_glyph(&block),
        GearGlyph::Family(GearFamily::HeadHighGain)
    );
    assert_eq!(block_glyph(&ChainBlock::insert("slot")), GearGlyph::Generic);
}
