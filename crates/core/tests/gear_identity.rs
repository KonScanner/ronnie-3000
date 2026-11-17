//! Gear identification: which family (and so which skin and icon) a block shows.

use ronie_core::Tone;
use ronie_core::gear::{GearCategory, GearFamily, GearHints, MatchSource, ToneType, identify};
use serde_json::json;

fn catalog(gear: &str, title: &str, makes: &[&str]) -> GearHints {
    GearHints {
        gear: Some(gear.to_owned()),
        title: title.to_owned(),
        makes: makes.iter().map(|make| (*make).to_owned()).collect(),
        ..GearHints::default()
    }
}

#[test]
fn catalog_vox_ac30_is_a_chime_combo() {
    let id = identify(&catalog(
        "amp",
        "'02 Vox AC30/6 Top Boost",
        &["Vox", "AC30/6 Top Boost"],
    ));
    assert_eq!(id.family, GearFamily::ComboChime);
    assert_eq!(id.category, GearCategory::Amp);
    assert_eq!(id.source, MatchSource::Catalog);
}

#[test]
fn catalog_roland_jc40_is_a_clean_combo() {
    let id = identify(&catalog("amp", "Roland JC-40", &["Roland", "JC-40"]));
    assert_eq!(id.family, GearFamily::ComboClean);
}

#[test]
fn catalog_greenback_4x12_is_a_vintage_british_cab() {
    let id = identify(&catalog(
        "cab",
        "UK Greenback - Marshall 1960TV 4x12 M201",
        &["Marshall", "1960TV"],
    ));
    assert_eq!(id.family, GearFamily::Cab412Vintage);
    assert_eq!(id.category, GearCategory::Cab);
}

#[test]
fn catalog_dual_rectifier_is_a_modern_us_head() {
    let id = identify(&catalog(
        "amp",
        "Dual Rectifier Modern",
        &["Mesa/Boogie", "Dual Rectifier"],
    ));
    assert_eq!(id.family, GearFamily::HeadModernUs);
}

#[test]
fn nam_metadata_wins_over_the_title() {
    let hints = GearHints {
        title: "Plexi crunch".to_owned(),
        nam_make: Some("EVH".to_owned()),
        nam_model: Some("5150 III".to_owned()),
        nam_gear_type: Some("amp".to_owned()),
        ..GearHints::default()
    };
    let id = identify(&hints);
    assert_eq!(id.family, GearFamily::HeadHighGain);
    assert_eq!(id.source, MatchSource::NamMetadata);
}

#[test]
fn title_alone_identifies_the_family_and_its_category() {
    let hints = GearHints {
        title: "Peavey 6505+ lead channel".to_owned(),
        ..GearHints::default()
    };
    let id = identify(&hints);
    assert_eq!(id.family, GearFamily::HeadHighGain);
    assert_eq!(id.category, GearCategory::Amp);
    assert_eq!(id.source, MatchSource::Title);
}

#[test]
fn the_category_limits_which_families_can_match() {
    // A pedal whose title mentions an amp stays a pedal.
    let id = identify(&catalog("pedal", "5150 boost into the front", &[]));
    assert_eq!(id.category, GearCategory::Pedal);
    assert_eq!(id.family, GearFamily::Boost);
}

#[test]
fn keywords_match_whole_words_only() {
    // "Bassman" must not match the "bass" keyword.
    let id = identify(&catalog(
        "amp",
        "Fender Bassman 5F6-A",
        &["Fender", "Bassman"],
    ));
    assert_eq!(id.family, GearFamily::ComboClean);
}

#[test]
fn unmatched_amps_use_the_default_for_their_tone_type() {
    let with = |tone: Option<&str>| GearHints {
        gear: Some("amp".to_owned()),
        title: "bedroom capture".to_owned(),
        tone_type: tone.map(str::to_owned),
        ..GearHints::default()
    };
    assert_eq!(
        identify(&with(Some("clean"))).family,
        GearFamily::AmpDefaultClean
    );
    assert_eq!(
        identify(&with(Some("crunch"))).family,
        GearFamily::AmpDefaultCrunch
    );
    assert_eq!(
        identify(&with(Some("overdrive"))).family,
        GearFamily::AmpDefaultCrunch
    );
    assert_eq!(
        identify(&with(Some("hi_gain"))).family,
        GearFamily::AmpDefaultHiGain
    );
    assert_eq!(identify(&with(None)).family, GearFamily::AmpDefaultHiGain);
    assert_eq!(identify(&with(Some("clean"))).source, MatchSource::Category);
}

#[test]
fn unmatched_pedals_use_the_default_for_their_tone_type() {
    let with = |tone: Option<&str>| GearHints {
        gear: Some("pedal".to_owned()),
        title: "mystery drive".to_owned(),
        tone_type: tone.map(str::to_owned),
        ..GearHints::default()
    };
    assert_eq!(
        identify(&with(Some("overdrive"))).family,
        GearFamily::PedalDefaultDark
    );
    assert_eq!(
        identify(&with(Some("fuzz"))).family,
        GearFamily::PedalDefaultDark
    );
    assert_eq!(
        identify(&with(Some("clean"))).family,
        GearFamily::PedalDefaultLight
    );
    assert_eq!(identify(&with(None)).family, GearFamily::PedalDefaultLight);
}

#[test]
fn cabs_take_their_layout_from_the_name() {
    let cab = |title: &str| identify(&catalog("cab", title, &[])).family;
    assert_eq!(cab("V30 4x12 SM57"), GearFamily::Cab412Modern);
    assert_eq!(cab("plain 4x12 mix"), GearFamily::Cab412Modern);
    assert_eq!(cab("my 2x12 mix"), GearFamily::Cab212);
    assert_eq!(cab("Deluxe 1x12"), GearFamily::Cab112);
    assert_eq!(cab("SVT 8x10"), GearFamily::Cab810);
    assert_eq!(cab("my-ir-mix"), GearFamily::CabDefault);
}

#[test]
fn rigs_outboard_and_spaces_have_their_own_families() {
    assert_eq!(
        identify(&catalog("full-rig", "Recto into V30", &[])).family,
        GearFamily::FullRig
    );
    assert_eq!(
        identify(&catalog("outboard", "AMS Neve 88R", &["AMS Neve", "88R"])).family,
        GearFamily::Outboard
    );
    assert_eq!(
        identify(&catalog("space", "Plate 2.1 s", &[])).family,
        GearFamily::Space
    );
}

#[test]
fn nothing_known_is_unknown() {
    let id = identify(&GearHints::default());
    assert_eq!(id.family, GearFamily::Unknown);
    assert_eq!(id.category, GearCategory::Unknown);
    assert_eq!(id.source, MatchSource::None);
}

#[test]
fn families_name_their_skin_and_icon() {
    assert_eq!(GearFamily::ComboChime.skin_id(), Some("chime"));
    assert_eq!(GearFamily::Cab412Vintage.skin_id(), Some("cab-412-vintage"));
    assert_eq!(GearFamily::OdGreen.skin_id(), Some("od-green"));
    assert_eq!(GearFamily::Unknown.skin_id(), None);
    assert_eq!(GearFamily::ComboChime.icon_id(), "combo-chime");
    assert_eq!(GearFamily::AmpDefaultCrunch.icon_id(), "head-generic");
    assert_eq!(GearFamily::Unknown.icon_id(), "unknown");
}

#[test]
fn tone_types_parse_nam_spellings() {
    assert_eq!(ToneType::from_nam("hi_gain"), ToneType::HiGain);
    assert_eq!(ToneType::from_nam(" Hi-Gain "), ToneType::HiGain);
    assert_eq!(ToneType::from_nam("overdrive"), ToneType::Crunch);
    assert_eq!(ToneType::from_nam("fuzz"), ToneType::Fuzz);
    assert_eq!(ToneType::from_nam("whatever"), ToneType::Unknown);
}

#[test]
fn hints_come_from_a_catalog_tone() {
    let tone = Tone::from_value(json!({
        "id": 104,
        "title": "Big Muff Triangle",
        "format": "nam",
        "gear": "pedal",
        "models": [{"id": 1, "name": "Sustain 7", "model_url": "https://fixtures.local/1.nam"}],
        "makes": [{"id": 17, "name": "Electro-Harmonix"}, " Big Muff "]
    }))
    .expect("tone parses");
    let hints = GearHints::from_tone(&tone, None);
    assert_eq!(hints.gear.as_deref(), Some("pedal"));
    assert_eq!(
        hints.makes,
        vec!["Electro-Harmonix".to_owned(), "Big Muff".to_owned()]
    );
    assert_eq!(identify(&hints).family, GearFamily::Fuzz);
}

#[test]
fn hints_take_nam_metadata_from_the_active_model() {
    let tone = Tone::from_value(json!({
        "id": 0,
        "local": true,
        "title": "my captures",
        "format": "nam",
        "models": [{
            "id": 7,
            "name": "lead",
            "model_url": "file:///tmp/stash/abc-1.nam",
            "gear": "amp",
            "gear_make": "Marshall",
            "gear_model": "JCM800 2203",
            "tone_type": "crunch"
        }]
    }))
    .expect("tone parses");
    let hints = GearHints::from_tone(&tone, tone.models.first());
    assert_eq!(hints.nam_make.as_deref(), Some("Marshall"));
    assert_eq!(hints.tone_type.as_deref(), Some("crunch"));
    let id = identify(&hints);
    assert_eq!(id.family, GearFamily::HeadBritish);
    assert_eq!(id.source, MatchSource::NamMetadata);
}
