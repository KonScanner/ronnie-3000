//! Chain layouts, tile states, context actions, and local file loading tests.

use std::sync::Arc;

use ronie_core::{ChainSide, params::ParameterId};
use ronie_ui::{
    backend::{MockBackend, UiBackend},
    default::{
        ChainGallery, ChromeController, TileVisualState,
        tile_menu::{self, TileMenuItem},
    },
    testbed::{UiScenario, load_scenarios},
};

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn gallery(id: &str) -> (ChainGallery, Arc<MockBackend>) {
    let scenario = scenario(id);
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    (ChainGallery::new(backend.clone()), backend)
}

fn parameter(backend: &MockBackend, id: &'static str) -> f32 {
    backend.parameter(ParameterId(id)).expect("parameter").0
}

#[test]
fn empty_mono_stereo_and_branched_layouts_keep_expected_slot_counts_and_geometry() {
    let (mono, _backend) = gallery("main-empty");
    assert_eq!(mono.layout().tile_size, 224);
    assert_eq!(mono.layout().tile_gap, 24);
    assert_eq!(mono.layout().edge_fade, 32);
    assert!(mono.lane_ids(ChainSide::Left).len() >= 5);

    let (stereo, _backend) = gallery("main-stereo");
    assert_eq!(stereo.layout().tile_size, 160);
    assert_eq!(stereo.layout().lane_gap, 24);
    assert_eq!(stereo.lane_ids(ChainSide::Left).len(), 3);
    assert_eq!(stereo.lane_ids(ChainSide::Right).len(), 3);

    let (branched, _backend) = gallery("main-stereo-branched");
    assert!(branched.layout().branch_indent > 0);
}

#[test]
fn tile_loading_failure_and_drop_states_are_explicit() {
    let (loading, _backend) = gallery("load-tile-loading");
    assert_eq!(loading.tile_state("blk-2"), TileVisualState::Loading);
    let (failed, _backend) = gallery("load-detail-failed");
    assert_eq!(failed.tile_state("blk-2"), TileVisualState::Failed);
    let (mut ready, _backend) = gallery("main-stereo");
    assert_eq!(ready.tile_state("blk-l1"), TileVisualState::Ready);
    assert_eq!(
        ready.with_drop_armed("blk-l1").tile_state("blk-l1"),
        TileVisualState::DropArmed
    );
}

#[test]
fn power_copy_paste_swap_remove_and_branch_actions_mutate_once() {
    let (mut gallery, backend) = gallery("main-stereo");
    let initial = backend.chain_state(0).revision;
    assert!(!gallery.can_paste());
    gallery.set_enabled("blk-l1", false).expect("power");
    assert_eq!(backend.chain_state(0).revision, initial + 1);
    gallery.copy("blk-l1").expect("copy");
    assert!(gallery.can_paste());
    gallery.paste(ChainSide::Right, 1).expect("paste");
    assert_eq!(backend.chain_state(0).revision, initial + 3);
    gallery.remove("blk-l1").expect("remove");
    assert_eq!(backend.chain_state(0).revision, initial + 4);
    gallery
        .set_branch(ChainSide::Left, "blk-l2")
        .expect("branch");
    gallery.clear_branch().expect("clear branch");
    assert_eq!(backend.chain_state(0).revision, initial + 6);
}

#[test]
fn right_lane_power_updates_only_the_right_block() {
    let (mut gallery, backend) = gallery("main-stereo");
    gallery.set_enabled("blk-r1", false).expect("right power");
    let chain = backend.chain_state(0).chain;
    assert!(
        !chain
            .right
            .iter()
            .find(|block| block.id.0 == "blk-r1")
            .expect("right block")
            .params
            .enabled
    );
    assert!(
        chain
            .left
            .iter()
            .find(|block| block.id.0 == "blk-l1")
            .expect("left block")
            .params
            .enabled
    );
}

#[test]
fn linked_pans_mirror_and_relink_snaps_to_the_left_anchor() {
    let (mut gallery, backend) = gallery("main-stereo");
    gallery.set_pan(ChainSide::Left, 0.25).expect("linked pan");
    assert_eq!(parameter(&backend, "chainPanLeft"), 0.25);
    assert_eq!(parameter(&backend, "chainPanRight"), 0.75);
    gallery.set_pan_linked(false).expect("unlink");
    gallery
        .set_pan(ChainSide::Right, 0.9)
        .expect("independent pan");
    assert_eq!(parameter(&backend, "chainPanLeft"), 0.25);
    gallery.set_pan_linked(true).expect("relink");
    assert_eq!(parameter(&backend, "chainPanRight"), 0.75);
}

#[test]
fn solo_is_exclusive_and_polarity_toggles_independently() {
    let (mut gallery, backend) = gallery("main-stereo");
    gallery.toggle_solo(ChainSide::Left).expect("left solo");
    assert_eq!(parameter(&backend, "chainSoloLeft"), 1.0);
    gallery.toggle_solo(ChainSide::Right).expect("right solo");
    assert_eq!(parameter(&backend, "chainSoloLeft"), 0.0);
    assert_eq!(parameter(&backend, "chainSoloRight"), 1.0);
    gallery
        .toggle_invert(ChainSide::Left)
        .expect("left polarity");
    assert_eq!(parameter(&backend, "chainInvertLeft"), 1.0);
    assert_eq!(parameter(&backend, "chainInvertRight"), 0.0);
}

#[test]
fn swapping_lanes_moves_the_branch_and_polarity_with_them() {
    let (mut gallery, backend) = gallery("main-stereo");
    let left = gallery.lane_ids(ChainSide::Left);
    let right = gallery.lane_ids(ChainSide::Right);
    gallery
        .set_branch(ChainSide::Right, "blk-r2")
        .expect("right branch");
    gallery
        .toggle_invert(ChainSide::Left)
        .expect("left polarity");
    gallery.swap_chains().expect("swap");
    assert_eq!(gallery.lane_ids(ChainSide::Left), right);
    assert_eq!(gallery.lane_ids(ChainSide::Right), left);
    let chain = backend.chain_state(0).chain;
    assert_eq!(chain.branch.as_ref().expect("branch").side, ChainSide::Left);
    assert_eq!(parameter(&backend, "chainInvertLeft"), 0.0);
    assert_eq!(parameter(&backend, "chainInvertRight"), 1.0);
}

#[test]
fn retry_enters_loading_once_and_rejects_a_duplicate_retry() {
    let (mut gallery, backend) = gallery("load-detail-failed");
    let initial = backend.chain_state(0).revision;
    gallery.retry("blk-2").expect("retry");
    assert_eq!(gallery.tile_state("blk-2"), TileVisualState::Loading);
    assert_eq!(backend.chain_state(0).revision, initial + 1);
    assert!(gallery.retry("blk-2").is_err());
    assert_eq!(backend.chain_state(0).revision, initial + 1);
}

#[test]
fn context_menu_rows_match_tone_and_insert_capabilities() {
    assert_eq!(
        tile_menu::items(false, false),
        vec![
            TileMenuItem::Copy,
            TileMenuItem::LoadFile,
            TileMenuItem::LoadFolder,
            TileMenuItem::Remove
        ]
    );
    assert_eq!(
        tile_menu::items(true, true),
        vec![
            TileMenuItem::Paste,
            TileMenuItem::LoadFile,
            TileMenuItem::LoadFolder
        ]
    );
    assert_eq!(
        tile_menu::items(true, false),
        vec![
            TileMenuItem::Paste,
            TileMenuItem::LoadFile,
            TileMenuItem::LoadFolder
        ]
    );
}

#[test]
fn chrome_owns_a_persistent_gallery() {
    let scenario = scenario("main-empty");
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let chrome = ChromeController::new(backend, Vec::new(), false, true);
    assert!(chrome.gallery().lane_ids(ChainSide::Left).len() >= 5);
}

#[test]
fn local_file_drop_targets_one_insert_and_loads_real_bytes() {
    let (mut gallery, backend) = gallery("main-empty");
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("identity.wav");
    let samples = [0_i16, 1000, -1000, 0];
    let mut bytes = Vec::from(b"RIFF".as_slice());
    bytes.extend_from_slice(&(40_u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&path, bytes).expect("WAV");
    let insert = gallery
        .lane_ids(ChainSide::Left)
        .last()
        .expect("insert")
        .clone();
    gallery.drop_file(&insert, &path).expect("local drop");
    let chain = backend.chain_state(0).chain;
    assert!(chain.left.iter().any(|block| block.tone.is_some()));
}

#[test]
fn tile_activation_targets_detail_or_tone_selection() {
    use ronie_ui::default::GallerySelection;

    let (mut empty, _backend) = gallery("main-empty");
    let insert = empty
        .lane_ids(ChainSide::Left)
        .last()
        .expect("insert")
        .clone();
    empty.activate(&insert);
    assert_eq!(
        empty.selection(),
        &GallerySelection::SelectTone {
            side: ChainSide::Left,
            target_block_id: ronie_core::BlockId(insert),
        }
    );

    let (mut stereo, _backend) = gallery("main-stereo");
    stereo.activate("blk-l1");
    assert_eq!(
        stereo.selection(),
        &GallerySelection::BlockDetail(ronie_core::BlockId("blk-l1".to_owned()))
    );
}

#[test]
fn lane_geometry_targets_insert_slots_and_routes_add_tiles() {
    use ronie_ui::default::lane::{Routing, gap_center_x, lane_width, routing_for, slot_at};

    assert_eq!(lane_width(3, 160), 528);
    assert_eq!(gap_center_x(1, 160), 172.0);
    assert_eq!(routing_for(0, 1), Routing::None);
    assert_eq!(routing_for(0, 3), Routing::Right);
    assert_eq!(routing_for(1, 3), Routing::Both);
    assert_eq!(routing_for(2, 3), Routing::Left);
    assert_eq!(slot_at(0.0, 3, 160), 0);
    assert_eq!(slot_at(100.0, 3, 160), 1);
    assert_eq!(slot_at(500.0, 3, 160), 3);
    assert_eq!(slot_at(10.0, 0, 160), 0);
}

#[test]
fn the_tile_gain_shows_decibels_from_minus_to_plus_twenty_four() {
    use ronie_ui::default::tile::{format_gain_db, gain_db};

    assert_eq!(gain_db(0.0), -24.0);
    assert_eq!(gain_db(0.5), 0.0);
    assert_eq!(gain_db(1.0), 24.0);
    assert_eq!(format_gain_db(0.5), "0.0 dB");
    assert_eq!(format_gain_db(0.625), "+6.0 dB");
    assert_eq!(format_gain_db(0.4479), "-2.5 dB");
    assert_eq!(format_gain_db(2.0), "+24.0 dB");
    assert_eq!(
        format_gain_db(0.5005),
        "0.0 dB",
        "a rounded zero has no sign"
    );
}

#[test]
fn a_tile_gain_gesture_writes_only_the_input_gain_of_its_tone() {
    use ronie_ui::backend::{GesturePhase, UiAction};

    let (mut gallery, backend) = gallery("main-stereo");
    let before = backend
        .chain_state(0)
        .chain
        .block(&ronie_core::BlockId("blk-l1".to_owned()))
        .expect("tone")
        .params
        .clone();
    gallery
        .set_input_gain("blk-l1", 0.75, GesturePhase::Begin)
        .expect("gain");
    gallery
        .set_input_gain("blk-l1", 0.8, GesturePhase::End)
        .expect("gain");
    let after = backend
        .chain_state(0)
        .chain
        .block(&ronie_core::BlockId("blk-l1".to_owned()))
        .expect("tone")
        .params
        .clone();
    assert_eq!(after.input_gain, 0.8);
    assert_eq!(
        ronie_core::BlockParams {
            input_gain: before.input_gain,
            ..after
        },
        before
    );
    assert!(backend.actions().iter().any(|action| matches!(
        action,
        UiAction::SetBlockParamsGesture {
            gesture: GesturePhase::Begin,
            ..
        }
    )));
    let open_slot = gallery
        .lane_ids(ChainSide::Left)
        .last()
        .cloned()
        .expect("slot");
    assert!(
        gallery
            .set_input_gain(&open_slot, 0.6, GesturePhase::End)
            .is_err(),
        "an open slot has no gain"
    );
}

#[test]
fn meter_and_gear_fallbacks_cover_extremes_and_all_tone_families() {
    use ronie_ui::default::tile::{GearGlyph, gear_glyph, meter_fraction};

    assert_eq!(meter_fraction(-60.0), 0.0);
    assert_eq!(meter_fraction(-30.0), 0.5);
    assert_eq!(meter_fraction(0.0), 1.0);
    assert_eq!(meter_fraction(12.0), 1.0);
    assert_eq!(meter_fraction(f32::NAN), 0.0);
    assert_eq!(gear_glyph("amp"), GearGlyph::Amp);
    assert_eq!(gear_glyph("cab"), GearGlyph::Cabinet);
    assert_eq!(gear_glyph("pedal"), GearGlyph::Pedal);
    assert_eq!(gear_glyph("outboard"), GearGlyph::Outboard);
    assert_eq!(gear_glyph("spaces"), GearGlyph::Space);
    assert_eq!(gear_glyph("unknown"), GearGlyph::Generic);
}

/// The right edge of the chain view at the design width: 1024 less 58 and 24 points.
const CHAIN_RIGHT: f32 = 942.0;
/// The width of the edge fade.
const EDGE_FADE: f32 = 32.0;

fn main_driver(id: &str) -> (ronie_ui::driver::UiDriver, Arc<MockBackend>) {
    let backend = Arc::new(MockBackend::from_scenario(&scenario(id)));
    let chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    let mut driver = ronie_ui::driver::UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    (driver, backend)
}

fn control_rect(driver: &ronie_ui::driver::UiDriver, label: &str) -> Option<eframe::egui::Rect> {
    driver
        .controls()
        .iter()
        .find(|control| control.label == label)
        .map(|control| control.rect)
}

fn first_tile_title(backend: &MockBackend) -> String {
    backend.chain_state(0).chain.left[0]
        .tone
        .as_ref()
        .expect("tone")
        .title
        .clone()
}

#[test]
fn a_vertical_wheel_over_the_chain_scrolls_it_sideways() {
    use eframe::egui;

    let (mut driver, backend) = main_driver("main-mono");
    let title = first_tile_title(&backend);
    let before = control_rect(&driver, &title).expect("first tile");
    let over_chain = before.center();
    driver.play(vec![
        egui::Event::PointerMoved(over_chain),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    for _ in 0..20 {
        driver.frame();
    }
    let after = control_rect(&driver, &title).expect("first tile");
    assert!(
        after.min.x < before.min.x,
        "the tile must move left: {before:?} then {after:?}"
    );
    assert_eq!(
        after.min.y, before.min.y,
        "the chain does not scroll up or down"
    );
}

#[test]
fn the_edge_arrows_show_where_tiles_continue_and_scroll_one_tile() {
    let (mut driver, backend) = main_driver("main-mono");
    assert!(driver.is_visible("Scroll chain right"));
    assert!(!driver.is_visible("Scroll chain left"));
    let title = first_tile_title(&backend);
    let before = control_rect(&driver, &title).expect("first tile");

    let _ = driver.click("Scroll chain right").expect("right arrow");
    driver.settle();
    assert!(driver.is_visible("Scroll chain left"));
    let after = control_rect(&driver, &title).expect("first tile");
    assert!(after.max.x < before.max.x, "{before:?} then {after:?}");

    let _ = driver.click("Scroll chain left").expect("left arrow");
    driver.settle();
    assert!(!driver.is_visible("Scroll chain left"));
    assert_eq!(control_rect(&driver, &title), Some(before));

    let (stereo, _backend) = main_driver("main-stereo");
    assert!(
        !stereo.is_visible("Scroll chain right"),
        "a short chain has no arrow"
    );
}

#[test]
fn a_loaded_tone_scrolls_fully_into_view_where_it_lands() {
    let tone = scenario_backend_tone();
    let (mut driver, backend) = main_driver("main-empty");
    for _ in 0..4 {
        if driver.is_visible("Scroll chain right") {
            let _ = driver.click("Scroll chain right").expect("right arrow");
            driver.settle();
        }
    }
    // The last open slot is the one at the far right, and the tone lands in the first slot.
    let last = driver
        .controls()
        .iter()
        .filter(|control| control.label == "Add Tone")
        .max_by(|a, b| a.rect.min.x.total_cmp(&b.rect.min.x))
        .map(|control| control.rect.center())
        .expect("an open slot");
    let _ = driver.click_at(last);
    driver.settle();
    driver.chrome_mut().pick_tone(tone.clone());
    for _ in 0..4 {
        driver.settle();
    }

    assert_eq!(first_tile_title(&backend), tone.title);
    let landed = control_rect(&driver, &tone.title).expect("the new tile has an area");
    assert_eq!(landed.width(), 224.0, "no edge cuts the tile: {landed:?}");
    assert!(landed.min.x >= 58.0 + 24.0 + EDGE_FADE, "{landed:?}");
    assert!(landed.max.x <= CHAIN_RIGHT - EDGE_FADE, "{landed:?}");
}

#[test]
fn a_moved_tone_scrolls_fully_into_view_where_it_lands() {
    use eframe::egui;
    use ronie_core::BlockId;
    use ronie_ui::backend::UiAction;

    // A long left lane, and a right lane with one tone and two open slots.
    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-stereo")));
    backend
        .dispatch(UiAction::RemoveBlock(BlockId("blk-r2".to_owned())))
        .expect("remove");
    for _ in 0..4 {
        backend
            .dispatch(UiAction::DuplicateBlockToLane {
                block_id: BlockId("blk-l1".to_owned()),
                target_side: ChainSide::Left,
                target_index: 0,
            })
            .expect("duplicate");
    }
    let chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    let mut driver = ronie_ui::driver::UiDriver::new(chrome, (*backend).clone());
    for _ in 0..8 {
        if driver.is_visible("Scroll chain right") {
            let _ = driver.click("Scroll chain right").expect("right arrow");
            driver.settle();
        }
    }
    let moved = BlockId("blk-l2".to_owned());
    let title = backend
        .chain_state(0)
        .chain
        .block(&moved)
        .and_then(|block| block.tone.as_ref())
        .expect("tone")
        .title
        .clone();
    let from = control_rect(&driver, &title)
        .expect("the last tone of the left lane")
        .center();

    // The drop is far to the right on the right lane. The lane closes the gap before it, so
    // the tone lands in the second slot, which is past the left edge of the view.
    let tile_and_gap = 160.0 + 24.0;
    let to = egui::pos2(CHAIN_RIGHT - 80.0, from.y + tile_and_gap);
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    driver.play(vec![egui::Event::PointerMoved(from)]);
    driver.play(vec![button(from, true)]);
    // A real pointer crosses each point in between, over many frames.
    for step in 1..=8 {
        driver.play(vec![egui::Event::PointerMoved(
            from + (to - from) * step as f32 / 8.0,
        )]);
    }
    driver.play(vec![button(to, false)]);
    for _ in 0..4 {
        driver.settle();
    }

    let right: Vec<BlockId> = backend
        .chain_state(0)
        .chain
        .right
        .iter()
        .map(|block| block.id.clone())
        .collect();
    assert_eq!(right.get(1), Some(&moved), "the gap closed: {right:?}");
    let landed = control_rect(&driver, &title).expect("the moved tile has an area");
    assert_eq!(landed.width(), 160.0, "no edge cuts the tile: {landed:?}");
    // The pan rail takes 57 points at the left of a stereo chain.
    assert!(landed.min.x >= 58.0 + 24.0 + 57.0 + EDGE_FADE, "{landed:?}");
    assert!(landed.max.x <= CHAIN_RIGHT - EDGE_FADE, "{landed:?}");
}

fn scenario_backend_tone() -> ronie_core::Tone {
    MockBackend::from_scenario(&scenario("main-mono"))
        .chain_state(0)
        .chain
        .left[1]
        .tone
        .clone()
        .expect("tone")
}
