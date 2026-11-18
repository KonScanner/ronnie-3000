//! Family-level gear glyphs: recognised captures show their gear family, the rest keep the
//! category glyph.

use eframe::egui;
use ronnie_core::gear::{GearCategory, GearFamily};
use ronnie_core::{ChainBlock, ModelId, Tone};
use ronnie_ui::default::skins::auto_skin_for;
use ronnie_ui::default::tile::{GearGlyph, block_glyph, draw_gear, tone_glyph};
use serde_json::json;

mod common;

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
            "id": 7, "name": "lead", "model_url": common::file_url("/tmp/stash/abc-1.nam"),
            "gear": "amp", "gear_make": "EVH", "gear_model": "5150 III"
        }]
    }));
    assert_eq!(
        tone_glyph(&local, local.models.first()),
        GearGlyph::Family(GearFamily::HeadHighGain)
    );
}

/// Pairs of families that deliberately share one drawing arm. They are the only glyphs that
/// may look identical; every other pair in [`GearFamily::DRAWN`] must paint differently.
const SHARED_PAINT_ARMS: [(GearFamily, GearFamily); 1] =
    [(GearFamily::Cab412Vintage, GearFamily::Cab412Modern)];

fn shares_a_paint_arm(left: GearFamily, right: GearFamily) -> bool {
    SHARED_PAINT_ARMS.iter().any(|(first, second)| {
        (*first == left && *second == right) || (*first == right && *second == left)
    })
}

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
fn every_drawn_family_glyph_paints_and_only_the_stated_arm_matches() {
    let drawings: Vec<String> = GearFamily::DRAWN
        .iter()
        .map(|family| painted(GearGlyph::Family(*family)))
        .collect();
    for (index, drawing) in drawings.iter().enumerate() {
        for (other, other_drawing) in drawings.iter().enumerate().skip(index + 1) {
            let (left, right) = (GearFamily::DRAWN[index], GearFamily::DRAWN[other]);
            if shares_a_paint_arm(left, right) {
                assert_eq!(
                    drawing, other_drawing,
                    "{left:?} and {right:?} must keep their shared drawing arm"
                );
            } else {
                assert_ne!(
                    drawing, other_drawing,
                    "{left:?} and {right:?} look identical"
                );
            }
        }
    }
}

#[test]
fn the_two_412_cabs_share_their_drawing() {
    assert_eq!(
        painted(GearGlyph::Family(GearFamily::Cab412Vintage)),
        painted(GearGlyph::Family(GearFamily::Cab412Modern)),
        "Cab412Vintage draws in the arm that Cab412Modern uses"
    );
}

#[test]
fn a_block_uses_its_active_model_metadata() {
    let pack = tone(json!({
        "id": 0, "local": true, "title": "two amps", "format": "nam",
        "models": [
            {"id": 1, "name": "clean", "model_url": common::file_url("/tmp/stash/a-1.nam"),
             "gear": "amp", "gear_make": "Fender", "gear_model": "Twin Reverb"},
            {"id": 2, "name": "lead", "model_url": common::file_url("/tmp/stash/b-1.nam"),
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

/// A block per tone of the catalog fixture, each with the tone's first model active.
fn fixture_blocks() -> Vec<ChainBlock> {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/ui-catalog.json"))
            .expect("catalog fixture");
    fixture["tones"]
        .as_array()
        .expect("catalog tones")
        .iter()
        .map(|row| {
            let tone = tone(row.clone());
            let model = tone.models.first().map_or(ModelId(0), |model| model.id);
            ChainBlock::tone(format!("fixture-{}", tone.id.0), tone, model)
        })
        .collect()
}

/// A block of a capture that no keyword recognises, so it keeps its category's default family.
fn unrecognised_block(gear: &str, title: &str) -> ChainBlock {
    let tone = tone(json!({
        "id": 900, "title": title, "format": "nam", "gear": gear,
        "models": [{"id": 9001, "name": "one"}]
    }));
    ChainBlock::tone(format!("unrecognised-{gear}"), tone, ModelId(9001))
}

/// The category a category-level glyph stands for.
fn glyph_category(glyph: GearGlyph) -> GearCategory {
    match glyph {
        GearGlyph::FullRig => GearCategory::FullRig,
        GearGlyph::Amp => GearCategory::Amp,
        GearGlyph::Cabinet => GearCategory::Cab,
        GearGlyph::Pedal => GearCategory::Pedal,
        GearGlyph::Outboard => GearCategory::Outboard,
        GearGlyph::Space => GearCategory::Space,
        GearGlyph::Family(family) => family.category(),
        GearGlyph::Generic => GearCategory::Unknown,
    }
}

#[test]
fn a_blocks_glyph_and_its_automatic_skin_name_the_same_gear() {
    use ronnie_core::gear::block_identity;

    let mut blocks = fixture_blocks();
    blocks.push(unrecognised_block("amp", "bedroom capture"));
    blocks.push(unrecognised_block("pedal", "mystery drive"));
    blocks.push(unrecognised_block("cab", "my-ir-mix"));

    for block in &blocks {
        let (_, identity) = block_identity(block).expect("a block with a tone");
        let glyph = block_glyph(block);
        let skin = auto_skin_for(block);
        assert_eq!(
            skin.map(|skin| skin.id),
            identity.family.skin_id(),
            "{:?} has the wrong automatic skin",
            identity.family
        );
        match glyph {
            GearGlyph::Family(family) => {
                assert_eq!(family, identity.family);
                let skin = skin.expect("a drawn family has a skin");
                assert_eq!(family.skin_id(), Some(skin.id));
                assert_eq!(skin.category, family.category());
            }
            category_glyph => {
                assert!(
                    !GearFamily::DRAWN.contains(&identity.family),
                    "{:?} draws a family glyph but is not in DRAWN",
                    identity.family
                );
                if let Some(skin) = skin {
                    assert_eq!(
                        glyph_category(category_glyph),
                        skin.category,
                        "{:?} keeps the wrong category glyph",
                        identity.family
                    );
                }
            }
        }
    }
}
