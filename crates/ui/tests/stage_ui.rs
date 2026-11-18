//! The Stage layout: light header with the chain as gear icons, stage, light global panel
//! and black utility bar.

use std::sync::Arc;

use ronie_ui::{
    backend::{MockBackend, UiBackend},
    default::{ChromeController, Placement},
    driver::UiDriver,
    shell::ViewMode,
    testbed::{UiScenario, load_scenarios},
    theme::Look,
};

mod common;

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn stage_driver(id: &str) -> (UiDriver, Arc<MockBackend>) {
    let backend = Arc::new(MockBackend::from_scenario(&scenario(id)));
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    (driver, backend)
}

#[test]
fn reference_is_the_default_view_mode() {
    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-mono")));
    let chrome = ChromeController::new(backend, Vec::new(), false, true);
    assert_eq!(chrome.view_mode(), ViewMode::ReferenceDefault);
}

#[test]
fn stage_mode_installs_the_light_look() {
    let (driver, _) = stage_driver("main-mono");
    assert_eq!(Look::get(driver.context()), Look::stage());
}

#[test]
fn settings_still_open_in_stage_mode() {
    let (mut driver, _) = stage_driver("main-mono");
    driver.chrome_mut().open_settings();
    driver.settle();
    assert!(driver.chrome_mut().settings_mut().is_some());
    assert!(
        !driver.is_visible("Browse presets"),
        "settings replace the whole window"
    );
}

#[test]
fn the_global_panel_hides_while_the_browser_is_open() {
    let (mut driver, _) = stage_driver("main-mono");
    assert!(
        driver.label_starting_with("Gate").is_some(),
        "the global panel shows"
    );
    let _ = driver.click("Add a block").expect("the + button");
    driver.settle();
    let _ = driver.click("OPEN THE TONE BROWSER").expect("browser link");
    driver.settle();
    assert!(
        driver.chrome_mut().browser_mut().is_some(),
        "the tone browser opened"
    );
    assert!(
        driver.label_starting_with("Gate").is_none(),
        "the global panel hides under the browser"
    );
}

// ---------------------------------------------------------------- Task 3: header

use ronie_core::BlockId;
use ronie_ui::default::GallerySelection;

/// The scenarios document with one scenario edited, loaded as a driver in Stage mode.
fn edited_driver(
    id: &str,
    edit: impl FnOnce(&mut serde_json::Value),
) -> (UiDriver, Arc<MockBackend>) {
    let mut doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/ui-scenarios.json")).expect("json");
    let scenario = doc["scenarios"]
        .as_array_mut()
        .expect("scenarios")
        .iter_mut()
        .find(|scenario| scenario["id"] == id)
        .expect("scenario exists");
    edit(&mut scenario["state"]["chain"]);
    let scenario = load_scenarios(&doc.to_string())
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists");
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    (driver, backend)
}

fn rect_of(driver: &UiDriver, label: &str) -> Option<eframe::egui::Rect> {
    driver
        .controls()
        .iter()
        .find(|control| control.label == label)
        .map(|control| control.rect)
}

const VOX: &str = "Chain: '02 Vox AC30/6 Top Boost";

#[test]
fn strip_shows_one_button_per_block() {
    let (driver, _) = stage_driver("main-mono");
    for title in [
        "Chain: Green Russian Big Muff + BD-2",
        VOX,
        "Chain: UK Greenback - Marshall 1960TV 4x12 M201",
        "Add a block",
    ] {
        assert!(driver.is_visible(title), "{title} is missing");
    }
    for label in [
        "Previous preset",
        "Next preset",
        "Browse presets",
        "Save preset",
        "New preset",
    ] {
        assert!(driver.is_visible(label), "{label} is missing");
    }
}

#[test]
fn clicking_a_block_opens_it() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver.click(VOX).expect("block button");
    driver.settle();
    assert!(
        matches!(
            driver.chrome().gallery().selection(),
            GallerySelection::BlockDetail(id) if *id == BlockId("blk-2".to_owned())
        ) || driver.chrome_mut().detail_mut().is_some(),
        "the block opened"
    );
}

const STRIP_MENU_ROWS: [&str; 4] = ["BYPASS", "REPLACE\u{2026}", "DUPLICATE", "REMOVE"];

fn strip_menu_open(driver: &UiDriver) -> bool {
    STRIP_MENU_ROWS.iter().any(|row| driver.is_visible(row))
}

#[test]
fn right_clicking_a_block_opens_its_menu() {
    let (mut driver, backend) = stage_driver("main-mono");
    let _ = driver.secondary_click(VOX).expect("block button");
    driver.settle();
    assert!(
        backend.chain_state(0).chain.left[1].params.enabled,
        "a right-click no longer bypasses"
    );
    for row in STRIP_MENU_ROWS {
        assert!(driver.is_visible(row), "{row} is missing");
    }
    assert!(
        driver.shows_text("'02 VOX AC30/6 TOP BOOST"),
        "the menu names the block"
    );
    assert!(
        driver.shows_text("double-click"),
        "BYPASS hints the shortcut"
    );
    let icon = rect_of(&driver, VOX).expect("the icon");
    let bypass = rect_of(&driver, "BYPASS").expect("the row");
    assert!(
        bypass.top() > icon.bottom(),
        "the menu opens under the icon"
    );
    assert!(
        (bypass.left()..=bypass.right()).contains(&icon.center().x),
        "the menu sits under the icon"
    );

    let _ = driver.press_key(eframe::egui::Key::Escape);
    assert!(!strip_menu_open(&driver), "Escape closes the menu");

    let _ = driver.secondary_click(VOX).expect("block button");
    driver.settle();
    assert!(strip_menu_open(&driver));
    let _ = driver.click_empty_space();
    driver.settle();
    assert!(!strip_menu_open(&driver), "a click outside closes the menu");
}

const CATEGORY_ROWS: [&str; 6] = ["PEDAL", "AMP", "AMP + CAB", "CAB", "OUTBOARD", "SPACE"];

fn open_menu(driver: &mut UiDriver) {
    let _ = driver.click("Add a block").expect("plus");
    driver.settle();
}

#[test]
fn plus_opens_the_add_menu() {
    let (mut driver, _) = stage_driver("main-mono");
    open_menu(&mut driver);
    for label in CATEGORY_ROWS
        .into_iter()
        .chain(["LOAD A FILE\u{2026}", "OPEN THE TONE BROWSER"])
    {
        assert!(driver.is_visible(label), "{label} is missing");
    }
    assert!(driver.chrome_mut().browser_mut().is_none());
    assert!(
        driver.chrome_mut().detail_mut().is_some(),
        "the stage still shows the amp page"
    );
}

#[test]
fn a_second_click_or_escape_closes_the_menu() {
    let (mut driver, _) = stage_driver("main-mono");
    open_menu(&mut driver);
    assert!(
        driver.is_visible("OPEN THE TONE BROWSER"),
        "the menu is open"
    );
    let _ = driver.click("Add a block").expect("plus");
    driver.settle();
    assert!(!driver.is_visible("PEDAL"), "the second click closes it");
    assert!(
        !driver.is_visible("OPEN THE TONE BROWSER"),
        "the second click closes it"
    );
    open_menu(&mut driver);
    assert!(
        driver.is_visible("OPEN THE TONE BROWSER"),
        "the menu is open"
    );
    let _ = driver.press_key(eframe::egui::Key::Escape);
    driver.settle();
    assert!(!driver.is_visible("PEDAL"), "escape closes it");
    assert!(
        !driver.is_visible("OPEN THE TONE BROWSER"),
        "escape closes it"
    );
}

#[test]
fn clicking_outside_closes_the_menu() {
    let (mut driver, _) = stage_driver("main-mono");
    open_menu(&mut driver);
    assert!(
        driver.is_visible("OPEN THE TONE BROWSER"),
        "the menu is open"
    );
    let _ = driver.click_empty_space();
    driver.settle();
    assert!(!driver.is_visible("PEDAL"));
    assert!(
        !driver.is_visible("OPEN THE TONE BROWSER"),
        "outside click closes it"
    );
}

fn lane_radio(driver: &UiDriver, label: &str) -> bool {
    driver
        .controls()
        .iter()
        .find(|control| control.label == label)
        .unwrap_or_else(|| panic!("{label} is missing"))
        .checked
}

#[test]
fn stereo_menu_offers_lane_a_and_b() {
    let (mut driver, _) = stage_driver("main-stereo");
    open_menu(&mut driver);
    assert!(lane_radio(&driver, "LANE A"), "lane A holds the selection");
    assert!(!lane_radio(&driver, "LANE B"));
    let _ = driver.click("Chain: Roland JC-40").expect("lane B block");
    driver.settle();
    let _ = driver.click("Add a block").expect("plus");
    driver.settle();
    if !driver.is_visible("LANE B") {
        open_menu(&mut driver);
    }
    assert!(lane_radio(&driver, "LANE B"), "lane B holds the selection");
    assert!(!lane_radio(&driver, "LANE A"));
}

#[test]
fn mono_menu_adds_to_the_end_of_the_chain() {
    let (mut driver, _) = stage_driver("main-mono");
    open_menu(&mut driver);
    assert!(driver.shows_text("placed by type"));
    assert!(!driver.is_visible("LANE A") && !driver.is_visible("LANE B"));
}

fn remembered_driver(tones: &[(i64, &str)]) -> UiDriver {
    use ronie_core::gear::GearCategory;
    use ronie_ui::{backend::UiAction, services::recent_tones::RecentTones};
    let mut recent = RecentTones::default();
    for (id, gear) in tones {
        recent.remember(
            ronie_core::Tone::from_value(serde_json::json!({
                "id": id, "title": format!("Tone {id}"), "format": "nam", "gear": gear,
                "models": [{"id": id * 10, "name": "m",
                    "model_url": format!("https://fixtures.invalid/m/{id}.nam")}]
            }))
            .expect("tone"),
        );
    }
    assert_eq!(recent.count(GearCategory::Amp), 2);
    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-mono")));
    backend
        .dispatch(UiAction::SetTextPreference {
            key: RecentTones::KEY.to_owned(),
            value: recent.to_text(),
        })
        .expect("seed");
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    driver
}

#[test]
fn category_rows_count_the_library() {
    let mut driver = remembered_driver(&[(1, "amp"), (2, "amp")]);
    open_menu(&mut driver);
    assert!(driver.shows_text("2"), "the AMP row counts two tones");
    assert!(
        driver.shows_text("\u{2014}"),
        "an empty category shows a dash"
    );
    let (mut empty, _) = stage_driver("main-mono");
    open_menu(&mut empty);
    assert!(!empty.shows_text("2"));
}

#[test]
fn mono_falls_back_to_lane_a() {
    let (mut driver, _) = stage_driver("main-stereo");
    let _ = driver.click("Chain: Roland JC-40").expect("lane B block");
    driver.settle();
    open_menu(&mut driver);
    assert!(lane_radio(&driver, "LANE B"));
    // A click on the switch would close the menu, so the chain changes behind it.
    let _ = driver.chrome_mut().set_stereo(false);
    driver.settle();
    assert!(driver.shows_text("placed by type"));
    let _ = driver.click("OPEN THE TONE BROWSER").expect("link");
    driver.settle();
    assert!(driver.chrome_mut().browser_mut().is_some());
    let target = driver
        .chrome_mut()
        .load_flow_mut()
        .pending_target()
        .cloned()
        .expect("a target");
    assert_eq!(target.side, ronie_core::ChainSide::Left);
}

#[test]
fn open_the_tone_browser_opens_it_for_the_lane() {
    let (mut driver, _) = stage_driver("main-stereo");
    let _ = driver.click("Chain: Roland JC-40").expect("lane B block");
    driver.settle();
    open_menu(&mut driver);
    let _ = driver.click("OPEN THE TONE BROWSER").expect("link");
    driver.settle();
    assert!(driver.chrome_mut().browser_mut().is_some());
    let target = driver
        .chrome_mut()
        .load_flow_mut()
        .pending_target()
        .cloned()
        .expect("a target");
    assert_eq!(target.side, ronie_core::ChainSide::Right);
    assert!(!driver.is_visible("PEDAL"), "the menu closed");
}

#[test]
fn a_long_chain_scrolls_instead_of_overlapping() {
    let (driver, _) = edited_driver("main-mono", |chain| {
        let template = chain["chain"][1].clone();
        let mut blocks = Vec::new();
        for index in 0..14 {
            let mut block = template.clone();
            block["blockId"] = format!("long-{index}").into();
            block["tone"]["title"] = format!("Block {index}").into();
            blocks.push(block);
        }
        blocks.push(chain["chain"][3].clone());
        chain["chain"] = blocks.into();
    });
    let strip = eframe::egui::Rect::from_min_max(
        eframe::egui::pos2(260.0, 0.0),
        eframe::egui::pos2(644.0, 64.0),
    );
    let shown: Vec<_> = (0..14)
        .filter_map(|index| rect_of(&driver, &format!("Chain: Block {index}")))
        .collect();
    assert!(!shown.is_empty(), "some blocks show");
    assert!(shown.len() < 14, "not all fourteen fit");
    for rect in &shown {
        assert!(strip.contains_rect(*rect), "{rect:?} leaves the strip");
    }
    assert!(driver.is_visible("Scroll blocks right"));
}

#[test]
fn stereo_lists_lane_a_then_lane_b() {
    let (driver, _) = stage_driver("main-stereo");
    let left = rect_of(&driver, "Chain: UK Greenback - Marshall 1960TV 4x12 M201").expect("lane A");
    let right = rect_of(&driver, "Chain: Roland JC-40").expect("lane B");
    assert!(left.max.x <= right.min.x, "lane A comes before lane B");
}

#[test]
fn a_long_preset_name_does_not_move_the_arrows() {
    let (short, _) = stage_driver("main-mono");
    let (long, _) = edited_driver("main-mono", |chain| {
        chain["preset"]["name"] =
            "A very long preset name that goes on and on and on and on".into();
    });
    assert!(rect_of(&short, "Next preset").is_some(), "the arrows show");
    assert_eq!(
        rect_of(&short, "Next preset"),
        rect_of(&long, "Next preset")
    );
    assert_eq!(
        rect_of(&short, "Previous preset"),
        rect_of(&long, "Previous preset")
    );
}

// ---------------------------------------------------------------- Task 4: stage

#[test]
fn the_first_block_opens_by_itself() {
    let (mut driver, _) = stage_driver("main-mono");
    assert!(
        matches!(
            driver.chrome().gallery().selection(),
            GallerySelection::BlockDetail(id) if *id == BlockId("blk-1".to_owned())
        ),
        "the first block of lane A is selected"
    );
    assert!(
        driver.chrome_mut().detail_mut().is_some(),
        "its detail shows"
    );
}

#[test]
fn an_empty_chain_prompts_to_add_a_block() {
    let (mut driver, _) = stage_driver("main-empty");
    assert!(driver.shows_text("Your chain is empty"));
    let _ = driver.click("Add your first block").expect("prompt button");
    driver.settle();
    assert!(
        driver.chrome_mut().browser_mut().is_some(),
        "the tone browser opened"
    );
}

#[test]
fn the_tuner_takes_the_stage() {
    let (mut driver, _) = stage_driver("main-mono");
    driver.chrome_mut().set_tuner(true);
    driver.settle();
    assert!(driver.chrome().tuner_visible());
    assert!(driver.chrome_mut().detail_mut().is_none() || !driver.is_visible("Back"));
    assert!(!driver.shows_text("Your chain is empty"));
}

// ---------------------------------------------------------------- Task 5: panel and utility bar

fn faceplate_shapes(look: Look) -> String {
    use eframe::egui;
    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-mono")));
    let mut faceplate = ronie_ui::default::faceplate::Faceplate::new(backend);
    let context = egui::Context::default();
    ronie_ui::fonts::Fonts::install(&context);
    Look::install(&context, look);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 578.0),
        )),
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| {
        egui::Panel::bottom("faceplate")
            .exact_size(108.0)
            .show(ui, |ui| faceplate.show(ui));
    });
    output.textures_delta.clear();
    output
        .shapes
        .iter()
        .map(|clipped| format!("{:?}", clipped.shape))
        .collect::<Vec<_>>()
        .join("\n")
}

fn color_text(color: eframe::egui::Color32) -> String {
    format!("{color:?}")
}

#[test]
fn faceplate_uses_the_light_panel_in_stage_mode() {
    let look = Look::stage();
    let shapes = faceplate_shapes(look);
    assert!(
        shapes.contains(&color_text(look.panel)),
        "the panel fill is light"
    );
    assert!(
        shapes.contains(&color_text(look.text_muted)),
        "captions use the muted text colour"
    );
    let reference = faceplate_shapes(Look::reference());
    assert!(
        !reference.contains(&color_text(look.panel)),
        "the reference stays dark"
    );
}

#[test]
fn utility_tuner_toggles_the_tuner() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver.click("Tuner").expect("tuner");
    driver.settle();
    assert!(driver.chrome().tuner_visible());
}

#[test]
fn utility_settings_opens_settings() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver.click("Settings").expect("settings");
    driver.settle();
    assert!(driver.chrome_mut().settings_mut().is_some());
}

#[test]
fn utility_mono_stereo_switch_modes() {
    let (mut driver, backend) = stage_driver("main-mono");
    let _ = driver.click("Stereo mode").expect("stereo");
    driver.settle();
    assert!(backend.chain_state(0).chain.stereo_enabled);
    let _ = driver.click("Mono mode").expect("mono");
    driver.settle();
    assert!(!backend.chain_state(0).chain.stereo_enabled);
}

#[test]
fn utility_undo_redo_follow_history() {
    let (mut driver, backend) = stage_driver("main-mono");
    let _ = driver.secondary_click(VOX).expect("the block menu");
    driver.settle();
    let _ = driver.click("BYPASS").expect("bypass a block");
    driver.settle();
    assert!(!backend.chain_state(0).chain.left[1].params.enabled);
    let _ = driver.click("Undo").expect("undo");
    driver.settle();
    assert!(
        backend.chain_state(0).chain.left[1].params.enabled,
        "undo restored it"
    );
    let _ = driver.click("Redo").expect("redo");
    driver.settle();
    assert!(
        !backend.chain_state(0).chain.left[1].params.enabled,
        "redo bypassed it again"
    );
}

#[test]
fn disabled_controls_fade_into_the_light_panel() {
    let veil = color_text(ronie_ui::theme::palette::KNOB_VEIL);
    assert!(
        faceplate_shapes(Look::reference()).contains(&veil),
        "the reference dims with the dark veil"
    );
    assert!(
        !faceplate_shapes(Look::stage()).contains(&veil),
        "no dark veil on the light panel"
    );
}

// ---------------------------------------------------------------- Task 7: review fixes

/// Runs `frames` frames of the whole window for `chrome` and returns the last frame's shapes.
fn window_shapes(chrome: &mut ChromeController, frames: usize) -> Vec<eframe::egui::Shape> {
    use eframe::egui;
    let context = egui::Context::default();
    ronie_ui::fonts::Fonts::install(&context);
    let mut shapes = Vec::new();
    for _ in 0..frames {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 658.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| ronie_ui::default::show(ui, chrome));
        });
        output.textures_delta.clear();
        shapes = output
            .shapes
            .into_iter()
            .map(|clipped| clipped.shape)
            .collect();
    }
    shapes
}

fn flat(shapes: Vec<eframe::egui::Shape>) -> Vec<eframe::egui::Shape> {
    let mut out = Vec::new();
    for shape in shapes {
        match shape {
            eframe::egui::Shape::Vec(inner) => out.extend(flat(inner)),
            other => out.push(other),
        }
    }
    out
}

#[test]
fn the_background_leaves_the_banner_visible() {
    use eframe::egui::Shape;
    let fixture = scenario("banner-no-input");
    let backend = Arc::new(MockBackend::from_scenario(&fixture));
    let mut chrome = ChromeController::new(backend, Vec::new(), false, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let device = ronie_ui::default::SystemDeviceState::from_ui_value(&fixture.state["device"])
        .expect("device settings")
        .banner;
    chrome.set_device_state(Some(device), 0);
    let shapes = flat(window_shapes(&mut chrome, 3));
    let background = Look::stage().background;
    let banner_drawn = shapes.iter().any(|shape| {
        matches!(shape, Shape::Rect(rect) if rect.rect.min.y < 1.0 && (40.0..=46.0).contains(&rect.rect.height()) && rect.fill == eframe::egui::Color32::BLACK)
    });
    assert!(banner_drawn, "the scenario shows a banner");
    let covered = shapes.iter().any(|shape| {
        matches!(shape, Shape::Rect(rect) if rect.fill == background && rect.rect.min.y < 40.0)
    });
    assert!(!covered, "no light fill is painted over the banner");
}

#[test]
fn removing_a_block_opens_its_neighbour_without_a_flash() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver.click("Remove").expect("remove");
    driver.frame();
    assert!(
        !driver.is_visible("Add Tone"),
        "the chain gallery never shows on the stage"
    );
    driver.settle();
    assert!(
        matches!(
            driver.chrome().gallery().selection(),
            GallerySelection::BlockDetail(id) if *id == BlockId("blk-2".to_owned())
        ),
        "the next block opens: {:?}",
        driver.chrome().gallery().selection()
    );
}

#[test]
fn removing_the_last_block_opens_the_previous_one() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver
        .click("Chain: UK Greenback - Marshall 1960TV 4x12 M201")
        .expect("third block");
    driver.settle();
    let _ = driver.click("Remove").expect("remove");
    for _ in 0..3 {
        assert!(
            !driver.is_visible("Add Tone"),
            "the chain gallery never shows on the stage"
        );
        driver.frame();
    }
    driver.settle();
    assert!(
        matches!(
            driver.chrome().gallery().selection(),
            GallerySelection::BlockDetail(id) if *id == BlockId("blk-2".to_owned())
        ),
        "the previous block opens: {:?}",
        driver.chrome().gallery().selection()
    );
}

#[test]
fn a_second_click_on_the_preset_pill_closes_the_browser() {
    use ronie_ui::default::Overlay;
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver.click("Browse presets").expect("pill");
    driver.settle();
    assert_eq!(driver.chrome().overlay(), Overlay::PresetBrowser);
    let _ = driver.click("Browse presets").expect("pill again");
    driver.settle();
    assert_eq!(
        driver.chrome().overlay(),
        Overlay::None,
        "the second click closes it"
    );
}

#[test]
fn deck_captions_stay_light_on_the_dark_deck() {
    use eframe::egui::{self, Shape};
    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-mono")));
    let mut faceplate = ronie_ui::default::faceplate::Faceplate::new(backend);
    faceplate.open_deck(ronie_ui::default::DeckKind::Gate);
    let context = egui::Context::default();
    ronie_ui::fonts::Fonts::install(&context);
    Look::install(&context, Look::stage());
    let mut texts = Vec::new();
    for _ in 0..2 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 578.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            egui::Panel::bottom("faceplate")
                .exact_size(108.0)
                .show(ui, |ui| faceplate.show(ui));
        });
        output.textures_delta.clear();
        texts = flat(
            output
                .shapes
                .into_iter()
                .map(|clipped| clipped.shape)
                .collect(),
        )
        .into_iter()
        .filter_map(|shape| match shape {
            Shape::Text(text) if text.pos.y < 470.0 => {
                Some((text.galley.text().to_owned(), text.fallback_color))
            }
            _ => None,
        })
        .collect();
    }
    assert!(!texts.is_empty(), "the deck shows above the panel");
    for (text, color) in &texts {
        // Unmultiplied, so that a dimmed caption is judged by its own colour.
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        let brightest = r.max(g).max(b);
        assert!(
            brightest >= 0x80,
            "{text} is dark ({color:?}) on the dark deck"
        );
    }
}

#[test]
fn the_block_you_add_opens_on_the_stage() {
    let (mut driver, backend) = stage_driver("main-mono");
    let _ = driver.click("Add a block").expect("plus");
    driver.settle();
    let _ = driver.click("OPEN THE TONE BROWSER").expect("browser link");
    driver.settle();
    let tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 4242, "title": "Freshly added amp", "format": "nam", "gear": "amp",
        "models": [{"id": 424201, "name": "One", "model_url": "https://fixtures.invalid/m/424201.nam"}]
    }))
    .expect("tone");
    driver.chrome_mut().pick_tone(tone);
    for _ in 0..20 {
        driver.settle();
    }
    let chain = backend.chain_state(0).chain;
    let added = chain
        .left
        .iter()
        .find(|block| {
            block
                .tone
                .as_ref()
                .is_some_and(|tone| tone.title == "Freshly added amp")
        })
        .map(|block| block.id.clone())
        .expect("the tone was added");
    assert!(
        matches!(driver.chrome().gallery().selection(), GallerySelection::BlockDetail(id) if *id == added),
        "the added block opens: {:?}",
        driver.chrome().gallery().selection()
    );
}

#[test]
fn a_loaded_tone_joins_the_library() {
    use ronie_ui::services::RecentTones;
    let (mut driver, backend) = stage_driver("main-mono");
    let _ = driver.click("Add a block").expect("plus");
    driver.settle();
    let _ = driver.click("OPEN THE TONE BROWSER").expect("browser link");
    driver.settle();
    let tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 4343, "title": "Library amp", "format": "nam", "gear": "amp",
        "models": [{"id": 434301, "name": "One", "model_url": "https://fixtures.invalid/m/434301.nam"}]
    }))
    .expect("tone");
    driver.chrome_mut().pick_tone(tone);
    for _ in 0..20 {
        driver.settle();
    }
    assert_eq!(
        driver.chrome().recent_tones().tones()[0].title,
        "Library amp"
    );
    let stored = backend
        .text_preference(RecentTones::KEY)
        .expect("the library is stored");
    assert!(stored.contains("Library amp"), "{stored}");
}

// ---------------------------------------------------------------- Task 8: designs

#[test]
fn the_utility_bar_holds_the_designed_items() {
    let (driver, _) = stage_driver("main-mono");
    for label in [
        "Tuner", "MIDI", "Optimize", "TONE3000", "Settings", "Account",
    ] {
        let rect = rect_of(&driver, label).unwrap_or_else(|| panic!("{label} is missing"));
        assert!(
            rect.min.y >= 578.0 - 1.0,
            "{label} sits in the utility bar: {rect:?}"
        );
    }
}

#[test]
fn optimize_toggles_from_the_utility_bar() {
    let (mut driver, backend) = stage_driver("main-mono");
    let before = ronie_ui::default::tuning::optimize(backend.as_ref());
    let _ = driver.click("Optimize").expect("optimize");
    driver.settle();
    assert_ne!(
        ronie_ui::default::tuning::optimize(backend.as_ref()),
        before
    );
}

#[test]
fn tone3000_opens_the_tone_browser() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver.click("TONE3000").expect("tone3000");
    driver.settle();
    assert!(driver.chrome_mut().browser_mut().is_some());
}

#[test]
fn undo_and_redo_sit_in_the_preset_manager() {
    let (driver, _) = stage_driver("main-mono");
    for label in ["Undo", "Redo"] {
        let rect = rect_of(&driver, label).unwrap_or_else(|| panic!("{label} is missing"));
        assert!(
            rect.max.y <= 64.0 && rect.min.x >= 652.0,
            "{label} sits in the header: {rect:?}"
        );
    }
}

#[test]
fn the_lanes_switch_sits_in_the_global_panel() {
    let (driver, _) = stage_driver("main-mono");
    for label in ["Mono mode", "Stereo mode"] {
        let rect = rect_of(&driver, label).unwrap_or_else(|| panic!("{label} is missing"));
        assert!(
            (470.0..578.0).contains(&rect.center().y),
            "{label} sits in the panel: {rect:?}"
        );
    }
}

#[test]
fn the_block_page_has_no_back_link() {
    let (driver, _) = stage_driver("main-mono");
    assert!(driver.chrome().gallery().selection() != &GallerySelection::None);
    assert!(!driver.is_visible("Back to chain"));
}

#[test]
fn plus_adds_to_the_lane_of_the_selected_block() {
    let (mut driver, _) = stage_driver("main-stereo");
    let _ = driver.click("Chain: Roland JC-40").expect("lane B block");
    driver.settle();
    let _ = driver.click("Add a block").expect("plus");
    driver.settle();
    let _ = driver.click("OPEN THE TONE BROWSER").expect("browser link");
    driver.settle();
    assert!(
        matches!(
            driver.chrome().gallery().selection(),
            GallerySelection::SelectTone {
                side: ronie_core::ChainSide::Right,
                ..
            }
        ) || driver.chrome_mut().browser_mut().is_some()
            && !matches!(
                driver.chrome().gallery().selection(),
                GallerySelection::SelectTone {
                    side: ronie_core::ChainSide::Left,
                    ..
                }
            ),
        "the browser fills lane B: {:?}",
        driver.chrome().gallery().selection()
    );
}

// ---------------------------------------------------------------- Rendered skins

#[test]
fn the_amp_page_shows_the_render_knobs_instead_of_the_trims() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver
        .click("Chain: '02 Vox AC30/6 Top Boost")
        .expect("amp");
    driver.settle();
    assert!(driver.is_visible("Amp render"));
    for label in ["GAIN IN:", "OUTPUT:", "MIX:"] {
        assert!(
            driver.label_starting_with(label).is_some(),
            "{label} is on the render"
        );
    }
    assert!(
        driver.label_starting_with("In:").is_none(),
        "the In trim gives way to GAIN IN"
    );
}

#[test]
fn dragging_gain_in_on_the_render_raises_the_input_gain() {
    let (mut driver, backend) = stage_driver("main-mono");
    let _ = driver
        .click("Chain: '02 Vox AC30/6 Top Boost")
        .expect("amp");
    driver.settle();
    let amp = |backend: &MockBackend| {
        backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .find(|block| {
                block
                    .tone
                    .as_ref()
                    .is_some_and(|tone| tone.title.contains("AC30"))
            })
            .expect("amp block")
            .params
            .input_gain
    };
    let before = amp(&backend);
    let up: Vec<_> = (1..=6)
        .map(|step| eframe::egui::vec2(0.0, -10.0 * step as f32))
        .collect();
    let label = driver
        .label_starting_with("GAIN IN:")
        .expect("GAIN IN knob");
    let _ = driver.drag_control(&label, &up).expect("drag");
    assert!(amp(&backend) > before, "input gain rose from {before}");
}

#[test]
fn a_cab_page_shows_its_render_without_knobs() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver
        .click("Chain: UK Greenback - Marshall 1960TV 4x12 M201")
        .expect("cab");
    driver.settle();
    assert!(driver.is_visible("Cab render"));
    assert!(driver.label_starting_with("GAIN IN:").is_none());
}

#[test]
fn the_reference_layout_keeps_its_trims() {
    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-mono")));
    let chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    driver
        .chrome_mut()
        .gallery_mut()
        .activate(&first_tone_id(&backend));
    driver.settle();
    assert!(driver.label_starting_with("In:").is_some());
    assert!(driver.label_starting_with("GAIN IN:").is_none());
}

fn first_tone_id(backend: &MockBackend) -> String {
    backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|block| block.tone.is_some())
        .expect("a tone block")
        .id
        .0
        .clone()
}

// ---------------------------------------------------------------- Light block page

use ronie_ui::default::block::BlockView;

fn amp_page() -> (UiDriver, Arc<MockBackend>) {
    let (mut driver, backend) = stage_driver("main-mono");
    let _ = driver.click(VOX).expect("amp");
    driver.settle();
    (driver, backend)
}

fn block_titled(backend: &MockBackend, needle: &str) -> Option<ronie_core::ChainBlock> {
    let state = backend.chain_state(0);
    state
        .chain
        .left
        .iter()
        .chain(state.chain.right.iter())
        .find(|block| {
            block
                .tone
                .as_ref()
                .is_some_and(|tone| tone.title.contains(needle))
        })
        .cloned()
}

#[test]
fn the_block_page_has_a_title_row_and_tabs() {
    let (driver, _) = amp_page();
    for label in ["GEAR", "EQ", "INFO", "Bypass", "Replace", "Remove"] {
        assert!(driver.is_visible(label), "{label} is on the page");
    }
    assert!(
        driver.shows_text("AMP · '02 VOX AC30/6 TOP BOOST"),
        "the title row names the block"
    );
    assert!(driver.shows_text("NAM capture from TONE3000 · 6 captures in this tone"));
}

#[test]
fn the_eq_tab_opens_the_eq() {
    let (mut driver, _) = amp_page();
    let _ = driver.click("EQ").expect("EQ tab");
    driver.settle();
    assert_eq!(
        driver.chrome().detail().map(|detail| detail.view()),
        Some(BlockView::Eq)
    );
    let _ = driver.click("GEAR").expect("GEAR tab");
    driver.settle();
    assert_eq!(
        driver.chrome().detail().map(|detail| detail.view()),
        Some(BlockView::Tone)
    );
}

#[test]
fn bypass_toggles_the_block() {
    let (mut driver, backend) = amp_page();
    let _ = driver.click("Bypass").expect("Bypass");
    driver.settle();
    assert!(!block_titled(&backend, "AC30").expect("amp").params.enabled);
}

#[test]
fn remove_removes_the_block() {
    let (mut driver, backend) = amp_page();
    let _ = driver.click("Remove").expect("Remove");
    driver.settle();
    assert!(block_titled(&backend, "AC30").is_none());
}

#[test]
fn the_amp_render_fills_the_designed_stage_height() {
    let (driver, _) = amp_page();
    let render = rect_of(&driver, "Amp render").expect("render");
    assert!(
        (240.0..=300.0).contains(&render.height()),
        "the render is {render:?}"
    );
}

#[test]
fn render_knobs_have_a_comfortable_hit_target() {
    let (driver, _) = amp_page();
    for prefix in ["GAIN IN:", "OUTPUT:", "MIX:"] {
        let label = driver.label_starting_with(prefix).expect("knob");
        let rect = rect_of(&driver, &label).expect("rect");
        assert!(
            rect.width() >= 28.0 && rect.height() >= 28.0,
            "{prefix} is {rect:?}"
        );
    }
}

#[test]
fn a_focused_render_knob_shows_its_value() {
    let (mut driver, _) = amp_page();
    for _ in 0..60 {
        if driver
            .focused()
            .is_some_and(|label| label.starts_with("GAIN IN"))
        {
            break;
        }
        let _ = driver.press_key(eframe::egui::Key::Tab);
    }
    assert!(
        driver
            .focused()
            .is_some_and(|label| label.starts_with("GAIN IN")),
        "Tab reaches GAIN IN"
    );
    driver.unhover();
    driver.settle();
    assert!(
        driver.shows_text("GAIN IN  0.0 dB"),
        "the value hint shows for the focused knob"
    );
}

#[test]
fn a_cab_page_keeps_its_level_and_mix_trims() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver
        .click("Chain: UK Greenback - Marshall 1960TV 4x12 M201")
        .expect("cab");
    driver.settle();
    for prefix in ["In:", "Mix:", "Out:"] {
        assert!(
            driver.label_starting_with(prefix).is_some(),
            "{prefix} trim"
        );
    }
}

#[test]
fn normalize_toggles_from_the_gear_page() {
    let (mut driver, backend) = amp_page();
    backend
        .dispatch(ronie_ui::backend::UiAction::SetPreference {
            key: "t3k.showBlockNormalizeControl".to_owned(),
            value: true,
        })
        .expect("show normalize");
    driver.settle();
    assert!(
        block_titled(&backend, "AC30")
            .expect("amp")
            .params
            .normalize
    );
    let _ = driver.click("NORMALIZE").expect("NORMALIZE");
    driver.settle();
    assert!(
        !block_titled(&backend, "AC30")
            .expect("amp")
            .params
            .normalize
    );
}

// ---------------------------------------------------------------- Pedals share the stage

const OD: &str = "TS808 Tube Screamer Overdrive";

/// main-mono with a green overdrive right after the fuzz.
fn pedalboard_driver() -> (UiDriver, Arc<MockBackend>) {
    let mut scenario = scenario("main-mono");
    let chain = scenario.state["chain"]["chain"]
        .as_array_mut()
        .expect("chain");
    let mut od = chain[0].clone();
    od["blockId"] = "blk-od".into();
    od["activeModelId"] = 1199.into();
    od["tone"]["id"] = 199.into();
    od["tone"]["title"] = OD.into();
    od["tone"]["models"] = serde_json::json!([{ "id": 1199, "name": "Drive 5" }]);
    chain.insert(1, od);
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    (driver, backend)
}

fn add_beside(
    placement: impl FnOnce(&ronie_core::ChainSnapshot) -> Placement,
) -> (Vec<String>, String) {
    let (mut driver, backend) = pedalboard_driver();
    let chain = backend.chain_state(0).chain;
    let insert = chain
        .left
        .iter()
        .find(|block| block.is_insert)
        .map(|block| block.id.0.clone())
        .expect("insert");
    let tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 4243, "title": "Placed amp", "format": "nam", "gear": "amp",
        "models": [{"id": 424301, "name": "One", "model_url": "https://fixtures.invalid/m/424301.nam"}]
    }))
    .expect("tone");
    let placement = placement(&chain);
    driver
        .chrome_mut()
        .add_tone(ronie_core::ChainSide::Left, &insert, tone, placement);
    for _ in 0..20 {
        driver.settle();
    }
    let titles = backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .filter_map(|block| block.tone.as_ref().map(|tone| tone.title.clone()))
        .collect();
    (titles, "Placed amp".to_owned())
}

#[test]
fn a_tone_added_after_a_block_lands_after_it() {
    let (titles, added) = add_beside(|chain| Placement::After(chain.left[0].id.clone()));
    assert_eq!(titles.len(), 5, "{titles:?}");
    assert_eq!(titles[1], added, "{titles:?}");
}

#[test]
fn a_vanished_anchor_falls_back_to_the_end() {
    let (titles, added) = add_beside(|_| Placement::After(ronie_core::BlockId("gone".to_owned())));
    assert_eq!(titles.len(), 5, "{titles:?}");
    assert_eq!(titles[4], added, "{titles:?}");
}

#[test]
fn consecutive_pedals_share_the_stage() {
    let (driver, _) = pedalboard_driver();
    assert!(driver.is_visible("Pedal render"), "the open pedal");
    assert!(
        driver.is_visible(&format!("Pedal render: {OD}")),
        "its neighbour shares the stage"
    );
    assert!(
        driver.shows_text("PEDALS · GREEN RUSSIAN BIG MUFF + BD-2 → TS808 TUBE SCREAMER OVERDRIVE")
    );
}

#[test]
fn clicking_a_neighbour_pedal_selects_it() {
    let (mut driver, _) = pedalboard_driver();
    let _ = driver
        .click(&format!("Pedal render: {OD}"))
        .expect("neighbour");
    driver.settle();
    assert_eq!(
        driver.chrome().gallery().selection(),
        &GallerySelection::BlockDetail(BlockId("blk-od".to_owned()))
    );
    assert!(
        driver.is_visible("Pedal render: Green Russian Big Muff + BD-2"),
        "the fuzz is now the neighbour"
    );
}

#[test]
fn an_amp_shares_the_stage_with_no_one() {
    let (mut driver, _) = pedalboard_driver();
    let _ = driver.click(VOX).expect("amp");
    driver.settle();
    assert!(driver.label_starting_with("Pedal render").is_none());
}

// ---------------------------------------------------------------- Slots on the stage

/// main-mono with `extra` more overdrives after the fuzz, so the pedals stage is crowded.
fn crowded_pedalboard(extra: usize) -> UiDriver {
    let mut scenario = scenario("main-mono");
    let chain = scenario.state["chain"]["chain"]
        .as_array_mut()
        .expect("chain");
    for index in 0..extra {
        let mut od = chain[0].clone();
        od["blockId"] = format!("blk-od{index}").into();
        od["tone"]["id"] = (200 + index).into();
        od["tone"]["title"] = format!("Overdrive {index}").into();
        chain.insert(1 + index, od);
    }
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    driver
}

#[test]
fn a_pedal_run_ends_with_a_pedal_slot() {
    let (driver, _) = pedalboard_driver();
    let slot = rect_of(&driver, "Add a pedal after these").expect("the slot");
    let last = rect_of(&driver, &format!("Pedal render: {OD}")).expect("last pedal");
    assert!(slot.left() >= last.right() + 17.9, "{slot:?} vs {last:?}");
    assert!((slot.center().y - last.center().y).abs() < 1.0);
    // 110 x 176 against a 300 pt render, scaled with the render.
    let scale = last.height() / 300.0;
    assert!((slot.width() - 110.0 * scale).abs() < 0.1, "{slot:?}");
    assert!((slot.height() - 176.0 * scale).abs() < 0.1, "{slot:?}");
}

#[test]
fn a_cab_page_has_no_slots() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver
        .click("Chain: UK Greenback - Marshall 1960TV 4x12 M201")
        .expect("cab");
    driver.settle();
    for name in [
        "Add a pedal after these",
        "Add a pedal before the amp",
        "Add a cab after the amp",
    ] {
        assert!(rect_of(&driver, name).is_none(), "{name}");
    }
}

#[test]
fn slots_stay_visible_in_a_narrow_window() {
    let roomy = rect_of(&pedalboard_driver().0, "Pedal render").expect("roomy pedal");
    let mut driver = crowded_pedalboard(4);
    let slot = rect_of(&driver, "Add a pedal after these").expect("the slot");
    let first = rect_of(&driver, "Pedal render").expect("open pedal");
    assert!(
        first.height() < roomy.height(),
        "the renders shrank: {first:?} vs {roomy:?}"
    );
    assert!(
        slot.right() <= 1024.0 - 56.0,
        "{slot:?} is inside the stage"
    );
    let scale = first.height() / 300.0;
    assert!((slot.height() - 176.0 * scale).abs() < 0.1, "{slot:?}");
    let _ = driver.click("Add a pedal after these").expect("clickable");
}

// ---------------------------------------------------------------- Light EQ and INFO

fn eq_block(backend: &MockBackend) -> ronie_core::EqParams {
    block_titled(backend, "AC30").expect("amp").params.eq
}

#[test]
fn the_eq_page_has_band_chips_power_and_pre_post() {
    let (mut driver, backend) = amp_page();
    let _ = driver.click("EQ").expect("EQ tab");
    driver.settle();
    for band in 1..=6 {
        assert!(
            driver.is_visible(&format!("Select band {band}")),
            "band {band}"
        );
    }
    let _ = driver.click("Select band 3").expect("band 3");
    driver.settle();
    assert_eq!(
        driver
            .chrome()
            .detail()
            .map(|detail| detail.eq().selected()),
        Some(2)
    );
    assert!(!eq_block(&backend).enabled);
    let _ = driver.click("EQ power").expect("power");
    driver.settle();
    assert!(eq_block(&backend).enabled, "the EQ turns on");
    assert!(!eq_block(&backend).pre);
    let _ = driver.click("PRE / POST").expect("pre/post");
    driver.settle();
    assert!(eq_block(&backend).pre, "the EQ moves before the block");
}

#[test]
fn the_eq_page_shows_the_curve() {
    let (mut driver, _) = amp_page();
    let _ = driver.click("EQ").expect("EQ tab");
    driver.settle();
    assert!(driver.is_visible("Band 1 frequency and gain"));
}

#[test]
fn the_info_page_shows_the_tone_and_its_page_link() {
    let (mut driver, _) = amp_page();
    let _ = driver.click("INFO").expect("INFO tab");
    driver.settle();
    assert!(driver.shows_text("'02 Vox AC30/6 Top Boost"));
    assert!(driver.is_visible("Open on TONE3000"));
}

#[test]
fn the_size_toggle_keeps_one_name() {
    let (mut driver, backend) = amp_page();
    backend
        .dispatch(ronie_ui::backend::UiAction::SetPreference {
            key: "t3k.showBlockSizeControl".to_owned(),
            value: true,
        })
        .expect("show size");
    driver.settle();
    let _ = driver.click("Full size").expect("size toggle");
    driver.settle();
    assert!(driver.is_visible("Full size"), "the name survives the flip");
}

// ---------------------------------------------------------------- Add menu results

const AMP_SEARCH: &str = "Search amps in your library and on TONE3000";

fn amp_tone(id: i64, title: &str) -> ronie_core::Tone {
    ronie_core::Tone::from_value(serde_json::json!({
        "id": id, "title": title, "format": "nam", "gear": "amp",
        "downloads_count": 3200, "user": {"username": "lowend_lab"},
        "models": [{"id": id * 10, "name": "m",
            "model_url": format!("https://fixtures.invalid/m/{id}.nam")}]
    }))
    .expect("tone")
}

/// A driver whose library holds `library`, newest first, and whose user is signed in or not.
fn menu_driver(
    id: &str,
    signed_in: bool,
    library: &[ronie_core::Tone],
) -> (UiDriver, Arc<MockBackend>) {
    use ronie_ui::{backend::UiAction, services::recent_tones::RecentTones};
    let mut recent = RecentTones::default();
    for tone in library.iter().rev() {
        recent.remember(tone.clone());
    }
    let backend = Arc::new(MockBackend::from_scenario(&scenario(id)));
    backend
        .dispatch(UiAction::SetTextPreference {
            key: RecentTones::KEY.to_owned(),
            value: recent.to_text(),
        })
        .expect("seed");
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), signed_in, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    (driver, backend)
}

fn open_amp(driver: &mut UiDriver) {
    open_menu(driver);
    let _ = driver.click("AMP").expect("AMP row");
    driver.settle();
}

/// Answers the query of the menu store with `tones`, out of `total` tones.
fn answer_menu(driver: &mut UiDriver, tones: Vec<ronie_core::Tone>, total: u64) {
    let store = driver.chrome_mut().add_store_mut().expect("the menu store");
    let generation = store.begin_query();
    let completed = store.complete_query(
        generation,
        ronie_core::Page {
            data: tones,
            page: 1,
            page_size: 12,
            total,
            total_pages: total.div_ceil(12),
        },
        false,
    );
    assert!(completed);
    driver.settle();
}

fn lane_titles(backend: &MockBackend, right: bool) -> Vec<String> {
    let chain = backend.chain_state(0).chain;
    let lane = if right { &chain.right } else { &chain.left };
    lane.iter()
        .filter(|block| !block.is_insert)
        .filter_map(|block| block.tone.as_ref().map(|tone| tone.title.clone()))
        .collect()
}

#[test]
fn a_category_shows_library_and_tone3000() {
    let (mut driver, _) = menu_driver("main-mono", true, &[]);
    open_amp(&mut driver);
    for text in ["YOUR LIBRARY", "TONE3000", "TRENDING"] {
        assert!(driver.shows_text(text), "{text} is missing");
    }
    assert!(driver.is_visible("Back to categories"));
    assert!(driver.is_visible(AMP_SEARCH));
    assert!(
        driver.shows_text("Nothing yet. Tones you load show up here."),
        "an empty library says so"
    );
    assert!(!driver.is_visible("PEDAL"), "the categories gave way");
}

#[test]
fn tone3000_rows_come_from_the_menu_store() {
    let (mut driver, _) = menu_driver("main-mono", true, &[]);
    open_amp(&mut driver);
    let store = driver.chrome_mut().add_store_mut().expect("the menu store");
    assert_eq!(store.query().gear, "amp");
    answer_menu(
        &mut driver,
        vec![
            amp_tone(501, "5150 III Red"),
            amp_tone(502, "Friedman BE-100"),
            amp_tone(503, "Twin Reverb Clean"),
        ],
        1284,
    );
    for title in ["5150 III Red", "Friedman BE-100", "Twin Reverb Clean"] {
        assert!(driver.is_visible(title), "{title} is missing");
    }
    assert!(driver.shows_text("lowend_lab \u{b7} 3.2k \u{2193}"));
    assert!(driver.is_visible("SHOW ALL 1,284 AMPS IN THE TONE BROWSER"));
}

#[test]
fn adding_a_library_amp_puts_it_before_the_cab_and_opens_it() {
    let (mut driver, backend) = menu_driver("main-mono", false, &[amp_tone(601, "Plexi Lead")]);
    open_amp(&mut driver);
    assert!(driver.shows_text("Downloaded \u{b7} 1 capture"));
    let _ = driver.click("Plexi Lead").expect("library row");
    driver.settle();
    let titles = lane_titles(&backend, false);
    assert_eq!(titles[2], "Plexi Lead", "{titles:?}");
    assert!(!driver.is_visible("Back to categories"), "the menu closed");
    let added = backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|block| {
            block
                .tone
                .as_ref()
                .is_some_and(|tone| tone.title == "Plexi Lead")
        })
        .map(|block| block.id.clone())
        .expect("the block");
    assert!(
        matches!(driver.chrome().gallery().selection(), GallerySelection::BlockDetail(id) if *id == added),
        "the new block is on the stage: {:?}",
        driver.chrome().gallery().selection()
    );
    assert!(driver.chrome_mut().browser_mut().is_none());
}

#[test]
fn stereo_adds_to_the_chosen_lane() {
    let (mut driver, backend) = menu_driver("main-stereo", false, &[amp_tone(602, "Lane B Amp")]);
    open_amp(&mut driver);
    let _ = driver.click("LANE B").expect("lane B");
    driver.settle();
    let _ = driver.click("Lane B Amp").expect("library row");
    driver.settle();
    assert!(lane_titles(&backend, true).contains(&"Lane B Amp".to_owned()));
    assert!(!lane_titles(&backend, false).contains(&"Lane B Amp".to_owned()));
}

#[test]
fn a_tone_already_in_the_chain_reads_in_chain() {
    let backend = MockBackend::from_scenario(&scenario("main-mono"));
    let vox = backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find_map(|block| block.tone.clone().filter(|tone| tone.title.contains("Vox")))
        .expect("the Vox tone");
    let (mut driver, _) = menu_driver("main-mono", false, &[vox, amp_tone(603, "Not Loaded")]);
    open_amp(&mut driver);
    assert!(driver.shows_text("IN CHAIN"));
    assert!(driver.shows_text("ADD"), "the other tone can be added");
}

#[test]
fn searching_filters_the_library() {
    let (mut driver, _) = menu_driver(
        "main-mono",
        true,
        &[amp_tone(604, "JCM800 Crunch"), amp_tone(605, "Twin Clean")],
    );
    open_amp(&mut driver);
    assert!(driver.is_visible("Twin Clean"));
    let _ = driver.fill(AMP_SEARCH, "jcm").expect("search");
    assert!(driver.is_visible("JCM800 Crunch"));
    assert!(!driver.is_visible("Twin Clean"));
    let text = driver
        .chrome_mut()
        .add_store_mut()
        .map(|store| store.query().text.clone());
    assert_eq!(text.as_deref(), Some("jcm"));
}

/// A build with no key yet, whose sign-in opens the key page.
struct NoKey;

impl ronie_ui::services::key_setup::KeySetup for NoKey {
    fn origin(&self) -> String {
        "https://www.fixtures.invalid".to_owned()
    }

    fn has_saved_key(&self) -> bool {
        false
    }

    fn save(
        &mut self,
        _key: &str,
    ) -> Result<
        Option<ronie_ui::services::key_setup::CommunityParts>,
        ronie_ui::services::key_setup::KeySetupError,
    > {
        Ok(None)
    }

    fn remove(
        &mut self,
    ) -> Result<
        Option<ronie_ui::services::key_setup::CommunityParts>,
        ronie_ui::services::key_setup::KeySetupError,
    > {
        Ok(None)
    }
}

#[test]
fn signed_out_shows_the_sign_in_box() {
    let (mut driver, _) = menu_driver("main-mono", false, &[amp_tone(606, "Kept Amp")]);
    driver.chrome_mut().use_key_setup(Box::new(NoKey));
    open_amp(&mut driver);
    assert!(driver.is_visible("SIGN IN"));
    assert!(driver.is_visible("Search amps in your library"));
    assert!(
        driver.shows_text("Sign in to search and download from the TONE3000 community library.")
    );
    assert!(driver.is_visible("Kept Amp"), "the library stays");
    assert!(!driver.shows_text("TRENDING"));
    assert!(!driver.chrome().auth_active());
    let _ = driver.click("SIGN IN").expect("sign in");
    driver.settle();
    // A build with no key starts its sign-in on the key page, as the browser's button does.
    assert!(driver.chrome().key_page().is_some(), "the sign-in starts");
    assert!(!driver.shows_text("YOUR LIBRARY"), "the menu closed");
}

#[test]
fn signing_out_while_results_show_turns_tone3000_into_the_sign_in_box() {
    let (mut driver, _) = menu_driver("main-mono", true, &[]);
    open_amp(&mut driver);
    answer_menu(&mut driver, vec![amp_tone(607, "Remote Amp")], 1);
    assert!(driver.is_visible("Remote Amp"));
    driver.chrome_mut().sign_out().expect("sign out");
    driver.frame();
    assert!(driver.is_visible("SIGN IN"));
    assert!(!driver.is_visible("Remote Amp"));
}

#[test]
fn show_all_opens_the_browser_filtered() {
    let (mut driver, _) = menu_driver("main-mono", true, &[]);
    open_amp(&mut driver);
    let _ = driver.fill(AMP_SEARCH, "plexi").expect("search");
    answer_menu(&mut driver, vec![amp_tone(608, "Plexi Remote")], 3);
    let _ = driver
        .click("SHOW ALL 3 AMPS IN THE TONE BROWSER")
        .expect("show all");
    driver.settle();
    let query = driver
        .chrome_mut()
        .browser_mut()
        .map(|browser| browser.store().query().clone())
        .expect("the browser opened");
    assert_eq!(query.gear, "amp");
    assert_eq!(query.text, "plexi");
    assert!(!driver.is_visible("Back to categories"), "the menu closed");
}

#[test]
fn back_returns_to_the_categories() {
    let (mut driver, _) = menu_driver("main-mono", true, &[]);
    open_amp(&mut driver);
    let _ = driver.click("Back to categories").expect("back");
    driver.settle();
    assert!(
        driver.is_visible("OPEN THE TONE BROWSER"),
        "the categories show again"
    );
    assert!(!driver.shows_text("YOUR LIBRARY"));
    assert!(
        driver.chrome_mut().add_store_mut().is_none(),
        "the results are gone"
    );
}

#[test]
fn a_second_add_takes_over_a_download_in_progress() {
    let (mut driver, backend) = menu_driver("main-mono", true, &[]);
    open_amp(&mut driver);
    answer_menu(
        &mut driver,
        vec![amp_tone(701, "Slow Download"), amp_tone(702, "Second Pick")],
        2,
    );
    // The first tone is still downloading.
    let insert = backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .rev()
        .find(|block| block.is_insert)
        .map(|block| block.id.0.clone())
        .expect("insert");
    let flow = driver.chrome_mut().load_flow_mut();
    flow.select(ronie_core::ChainSide::Left, &insert);
    let _ticket = flow
        .begin_remote_load(amp_tone(701, "Slow Download"))
        .expect("load");
    driver.settle();
    assert!(driver.shows_text("DOWNLOADING"));

    let _ = driver.click("Second Pick").expect("second row");
    driver.settle();
    let titles = lane_titles(&backend, false);
    assert!(titles.contains(&"Second Pick".to_owned()), "{titles:?}");
    assert!(!titles.contains(&"Slow Download".to_owned()), "{titles:?}");
    assert_eq!(driver.chrome().loading_tone(), None);
    assert!(!driver.shows_text("DOWNLOADING"));
}

#[test]
fn a_refused_library_load_shows_a_toast_and_keeps_the_menu_and_the_tone() {
    let local = ronie_core::Tone::from_value(serde_json::json!({
        "id": 0, "title": "Collected Stash", "format": "nam", "gear": "amp", "local": true,
        "models": [{"id": 1, "name": "m", "model_url": common::file_url("/stash/collected.nam")}]
    }))
    .expect("tone");
    let (mut driver, backend) = menu_driver("main-mono", false, &[local]);
    backend.refuse_tone_loads(true);
    open_amp(&mut driver);
    assert!(driver.shows_text("Local .nam file"));
    let _ = driver.click("Collected Stash").expect("library row");
    driver.settle();
    assert!(
        !lane_titles(&backend, false).contains(&"Collected Stash".to_owned()),
        "the chain stays as it was"
    );
    assert!(
        driver
            .chrome()
            .visible_toast()
            .is_some_and(|text| text.contains("try again")),
        "the player reads why: {:?}",
        driver.chrome().visible_toast()
    );
    assert!(driver.is_visible("Collected Stash"), "the menu stays open");
    assert!(
        driver
            .chrome()
            .recent_tones()
            .tones()
            .iter()
            .any(|tone| tone.title == "Collected Stash"),
        "the tone stays in the library"
    );
}

// ---------------------------------------------------------------- Add menu: load a file

/// Writes a short mono 16-bit WAV impulse response, a file that the menu's file dialog takes.
fn write_ir(path: &std::path::Path) {
    let samples = [0_i16, 1000, -1000, 0];
    let mut bytes = Vec::from(b"RIFF".as_slice());
    bytes.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
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
    std::fs::write(path, bytes).expect("WAV");
}

fn last_insert(backend: &MockBackend) -> String {
    backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .rev()
        .find(|block| block.is_insert)
        .map(|block| block.id.0.clone())
        .expect("insert")
}

#[test]
fn a_loaded_file_lands_at_the_end_of_the_lane_joins_the_library_and_opens() {
    let (mut driver, backend) = stage_driver("main-mono");
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("closet-cab.wav");
    write_ir(&path);
    let insert = last_insert(&backend);

    driver
        .chrome_mut()
        .drop_file_into(&insert, &path, Placement::End)
        .expect("the file loads");
    driver.settle();

    let lane = backend.chain_state(0).chain.left.clone();
    let last_tone = lane
        .iter()
        .rev()
        .find(|block| !block.is_insert)
        .expect("a tone block");
    assert_eq!(last_tone.id.0, insert, "the file fills the last insert");
    let title = last_tone.tone.as_ref().map(|tone| tone.title.clone());
    assert!(
        driver
            .chrome()
            .recent_tones()
            .tones()
            .first()
            .is_some_and(|tone| Some(&tone.title) == title.as_ref() && tone.local),
        "the file joins the library"
    );
    assert!(
        matches!(driver.chrome().gallery().selection(), GallerySelection::BlockDetail(id) if id.0 == insert),
        "the new block is on the stage: {:?}",
        driver.chrome().gallery().selection()
    );
}

#[test]
fn a_missing_file_shows_a_toast() {
    let (mut driver, backend) = stage_driver("main-mono");
    let directory = tempfile::tempdir().expect("tempdir");
    let insert = last_insert(&backend);
    let before = backend.chain_state(0).revision;

    let result = driver.chrome_mut().drop_file_into(
        &insert,
        &directory.path().join("gone.nam"),
        Placement::End,
    );
    driver.settle();

    assert!(result.is_err());
    assert_eq!(backend.chain_state(0).revision, before, "the chain stays");
    assert!(
        driver.chrome().visible_toast().is_some(),
        "the player reads that the file did not load"
    );
}

// ---------------------------------------------------------------- A slot opens the add menu

const PEDAL_SEARCH: &str = "Search pedals in your library";

fn gear_tone(id: i64, title: &str, gear: &str) -> ronie_core::Tone {
    ronie_core::Tone::from_value(serde_json::json!({
        "id": id, "title": title, "format": "nam", "gear": gear,
        "downloads_count": 1200, "user": {"username": "lowend_lab"},
        "models": [{"id": id * 10, "name": "m",
            "model_url": format!("https://fixtures.invalid/m/{id}.nam")}]
    }))
    .expect("tone")
}

/// A driver of scenario `id`, with its chain changed by `edit`, `library` in the library, and
/// its user signed in or not.
fn slot_driver(
    id: &str,
    signed_in: bool,
    library: &[ronie_core::Tone],
    edit: impl FnOnce(&mut Vec<serde_json::Value>),
) -> (UiDriver, Arc<MockBackend>) {
    use ronie_ui::{backend::UiAction, services::recent_tones::RecentTones};
    let mut scenario = scenario(id);
    edit(
        scenario.state["chain"]["chain"]
            .as_array_mut()
            .expect("chain"),
    );
    let mut recent = RecentTones::default();
    for tone in library.iter().rev() {
        recent.remember(tone.clone());
    }
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    backend
        .dispatch(UiAction::SetTextPreference {
            key: RecentTones::KEY.to_owned(),
            value: recent.to_text(),
        })
        .expect("seed");
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), signed_in, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    (driver, backend)
}

/// main-mono with a green overdrive right after the fuzz, as `pedalboard_driver`.
fn add_overdrive(chain: &mut Vec<serde_json::Value>) {
    let mut od = chain[0].clone();
    od["blockId"] = "blk-od".into();
    od["activeModelId"] = 1199.into();
    od["tone"]["id"] = 199.into();
    od["tone"]["title"] = OD.into();
    od["tone"]["models"] = serde_json::json!([{ "id": 1199, "name": "Drive 5" }]);
    chain.insert(1, od);
}

fn plus_is_solid(driver: &UiDriver) -> bool {
    driver
        .controls()
        .iter()
        .find(|control| control.label == "Add a block")
        .expect("the + button")
        .checked
}

/// The menu's outline: its back button sits in its top left corner inside the 8 pt padding, its
/// search field ends 8 + 6 + 8 pt inside its right edge, and its last link row ends 8 pt above
/// its bottom.
fn menu_rect(driver: &UiDriver, search: &str, last_row: &str) -> eframe::egui::Rect {
    let back = rect_of(driver, "Back to categories").expect("the back button");
    let field = rect_of(driver, search).expect("the search field");
    let link = rect_of(driver, last_row).expect("the last row");
    eframe::egui::Rect::from_min_max(
        back.min - eframe::egui::vec2(8.0, 8.0),
        eframe::egui::pos2(field.right() + 22.0, link.bottom() + 8.0),
    )
}

#[test]
fn the_pedal_slot_opens_the_menu_on_pedals() {
    let (mut driver, _) = slot_driver("main-mono", false, &[], add_overdrive);
    open_menu(&mut driver);
    assert!(plus_is_solid(&driver), "the + menu shows the + solid");
    let _ = driver.click("Add a block").expect("plus");
    driver.settle();

    let slot = rect_of(&driver, "Add a pedal after these").expect("the slot");
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    assert!(driver.is_visible("Back to categories"));
    assert!(driver.shows_text("PEDAL"), "the header names the category");
    assert!(
        driver.shows_text("after TS808 Tube Scream\u{2026}"),
        "the header says where the pedal goes"
    );
    assert!(!driver.shows_text("to the end of the chain"));
    assert!(!driver.is_visible("LANE A") && !driver.is_visible("LANE B"));
    assert!(!plus_is_solid(&driver), "the + stays dashed");

    let menu = menu_rect(&driver, PEDAL_SEARCH, "OPEN THE TONE BROWSER");
    assert!(!menu.intersects(slot), "{menu:?} covers {slot:?}");
    assert!(
        menu.right() <= slot.left() - 9.0 || menu.left() >= slot.right() + 9.0,
        "{menu:?} sits beside {slot:?}"
    );
    assert!((menu.width() - 380.0).abs() < 2.0, "{menu:?}");
    assert!(
        (menu.top()..=menu.bottom()).contains(&slot.center().y),
        "{menu:?} is level with {slot:?}"
    );
}

#[test]
fn a_second_click_on_the_slot_closes_its_menu() {
    let (mut driver, _) = slot_driver("main-mono", false, &[], add_overdrive);
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    assert!(driver.is_visible("Back to categories"));
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    assert!(!driver.is_visible("Back to categories"));
}

#[test]
fn the_plus_takes_over_a_slot_menu() {
    let (mut driver, _) = slot_driver("main-mono", false, &[], add_overdrive);
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    open_menu(&mut driver);
    assert!(driver.is_visible("OPEN THE TONE BROWSER"), "the + menu");
    assert!(driver.shows_text("placed by type"));
    assert!(plus_is_solid(&driver));
}

fn library_pedal() -> ronie_core::Tone {
    gear_tone(801, "Library Drive", "pedal")
}

#[test]
fn adding_from_the_pedal_slot_puts_the_pedal_after_the_run() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[library_pedal()], add_overdrive);
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    let _ = driver.click("Library Drive").expect("library row");
    driver.settle();
    let titles = lane_titles(&backend, false);
    assert_eq!(titles.len(), 5, "{titles:?}");
    assert_eq!(titles[1], OD, "{titles:?}");
    assert_eq!(titles[2], "Library Drive", "{titles:?}");
    assert!(!driver.is_visible("Back to categories"), "the menu closed");
    let added = block_titled(&backend, "Library Drive")
        .expect("the block")
        .id;
    assert!(
        matches!(driver.chrome().gallery().selection(), GallerySelection::BlockDetail(id) if *id == added),
        "the new block is on the stage: {:?}",
        driver.chrome().gallery().selection()
    );
}

#[test]
fn one_undo_removes_a_slot_add() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[library_pedal()], add_overdrive);
    let before = lane_titles(&backend, false);
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    let _ = driver.click("Library Drive").expect("library row");
    driver.settle();
    assert_eq!(lane_titles(&backend, false).len(), before.len() + 1);
    let _ = driver.click("Undo").expect("undo");
    driver.settle();
    assert_eq!(lane_titles(&backend, false), before);
}

#[test]
fn back_from_a_slot_menu_shows_categories_and_keeps_the_placement() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[library_pedal()], add_overdrive);
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    let _ = driver.click("Back to categories").expect("back");
    driver.settle();
    assert!(driver.is_visible("OPEN THE TONE BROWSER"), "the categories");
    assert!(
        driver.shows_text("after TS808 Tube Scream\u{2026}"),
        "the place stays"
    );
    let _ = driver.click("PEDAL").expect("PEDAL row");
    driver.settle();
    let _ = driver.click("Library Drive").expect("library row");
    driver.settle();
    let titles = lane_titles(&backend, false);
    assert_eq!(titles[2], "Library Drive", "{titles:?}");
}

#[test]
fn a_slot_menu_takes_over_a_download_of_the_plus_menu() {
    let (mut driver, backend) = slot_driver("main-mono", true, &[library_pedal()], add_overdrive);
    open_menu(&mut driver);
    let _ = driver.click("PEDAL").expect("PEDAL row");
    driver.settle();
    answer_menu(
        &mut driver,
        vec![gear_tone(701, "Slow Download", "pedal")],
        1,
    );
    // The + menu's pick is still downloading to the end of the lane.
    let insert = last_insert(&backend);
    let flow = driver.chrome_mut().load_flow_mut();
    flow.select(ronie_core::ChainSide::Left, &insert);
    let _ticket = flow
        .begin_remote_load(gear_tone(701, "Slow Download", "pedal"))
        .expect("load");
    driver.settle();
    assert!(driver.shows_text("DOWNLOADING"));
    // The + menu covers the slot, so the player closes it; the download goes on.
    let _ = driver.press_key(eframe::egui::Key::Escape);
    driver.settle();
    assert_eq!(
        driver.chrome().loading_tone(),
        Some(ronie_core::ToneId(701))
    );

    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    assert!(driver.is_visible("Back to categories"), "the slot menu");
    let _ = driver.click("Library Drive").expect("library row");
    driver.settle();
    let titles = lane_titles(&backend, false);
    assert_eq!(titles[2], "Library Drive", "{titles:?}");
    assert!(!titles.contains(&"Slow Download".to_owned()), "{titles:?}");
    assert_eq!(driver.chrome().loading_tone(), None);
    assert!(!driver.shows_text("DOWNLOADING"));
}

/// The labels of the slots that read as selected, which the stage draws solid.
fn solid_slots(driver: &UiDriver) -> Vec<String> {
    driver
        .controls()
        .iter()
        .filter(|control| control.label.starts_with("Add a ") && control.label != "Add a block")
        .filter(|control| control.checked)
        .map(|control| control.label.clone())
        .collect()
}

#[test]
fn the_open_slot_reads_as_selected() {
    let (mut driver, _) = slot_driver("main-mono", false, &[], add_overdrive);
    assert!(solid_slots(&driver).is_empty());
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    assert_eq!(solid_slots(&driver), ["Add a pedal after these"]);
}

#[test]
fn a_slot_menu_closes_when_its_slot_leaves_the_stage() {
    let (mut driver, backend) = slot_driver("main-mono", true, &[library_pedal()], add_overdrive);
    open_menu(&mut driver);
    let _ = driver.click("PEDAL").expect("PEDAL row");
    driver.settle();
    let slow = gear_tone(701, "Slow Download", "pedal");
    answer_menu(&mut driver, vec![slow.clone()], 1);
    // The + menu's pick is still downloading to the end of the lane.
    let insert = last_insert(&backend);
    let flow = driver.chrome_mut().load_flow_mut();
    flow.select(ronie_core::ChainSide::Left, &insert);
    let ticket = flow.begin_remote_load(slow.clone()).expect("load");
    driver.settle();
    let _ = driver.press_key(eframe::egui::Key::Escape);
    driver.settle();

    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    assert!(driver.is_visible("Back to categories"), "the slot menu");
    assert_eq!(solid_slots(&driver), ["Add a pedal after these"]);

    // The download lands and its pedal opens on the stage, whose own pedal slot sits after it.
    assert!(
        ticket
            .publish(slow, ronie_core::ModelId(7010), vec![1, 2, 3].into())
            .expect("publish")
    );
    assert!(
        driver
            .chrome_mut()
            .load_flow_mut()
            .finish_remote(ticket.generation())
    );
    driver.chrome_mut().gallery_mut().activate(&insert);
    driver.chrome_mut().sync_detail();
    driver.settle();
    assert!(
        driver.is_visible("Add a pedal after these"),
        "the new page has a pedal slot"
    );
    assert!(
        !driver.is_visible("Back to categories"),
        "the menu of the slot that left closes"
    );
    assert!(
        solid_slots(&driver).is_empty(),
        "{:?}",
        solid_slots(&driver)
    );
}

#[test]
fn a_browser_pick_from_a_slot_lands_at_the_slot() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[], add_overdrive);
    let before = lane_titles(&backend, false);
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    let _ = driver.click("OPEN THE TONE BROWSER").expect("browser link");
    driver.settle();
    assert!(
        driver.chrome_mut().browser_mut().is_some(),
        "the browser opened"
    );
    driver
        .chrome_mut()
        .pick_tone(gear_tone(803, "Browser Drive", "pedal"));
    for _ in 0..20 {
        driver.settle();
    }
    let titles = lane_titles(&backend, false);
    assert_eq!(titles.len(), before.len() + 1, "{titles:?}");
    assert_eq!(titles[2], "Browser Drive", "{titles:?}");
    let _ = driver.click("Undo").expect("undo");
    driver.settle();
    assert_eq!(lane_titles(&backend, false), before);
}

#[test]
fn a_file_from_a_slot_lands_at_the_slot() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[], add_overdrive);
    let before = lane_titles(&backend, false);
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("closet-cab.wav");
    write_ir(&path);
    let insert = last_insert(&backend);
    driver
        .chrome_mut()
        .drop_file_into(
            &insert,
            &path,
            Placement::After(BlockId("blk-od".to_owned())),
        )
        .expect("the file loads");
    driver.settle();
    let lane = backend.chain_state(0).chain.left.clone();
    let tones: Vec<_> = lane.iter().filter(|block| !block.is_insert).collect();
    assert_eq!(tones.len(), before.len() + 1);
    assert_eq!(tones[2].id.0, insert, "the file sits after the run");
    assert!(
        matches!(driver.chrome().gallery().selection(), GallerySelection::BlockDetail(id) if id.0 == insert),
        "the new block is on the stage"
    );
    let _ = driver.click("Undo").expect("undo");
    driver.settle();
    assert_eq!(lane_titles(&backend, false), before);
}

/// A local tone, which lands at once with or without a service.
fn local_pedal(id: i64, title: &str) -> ronie_core::Tone {
    ronie_core::Tone::from_value(serde_json::json!({
        "id": id, "title": title, "format": "nam", "gear": "pedal", "local": true,
        "models": [{"id": id * 10, "name": "m", "model_url": common::file_url(&format!("/stash/{id}.nam"))}]
    }))
    .expect("tone")
}

/// Opens the tone browser from the pedal slot after the run, for the lane's insert.
fn open_browser_from_the_slot(driver: &mut UiDriver) {
    let _ = driver.click("Add a pedal after these").expect("slot");
    driver.settle();
    let _ = driver.click("OPEN THE TONE BROWSER").expect("browser link");
    driver.settle();
    assert!(
        driver.chrome_mut().browser_mut().is_some(),
        "the browser opened"
    );
}

/// Loads `tone` into insert `insert` of lane A through the browser of the chain, which no slot
/// opened.
fn load_through_the_browser(driver: &mut UiDriver, insert: &str, tone: ronie_core::Tone) {
    let chrome = driver.chrome_mut();
    chrome
        .gallery_mut()
        .select_tone(ronie_core::ChainSide::Left, BlockId(insert.to_owned()));
    chrome.sync_browser();
    chrome.pick_tone(tone);
    chrome.sync_browser();
    driver.settle();
}

/// A key setup that starts a service over `transport` when a key is saved.
struct KeyOnSave(tempfile::TempDir, ronie_core::RecordedTransport);

/// Saves a key, which starts a service over `transport` in place of none.
fn start_a_service(driver: &mut UiDriver, transport: ronie_core::RecordedTransport) {
    let chrome = driver.chrome_mut();
    chrome.use_key_setup(Box::new(KeyOnSave(
        tempfile::tempdir().expect("temporary settings"),
        transport,
    )));
    chrome.open_key_page();
    chrome
        .key_page_mut()
        .expect("key page")
        .set_draft("t3k_pub_test_fake");
    chrome.save_key().expect("the key saves");
}

impl ronie_ui::services::key_setup::KeySetup for KeyOnSave {
    fn origin(&self) -> String {
        "https://www.fixtures.invalid".to_owned()
    }

    fn has_saved_key(&self) -> bool {
        false
    }

    fn save(
        &mut self,
        key: &str,
    ) -> Result<
        Option<ronie_ui::services::key_setup::CommunityParts>,
        ronie_ui::services::key_setup::KeySetupError,
    > {
        let origin = url::Url::parse("https://api.example").expect("origin");
        Ok(Some(
            ronie_ui::services::community::CommunityService::new(
                ronie_core::OAuthConfig::new(origin.clone(), key.to_owned()),
                ronie_core::OriginPolicy::new(origin, []),
                self.1.clone(),
                ronie_core::SettingsStore::standalone(
                    self.0.path(),
                    ronie_core::UI_PREFERENCES_SETTINGS,
                ),
            )
            .into(),
        ))
    }

    fn remove(
        &mut self,
    ) -> Result<
        Option<ronie_ui::services::key_setup::CommunityParts>,
        ronie_ui::services::key_setup::KeySetupError,
    > {
        Ok(None)
    }
}

#[test]
fn a_service_change_ends_the_place_of_a_browser_from_a_slot() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[], add_overdrive);
    let insert = last_insert(&backend);
    open_browser_from_the_slot(&mut driver);
    // A saved key starts a service, whose views start again with no browser.
    start_a_service(&mut driver, ronie_core::RecordedTransport::new(Vec::new()));
    assert!(driver.chrome_mut().browser_mut().is_none());

    load_through_the_browser(&mut driver, &insert, local_pedal(901, "Later Drive"));
    let titles = lane_titles(&backend, false);
    assert_eq!(
        titles.last().map(String::as_str),
        Some("Later Drive"),
        "{titles:?}"
    );
}

#[test]
fn a_browser_from_a_slot_places_only_the_tone_of_its_insert() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[], add_overdrive);
    let insert = last_insert(&backend);
    open_browser_from_the_slot(&mut driver);
    // A file fills the slot's insert at the end of the lane while the browser is open.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("closet-cab.wav");
    write_ir(&path);
    driver
        .chrome_mut()
        .drop_file_into(&insert, &path, Placement::End)
        .expect("the file loads");
    driver.settle();
    let file = lane_titles(&backend, false);

    // A later load into the next insert lands at the end, and the file stays where it is.
    let next = last_insert(&backend);
    assert_ne!(next, insert);
    load_through_the_browser(&mut driver, &next, local_pedal(902, "Next Drive"));
    let mut expected = file;
    expected.push("Next Drive".to_owned());
    assert_eq!(lane_titles(&backend, false), expected);
}

#[test]
fn a_file_load_keeps_the_place_of_a_slot_download() {
    let (mut driver, backend) = slot_driver("main-stereo", false, &[], |_| {});
    // A service whose first request, the download, waits for good.
    let held = Arc::new(tokio::sync::Notify::new());
    start_a_service(
        &mut driver,
        ronie_core::RecordedTransport::gated(Vec::new(), held.clone(), held),
    );
    driver.chrome_mut().close_auth();
    let chain = backend.chain_state(0).chain;
    let first = chain.left[0].id.clone();
    let insert = last_insert(&backend);
    let lane_b_insert = chain
        .right
        .iter()
        .rev()
        .find(|block| block.is_insert)
        .map(|block| block.id.0.clone())
        .expect("lane B insert");

    // A slot download before the first block of lane A starts and is held.
    let pedal = gear_tone(704, "Slot Download", "pedal");
    driver.chrome_mut().add_tone(
        ronie_core::ChainSide::Left,
        &insert,
        pedal.clone(),
        Placement::Before(first.clone()),
    );
    assert_eq!(
        driver.chrome().loading_tone(),
        Some(ronie_core::ToneId(704))
    );

    // A file lands at the end of lane B meanwhile.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("closet-cab.wav");
    write_ir(&path);
    driver
        .chrome_mut()
        .drop_file_into(&lane_b_insert, &path, Placement::End)
        .expect("the file loads");
    assert_eq!(
        lane_titles(&backend, true).last().map(String::as_str),
        Some("closet-cab")
    );

    // The download lands in its insert, as the worker publishes it.
    backend
        .dispatch(ronie_ui::backend::UiAction::LoadTone {
            target_insert_id: BlockId(insert.clone()),
            tone: pedal,
            model_id: ronie_core::ModelId(7040),
        })
        .expect("publish");
    let flow = driver.chrome_mut().load_flow_mut();
    let generation = flow.generation();
    assert!(flow.finish_remote(generation));
    driver.chrome_mut().sync_browser();
    let left = lane_titles(&backend, false);
    assert_eq!(
        left.first().map(String::as_str),
        Some("Slot Download"),
        "{left:?}"
    );
}

// ---------------------------------------------------------------- A cab's two mics

const CAB: &str = "Chain: UK Greenback - Marshall 1960TV 4x12 M201";

/// main-mono with its cab holding three mic positions, open on the cab.
fn cab_driver() -> (UiDriver, Arc<MockBackend>) {
    let (mut driver, backend) = edited_driver("main-mono", |chain| {
        let tone = &mut chain["chain"][2]["tone"];
        tone["models"] = serde_json::json!([
            {"id": 1103, "name": "M201 Cap Edge"},
            {"id": 1203, "name": "SM57 Cap Edge"},
            {"id": 1303, "name": "R121 Center"},
        ]);
        tone["models_count"] = 3.into();
        // A local pack switches its mics with no download and no sign-in.
        tone["local"] = true.into();
    });
    let _ = driver.click(CAB).expect("cab");
    driver.settle();
    (driver, backend)
}

fn cab(backend: &MockBackend) -> ronie_core::ChainBlock {
    block_titled(backend, "Greenback").expect("cab")
}

fn add_b(driver: &mut UiDriver) {
    let _ = driver.click("Add a second IR").expect("ADD A SECOND IR");
    driver.settle();
}

#[test]
fn a_cab_with_two_mics_offers_a_second_ir() {
    let (driver, backend) = cab_driver();
    let panel = rect_of(&driver, "Cab mics").expect("the MICS panel");
    assert!(driver.shows_text("MICS"));
    assert!(driver.shows_text("1 IR"));
    assert!(driver.is_visible("IR A capture"));
    assert!(driver.is_visible("Add a second IR"));
    assert!(driver.shows_text(
        "Blend a second mic: pick another mic position from this cab. Both play at once; BLEND \
         mixes them."
    ));
    assert!(!driver.is_visible("IR B capture"));
    assert!((panel.width() - 300.0).abs() < 0.5, "{panel:?}");
    let render = rect_of(&driver, "Cab render").expect("render");
    assert!(
        render.right() <= panel.left() - 20.0,
        "the render moves left of the panel: {render:?} vs {panel:?}"
    );
    assert_eq!(cab(&backend).ir_b, None);
}

#[test]
fn adding_b_picks_another_mic_and_shows_the_blend() {
    let (mut driver, backend) = cab_driver();
    add_b(&mut driver);
    let block = cab(&backend);
    let b = block.ir_b.expect("B is set").model_id;
    assert_ne!(Some(b), block.active_model_id, "another mic");
    assert!(driver.shows_text("2 IRs blended"));
    assert!(driver.is_visible("IR B capture"));
    assert!(!driver.is_visible("Add a second IR"));
    assert!(driver.is_visible("Blend: 50 / 50"));
    assert!(driver.is_visible("IR A level: 0.0 dB"));
    assert!(driver.is_visible("IR B level: 0.0 dB"));
    assert!(driver.is_visible("Invert IR B phase"));
    assert!(driver.is_visible("Remove IR B"));
}

#[test]
fn a_blend_drag_is_one_undo_step() {
    let (mut driver, backend) = cab_driver();
    add_b(&mut driver);
    let label = driver.label_starting_with("Blend:").expect("BLEND knob");
    let up: Vec<eframe::egui::Vec2> = (1..=6)
        .map(|step| eframe::egui::vec2(0.0, -6.0 * step as f32))
        .collect();
    let _ = driver.drag_control(&label, &up).expect("drag");
    let dragged = cab(&backend);
    assert!(dragged.params.ir_blend > 0.6, "{}", dragged.params.ir_blend);
    assert!(
        driver
            .label_starting_with("Blend:")
            .is_some_and(|label| label != "Blend: 50 / 50")
    );
    let _ = driver.click("Undo").expect("undo");
    driver.settle();
    let undone = cab(&backend);
    assert_eq!(
        undone.params.ir_blend, 0.5,
        "one step undoes the whole drag"
    );
    assert!(undone.ir_b.is_some(), "and leaves B");
}

#[test]
fn a_level_readout_drags_like_a_knob() {
    let (mut driver, backend) = cab_driver();
    add_b(&mut driver);
    let up: Vec<eframe::egui::Vec2> = (1..=4)
        .map(|step| eframe::egui::vec2(0.0, -5.0 * step as f32))
        .collect();
    let _ = driver
        .drag_control("IR B level: 0.0 dB", &up)
        .expect("drag");
    assert!(cab(&backend).params.ir_b_level > 0.5);
    assert_eq!(cab(&backend).params.ir_a_level, 0.5);
}

#[test]
fn removing_b_restores_a_alone() {
    let (mut driver, backend) = cab_driver();
    add_b(&mut driver);
    let _ = driver.click("Remove IR B").expect("REMOVE B");
    driver.settle();
    let block = cab(&backend);
    assert_eq!(block.ir_b, None);
    assert_eq!(block.active_model_id, Some(ronie_core::ModelId(1103)));
    assert!(driver.shows_text("1 IR"));
    assert!(driver.is_visible("Add a second IR"));
    assert!(!driver.is_visible("IR B capture"));
}

#[test]
fn invert_toggles_b_phase() {
    let (mut driver, backend) = cab_driver();
    add_b(&mut driver);
    assert!(!cab(&backend).params.ir_b_invert);
    let _ = driver.click("Invert IR B phase").expect("Ø B");
    driver.settle();
    assert!(cab(&backend).params.ir_b_invert);
    let _ = driver.click("Invert IR B phase").expect("Ø B");
    driver.settle();
    assert!(!cab(&backend).params.ir_b_invert);
}

#[test]
fn picking_b_from_its_capture_list_changes_only_b() {
    let (mut driver, backend) = cab_driver();
    add_b(&mut driver);
    let before = cab(&backend).ir_b.expect("B").model_id;
    let _ = driver.click("IR B capture").expect("B picker");
    driver.settle();
    assert!(
        !driver.is_visible("M201 Cap Edge"),
        "A's mic is not offered as B"
    );
    let other = if before.0 == 1203 {
        "R121 Center"
    } else {
        "SM57 Cap Edge"
    };
    let _ = driver.click(other).expect("other mic");
    driver.settle();
    let block = cab(&backend);
    assert_ne!(block.ir_b.expect("B").model_id, before);
    assert_eq!(block.active_model_id, Some(ronie_core::ModelId(1103)));
}

#[test]
fn a_b_with_no_bytes_shows_as_not_loaded() {
    let (mut driver, backend) = edited_driver("main-mono", |chain| {
        let block = &mut chain["chain"][2];
        block["tone"]["models"] = serde_json::json!([
            {"id": 1103, "name": "M201 Cap Edge"},
            {"id": 1203, "name": "SM57 Cap Edge"},
        ]);
        block["irBModelId"] = 1203.into();
        block["irBMissing"] = true.into();
    });
    assert!(cab(&backend).ir_b_missing);
    let _ = driver.click(CAB).expect("cab");
    driver.settle();
    assert!(driver.is_visible("IR B capture (not loaded)"));
    assert!(!driver.is_visible("IR B capture"));
}

#[test]
fn a_single_model_cab_has_no_second_ir() {
    let (mut driver, _) = edited_driver("main-mono", |chain| {
        chain["chain"][2]["tone"]["models_count"] = 1.into();
    });
    let _ = driver.click(CAB).expect("cab");
    driver.settle();
    assert!(driver.is_visible("Cab render"));
    assert!(!driver.is_visible("Cab mics"));
    assert!(!driver.is_visible("Add a second IR"));
}

#[test]
fn the_reference_layout_has_no_mics_panel() {
    let mut doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/ui-scenarios.json")).expect("json");
    let scenario = doc["scenarios"]
        .as_array_mut()
        .expect("scenarios")
        .iter_mut()
        .find(|scenario| scenario["id"] == "main-mono")
        .expect("scenario");
    let block = &mut scenario["state"]["chain"]["chain"][2];
    block["tone"]["models"] = serde_json::json!([
        {"id": 1103, "name": "M201 Cap Edge"},
        {"id": 1203, "name": "SM57 Cap Edge"},
    ]);
    block["irBModelId"] = 1203.into();
    let scenario = load_scenarios(&doc.to_string())
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-mono")
        .expect("scenario");
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    driver.chrome_mut().gallery_mut().activate("blk-3");
    driver.settle();
    assert!(
        driver.label_starting_with("In:").is_some(),
        "the detail is open"
    );
    for name in [
        "Cab mics",
        "Add a second IR",
        "IR A capture",
        "IR B capture",
        "Remove IR B",
        "Invert IR B phase",
    ] {
        assert!(!driver.is_visible(name), "{name}");
    }
    assert!(driver.label_starting_with("Blend:").is_none());
}

// ---------------------------------------------------------------- The pedal LCD

/// The pedalboard where the fuzz is a local tone with three captures, followed by the
/// single-capture overdrive.
fn capture_pedalboard() -> (UiDriver, Arc<MockBackend>) {
    crowded_capture_pedalboard(0)
}

/// `capture_pedalboard` with `extra` more overdrives after it, so the renders shrink.
fn crowded_capture_pedalboard(extra: usize) -> (UiDriver, Arc<MockBackend>) {
    let mut scenario = scenario("main-mono");
    let chain = scenario.state["chain"]["chain"]
        .as_array_mut()
        .expect("chain");
    chain[0]["tone"]["local"] = true.into();
    chain[0]["tone"]["models"] = serde_json::json!([
        { "id": 1104, "name": "Muff 1" },
        { "id": 1105, "name": "Muff 2" },
        { "id": 1106, "name": "Muff 3" },
    ]);
    let mut od = chain[0].clone();
    od["blockId"] = "blk-od".into();
    od["activeModelId"] = 1199.into();
    od["tone"]["id"] = 199.into();
    od["tone"]["title"] = OD.into();
    od["tone"]["models"] = serde_json::json!([{ "id": 1199, "name": "Drive 5" }]);
    for index in 0..extra {
        let mut more = od.clone();
        more["blockId"] = format!("blk-od{index}").into();
        more["tone"]["id"] = (300 + index).into();
        more["tone"]["title"] = format!("Overdrive {index}").into();
        chain.insert(1, more);
    }
    chain.insert(1, od);
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.set_view_mode(ViewMode::RonieFlow);
    let mut driver = UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    (driver, backend)
}

fn active_model(backend: &MockBackend, block: &str) -> Option<i64> {
    backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|each| each.id.0 == block)
        .and_then(|each| each.active_model_id)
        .map(|id| id.0)
}

#[test]
fn a_pedal_lcd_shows_its_capture() {
    let (driver, _) = capture_pedalboard();
    assert!(
        driver.is_visible("Capture display: Muff 1"),
        "{:?}",
        driver.visible_labels()
    );
    assert!(driver.shows_text("MUFF 1"));
}

#[test]
fn hovering_the_lcd_shows_the_capture_hint() {
    let (mut driver, _) = capture_pedalboard();
    driver.hover("Capture display: Muff 1").expect("LCD");
    driver.settle();
    assert!(driver.shows_text("CAPTURE 1 / 3 · Muff 1 · click for next"));
}

#[test]
fn clicking_the_lcd_steps_to_the_next_capture() {
    let (mut driver, backend) = capture_pedalboard();
    let _ = driver.click("Capture display: Muff 1").expect("LCD");
    driver.settle();
    assert_eq!(active_model(&backend, "blk-1"), Some(1105));
    assert!(driver.shows_text("MUFF 2"));
    let _ = driver.click("Capture display: Muff 2").expect("LCD");
    driver.settle();
    assert_eq!(active_model(&backend, "blk-1"), Some(1106));
    let _ = driver.click("Capture display: Muff 3").expect("LCD");
    driver.settle();
    assert_eq!(active_model(&backend, "blk-1"), Some(1104), "wraps");
    assert!(driver.is_visible("Capture display: Muff 1"));
}

#[test]
fn a_single_capture_pedal_does_nothing_on_click() {
    let (mut driver, backend) = capture_pedalboard();
    let _ = driver
        .click(&format!("Pedal render: {OD}"))
        .expect("neighbour");
    driver.settle();
    let revision = backend.chain_state(0).revision;
    let _ = driver.click("Capture display: Drive 5").expect("LCD");
    driver.settle();
    assert_eq!(active_model(&backend, "blk-od"), Some(1199));
    assert_eq!(backend.chain_state(0).revision, revision);
    assert!(driver.shows_text("DRIVE 5"));
}

#[test]
fn a_neighbour_lcd_is_shown_but_not_interactive() {
    let (mut driver, _) = capture_pedalboard();
    assert!(driver.shows_text("DRIVE 5"), "the neighbour's LCD");
    assert!(
        driver
            .label_starting_with("Capture display: Drive")
            .is_none(),
        "no display control on a neighbour"
    );
    let _ = driver
        .click(&format!("Pedal render: {OD}"))
        .expect("neighbour");
    driver.settle();
    assert_eq!(
        driver.chrome().gallery().selection(),
        &GallerySelection::BlockDetail(BlockId("blk-od".to_owned()))
    );
}

/// The fuzz of main-detail on its own Stage page, attached to a service that lists
/// captures 1104 to 1106. Clicks and frames go through `lcd_frame`.
fn catalog_pedal(backend: &Arc<MockBackend>) -> ronie_ui::default::BlockDetail {
    use ronie_ui::services::{ModelList, ModelSwitchFlow, session::SessionStatus::SignedIn};

    let shared: Arc<dyn UiBackend> = backend.clone();
    let mut detail =
        ronie_ui::default::BlockDetail::new(shared.clone(), "blk-1", false).expect("detail");
    detail.set_page(ronie_ui::default::block::BlockPage::Stage);
    detail.attach_model_switching(ModelSwitchFlow::new(shared));
    let request = detail.take_models_request(SignedIn).expect("a list is due");
    let models = [1104, 1105, 1106]
        .into_iter()
        .map(|id| {
            ronie_core::ToneModel::from_value(
                &serde_json::json!({
                    "id": id,
                    "name": format!("Model {id}"),
                    "model_url": format!("https://models.example/{id}.nam"),
                }),
                false,
            )
            .expect("model")
        })
        .collect();
    assert!(detail.complete_models(
        request.generation,
        request.tone,
        ModelList { models, total: 3 }
    ));
    detail
}

/// One frame of a lone detail. Returns its controls by label and its static texts.
fn lcd_frame(
    context: &eframe::egui::Context,
    detail: &mut ronie_ui::default::BlockDetail,
    events: Vec<eframe::egui::Event>,
) -> (Vec<(String, eframe::egui::Rect)>, Vec<String>) {
    use eframe::egui;
    use ronie_ui::services::session::SessionStatus::SignedIn;

    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 614.0),
        )),
        events,
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| detail.show(ui, SignedIn));
    output.textures_delta.clear();
    let Some(update) = output.platform_output.accesskit_update.take() else {
        return (Vec::new(), Vec::new());
    };
    let widgets = context.viewport(|viewport| {
        viewport
            .prev_pass
            .widgets
            .layers()
            .flat_map(|(_, widgets)| widgets.iter().copied())
            .map(|widget| (widget.id.accesskit_id(), widget))
            .collect::<std::collections::HashMap<_, _>>()
    });
    let controls = update
        .nodes
        .iter()
        .filter_map(|(id, node)| {
            let widget = widgets.get(id)?;
            Some((node.label()?.to_owned(), widget.interact_rect))
        })
        .collect();
    let texts = update
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == accesskit::Role::Label)
        .filter_map(|(_, node)| node.value().map(str::to_owned))
        .collect();
    (controls, texts)
}

/// Clicks the control whose label starts with `prefix`.
fn lcd_click(
    context: &eframe::egui::Context,
    detail: &mut ronie_ui::default::BlockDetail,
    prefix: &str,
) {
    use eframe::egui;

    let (controls, _) = lcd_frame(context, detail, Vec::new());
    let at = controls
        .iter()
        .find(|(label, _)| label.starts_with(prefix))
        .map(|(_, rect)| rect.center())
        .unwrap_or_else(|| panic!("no {prefix}: {controls:?}"));
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    lcd_frame(context, detail, vec![egui::Event::PointerMoved(at)]);
    lcd_frame(context, detail, vec![button(true)]);
    lcd_frame(context, detail, vec![button(false)]);
}

fn lcd_context() -> eframe::egui::Context {
    let context = eframe::egui::Context::default();
    context.enable_accesskit();
    ronie_ui::fonts::Fonts::install(&context);
    context
}

#[test]
fn the_lcd_shows_dots_while_the_capture_downloads() {
    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-detail")));
    let mut detail = catalog_pedal(&backend);
    let context = lcd_context();
    lcd_frame(&context, &mut detail, Vec::new());
    let (_, texts) = lcd_frame(&context, &mut detail, Vec::new());
    assert!(texts.iter().any(|text| text == "MODEL 1104"), "{texts:?}");

    lcd_click(&context, &mut detail, "Capture display: Model 1104");
    let request = detail.take_switch_request().expect("a download");
    assert_eq!(request.model.id, ronie_core::ModelId(1105));
    let (_, texts) = lcd_frame(&context, &mut detail, Vec::new());
    assert!(texts.iter().any(|text| text == "···"), "{texts:?}");
    assert!(
        !texts.iter().any(|text| text.starts_with("MODEL")),
        "{texts:?}"
    );
}

#[test]
fn rapid_lcd_clicks_end_on_the_landed_capture() {
    use ronie_core::{ModelId, Tone};

    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-detail")));
    let mut detail = catalog_pedal(&backend);
    let context = lcd_context();
    lcd_frame(&context, &mut detail, Vec::new());

    lcd_click(&context, &mut detail, "Capture display:");
    let first = detail.take_switch_request().expect("a download");
    lcd_click(&context, &mut detail, "Capture display:");
    let second = detail.take_switch_request().expect("the next download");
    assert_eq!(first.model.id, ModelId(1105));
    assert_eq!(
        second.model.id,
        ModelId(1106),
        "steps from the capture on its way"
    );
    assert!(
        detail.take_switch_request().is_none(),
        "one switch at a time"
    );

    let stored = |request: &ronie_ui::services::ModelSwitchRequest, id: i64| {
        Tone {
            models: vec![request.model.clone()],
            ..request.tone.clone()
        }
        .for_storage(ModelId(id), &[])
        .expect("stored tone")
    };
    let late = first
        .ticket
        .publish(stored(&first, 1105), ModelId(1105), vec![1, 2, 3].into());
    assert!(
        !matches!(late, Ok(true)),
        "the superseded switch cannot land"
    );
    assert!(!detail.finish_switch(first.ticket.generation()));
    assert!(
        second
            .ticket
            .publish(stored(&second, 1106), ModelId(1106), vec![1, 2, 3].into())
            .expect("publish")
    );
    assert!(detail.finish_switch(second.ticket.generation()));

    let (controls, texts) = lcd_frame(&context, &mut detail, Vec::new());
    assert!(texts.iter().any(|text| text == "MODEL 1106"), "{texts:?}");
    assert!(!texts.iter().any(|text| text == "···"), "{texts:?}");
    assert!(
        controls
            .iter()
            .any(|(label, _)| label == "Capture display: Model 1106"),
        "{controls:?}"
    );
    assert_eq!(active_model(&backend, "blk-1"), Some(1106));
}

#[test]
fn a_knob_beside_a_small_lcd_keeps_its_clicks() {
    use eframe::egui;

    let (mut driver, backend) = crowded_capture_pedalboard(5);
    let display = rect_of(&driver, "Capture display: Muff 1").expect("LCD");
    let render = rect_of(&driver, "Pedal render").expect("render");
    assert!(render.height() < 190.0, "a small render: {render:?}");
    // The knob nearest the LCD, and a point that both hit areas cover.
    let (label, knob) = ["GAIN IN:", "OUTPUT:", "MIX:"]
        .into_iter()
        .filter_map(|prefix| {
            let label = driver.label_starting_with(prefix)?;
            let rect = rect_of(&driver, &label)?;
            Some((label, rect))
        })
        .min_by(|(_, a), (_, b)| {
            a.center()
                .distance(display.center())
                .total_cmp(&b.center().distance(display.center()))
        })
        .expect("a knob");
    let shared = knob.intersect(display);
    assert!(shared.is_positive(), "{knob:?} and {display:?} overlap");
    let at = shared.center();
    let prefix = label.split(':').next().expect("prefix").to_owned();

    let _ = driver.click_at(at);
    driver.settle();
    assert_eq!(
        active_model(&backend, "blk-1"),
        Some(1104),
        "a click on the knob"
    );
    assert!(
        driver
            .focused()
            .is_some_and(|focused| focused.starts_with(&prefix)),
        "the knob takes the click: {:?}",
        driver.focused()
    );

    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    driver.play(vec![egui::Event::PointerMoved(at)]);
    for _ in 0..2 {
        driver.play(vec![button(true)]);
        driver.play(vec![button(false)]);
    }
    driver.settle();
    assert_eq!(
        active_model(&backend, "blk-1"),
        Some(1104),
        "a double-click on the knob"
    );
}

#[test]
fn an_lcd_step_is_one_undo_step() {
    let (mut driver, backend) = capture_pedalboard();
    let _ = driver.click("Capture display: Muff 1").expect("LCD");
    driver.settle();
    assert_eq!(active_model(&backend, "blk-1"), Some(1105));
    let _ = driver.click("Undo").expect("undo");
    driver.settle();
    assert_eq!(active_model(&backend, "blk-1"), Some(1104), "one undo");
    assert!(driver.is_visible("Capture display: Muff 1"));
}

#[test]
fn a_downloading_display_says_so() {
    let backend = Arc::new(MockBackend::from_scenario(&scenario("main-detail")));
    let mut detail = catalog_pedal(&backend);
    let context = lcd_context();
    lcd_frame(&context, &mut detail, Vec::new());
    lcd_click(&context, &mut detail, "Capture display: Model 1104");
    let _request = detail.take_switch_request().expect("a download");
    let (controls, _) = lcd_frame(&context, &mut detail, Vec::new());
    assert!(
        controls
            .iter()
            .any(|(label, _)| label == "Capture display: Model 1105 (loading)"),
        "{controls:?}"
    );
}

// ---------------------------------------------------------------- The amp capture lights

/// The amp of main-mono, open on the stage, with `count` captures named "Take 1" and up.
fn amp_driver(count: usize) -> (UiDriver, Arc<MockBackend>) {
    let (mut driver, backend) = edited_driver("main-mono", |chain| {
        let models: Vec<_> = (0..count)
            .map(|index| serde_json::json!({ "id": 1101 + index, "name": format!("Take {}", index + 1) }))
            .collect();
        chain["chain"][1]["tone"]["models"] = models.into();
        chain["chain"][1]["tone"]["local"] = true.into();
    });
    let _ = driver.click(VOX).expect("amp button");
    driver.settle();
    (driver, backend)
}

/// The lights of the open amp: label and whether lit, in order.
fn lights(driver: &UiDriver, count: usize) -> Vec<(String, bool)> {
    (1..=count)
        .filter_map(|index| {
            let label = format!("Take {index}");
            driver
                .controls()
                .iter()
                .find(|control| control.label == label)
                .map(|control| (label, control.checked))
        })
        .collect()
}

#[test]
fn an_amp_with_captures_shows_its_lights() {
    let (driver, _) = amp_driver(6);
    assert!(
        driver.is_visible("Captures"),
        "{:?}",
        driver.visible_labels()
    );
    assert!(driver.shows_text("CAPTURE"));
    let lit = lights(&driver, 6);
    assert_eq!(lit.len(), 6);
    assert_eq!(lit.iter().filter(|(_, on)| *on).count(), 1);
    assert!(lit[0].1, "the active capture is lit");
    assert!(
        driver.shows_text("Take 1"),
        "its short name beside the lights"
    );
}

#[test]
fn clicking_a_light_selects_its_capture() {
    let (mut driver, backend) = amp_driver(6);
    let _ = driver.click("Take 4").expect("light");
    driver.settle();
    assert_eq!(active_model(&backend, "blk-2"), Some(1104));
    let lit = lights(&driver, 6);
    assert!(lit[3].1 && lit.iter().filter(|(_, on)| *on).count() == 1);
}

#[test]
fn a_single_capture_amp_has_no_lights() {
    let (driver, _) = amp_driver(1);
    assert!(!driver.is_visible("Captures"));
    assert!(!driver.shows_text("CAPTURE"));
    assert!(!driver.is_visible("Take 1"));
}

#[test]
fn nine_captures_page_the_light_window() {
    let (mut driver, _) = amp_driver(9);
    let shown = lights(&driver, 9);
    assert_eq!(shown.len(), 8, "at most eight lights");
    assert!(shown[0].1, "the active light is inside the window");
    assert!(driver.is_visible("Next captures"));
    let _ = driver.click("Next captures").expect("page right");
    driver.settle();
    let shown = lights(&driver, 9);
    assert_eq!(shown.len(), 8);
    assert!(
        shown.iter().all(|(label, _)| label != "Take 1"),
        "paged by one"
    );
    let _ = driver.click("Take 9").expect("last light");
    driver.settle();
    let shown = lights(&driver, 9);
    assert!(shown.iter().any(|(label, on)| label == "Take 9" && *on));
    let _ = driver.click("Previous captures").expect("page left");
    driver.settle();
    let shown = lights(&driver, 9);
    assert_eq!(
        shown.first().map(|(label, _)| label.as_str()),
        Some("Take 1")
    );
    assert!(
        shown.iter().all(|(_, on)| !*on),
        "paged past the active one"
    );
}

#[test]
fn eight_captures_need_no_paging() {
    let (driver, _) = amp_driver(8);
    assert_eq!(lights(&driver, 8).len(), 8);
    assert!(!driver.is_visible("Next captures"));
    assert!(!driver.is_visible("Previous captures"));
}

/// At 640 the pill only fits without its short name, and at 600 only beside a chip that reads
/// just "SKIN"; nothing may run past the stage, whose right meter column is 80 pt wide.
#[test]
fn the_capture_pill_stays_inside_a_narrow_window() {
    for width in [700.0, 640.0, 600.0] {
        let (mut driver, _) = amp_driver(9);
        driver.set_window_size(eframe::egui::vec2(width, 600.0));
        let stage_right = width - 80.0;
        let window = eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(width, 600.0),
        );
        let inside = |label: &str| {
            let rect =
                rect_of(&driver, label).unwrap_or_else(|| panic!("{label} is drawn at {width}"));
            assert!(
                rect.is_positive() && window.contains_rect(rect),
                "{label} at {rect:?} in a window {width} wide"
            );
            rect
        };
        let next = inside("Next captures");
        let previous = inside("Previous captures");
        assert_eq!(
            next.size(),
            previous.size(),
            "the next arrow is not clipped"
        );
        let chip = inside("Skin");
        let pill = inside("Captures");
        assert!(
            pill.right() <= stage_right,
            "the pill {pill:?} ends inside the stage at {width}"
        );
        assert!(
            pill.left() >= chip.right(),
            "the pill {pill:?} clears the chip {chip:?} at {width}"
        );
        assert!(next.right() <= pill.right(), "the arrow is inside the pill");
        let shown = lights(&driver, 9);
        assert_eq!(shown.len(), 8, "{:?}", driver.visible_labels());
        for (label, _) in &shown {
            let light = inside(label);
            assert!(
                pill.contains_rect(light.shrink(1.0)),
                "{label} at {light:?}"
            );
        }
    }
}

#[test]
fn the_active_light_follows_a_capture_change_from_the_picker() {
    let (mut driver, backend) = amp_driver(6);
    driver
        .chrome_mut()
        .detail_mut()
        .expect("detail")
        .select_model(
            ronie_core::ModelId(1103),
            ronie_ui::services::session::SessionStatus::SignedIn,
        )
        .expect("picker select");
    driver.settle();
    assert_eq!(active_model(&backend, "blk-2"), Some(1103));
    let lit = lights(&driver, 6);
    assert!(lit[2].1, "{lit:?}");
}

#[test]
fn cabs_and_pedals_have_no_lights() {
    let (driver, _) = capture_pedalboard();
    assert!(driver.is_visible("Capture display: Muff 1"));
    assert!(!driver.is_visible("Captures"));
    assert!(!driver.is_visible("Muff 2"));
    let (mut cab, _) = edited_driver("main-mono", |chain| {
        chain["chain"][2]["tone"]["models"] = serde_json::json!([
            { "id": 1103, "name": "Mic 1" }, { "id": 1203, "name": "Mic 2" }, { "id": 1303, "name": "Mic 3" }
        ]);
    });
    let _ = cab
        .click("Chain: UK Greenback - Marshall 1960TV 4x12 M201")
        .expect("cab button");
    cab.settle();
    assert!(!cab.is_visible("Captures"));
}

#[test]
fn the_edge_of_the_last_light_selects_it_instead_of_paging() {
    let (mut driver, backend) = amp_driver(9);
    let rect = rect_of(&driver, "Take 8").expect("last visible light");
    // The light's hit area reaches 4 pt past its visible edge on the right.
    let _ = driver.click_at(eframe::egui::pos2(rect.right() - 1.0, rect.center().y));
    driver.settle();
    assert_eq!(active_model(&backend, "blk-2"), Some(1108));
    assert_eq!(
        lights(&driver, 9).first().map(|(l, _)| l.as_str()),
        Some("Take 1")
    );
}

// ---------------------------------------------------------------- The skin picker

const AMP_SKINS: [&str; 9] = [
    "High-gain head",
    "US modern head",
    "British head",
    "Chime combo",
    "Clean combo",
    "Bass head",
    "Clean amp",
    "Crunch amp",
    "Hi-gain amp",
];

fn block_skin(backend: &MockBackend, block: &str) -> Option<String> {
    backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|each| each.id.0 == block)
        .and_then(|each| each.skin.clone())
}

fn open_skin_list(driver: &mut UiDriver) {
    let _ = driver.click("Skin").expect("skin chip");
    driver.settle();
}

#[test]
fn auto_shows_the_automatic_family_name() {
    let (driver, _) = amp_page();
    assert!(driver.is_visible("Skin"), "{:?}", driver.visible_labels());
    assert!(
        driver.shows_text("Auto \u{b7} Chime combo"),
        "the chip names the automatic family"
    );
    let (mut pedal, _) = stage_driver("main-mono");
    let _ = pedal
        .click("Chain: Green Russian Big Muff + BD-2")
        .expect("pedal");
    pedal.settle();
    assert!(pedal.label_starting_with("Skin").is_some());
    assert!(
        ronie_ui::default::skins::skins_in(ronie_core::gear::GearCategory::Pedal)
            .iter()
            .map(|id| format!(
                "Auto \u{b7} {}",
                ronie_ui::default::skins::friendly_name(id)
            ))
            .any(|text| pedal.shows_text(&text)),
        "a pedal's chip names a pedal family"
    );
}

#[test]
fn the_skin_chip_lists_the_category_and_auto() {
    let (mut driver, _) = amp_page();
    assert!(!driver.is_visible("Auto"), "the list starts closed");
    open_skin_list(&mut driver);
    let auto = driver
        .controls()
        .iter()
        .find(|control| control.label == "Auto")
        .cloned()
        .expect("Auto is listed");
    assert!(auto.checked, "Auto is the current choice");
    assert!(
        driver.shows_text("Chime combo"),
        "Auto names the family on its right"
    );
    for name in AMP_SKINS {
        assert!(driver.is_visible(name), "{name} is listed");
    }
    for name in ["Green overdrive", "Modern 4x12"] {
        assert!(!driver.is_visible(name), "{name} is not an amp");
    }
    let chip = rect_of(&driver, "Skin").expect("chip");
    let auto_rect = auto.rect;
    assert!(auto_rect.bottom() < chip.top(), "the list opens upward");
    let _ = driver.click("Skin").expect("close");
    driver.settle();
    assert!(!driver.is_visible("Auto"), "a second click closes it");
}

/// In a short window the list of ten amp skins ends above its chip and scrolls, so a click on
/// the chip still closes it.
#[test]
fn the_skin_list_ends_above_its_chip_in_a_short_window() {
    let (mut driver, backend) = amp_page();
    driver.set_window_size(eframe::egui::vec2(1024.0, 400.0));
    open_skin_list(&mut driver);
    let chip = rect_of(&driver, "Skin").expect("chip");
    let list = rect_of(&driver, "Skins").expect("the list is open");
    assert!(
        !list.intersects(chip) && list.bottom() < chip.top(),
        "the list {list:?} ends above the chip {chip:?}"
    );
    let last = |driver: &UiDriver| {
        rect_of(driver, "Hi-gain amp")
            .filter(|rect| list.contains_rect(*rect) && rect.height() > 20.0)
    };
    assert!(last(&driver).is_none(), "the last row starts out of view");
    driver.play(vec![
        eframe::egui::Event::PointerMoved(list.center()),
        eframe::egui::Event::MouseWheel {
            unit: eframe::egui::MouseWheelUnit::Point,
            delta: eframe::egui::vec2(0.0, -400.0),
            phase: eframe::egui::TouchPhase::Move,
            source: eframe::egui::MouseWheelSource::Wheel,
            modifiers: eframe::egui::Modifiers::NONE,
        },
    ]);
    driver.settle();
    assert!(last(&driver).is_some(), "the rows scroll to the last one");
    let _ = driver.click("Skin").expect("close");
    driver.settle();
    assert!(
        !driver.is_visible("Skins"),
        "the click on the chip closed it"
    );
    assert_eq!(block_skin(&backend, "blk-2"), None, "and picked nothing");
}

#[test]
fn the_skin_list_does_not_reopen_after_another_view() {
    let (mut driver, _) = amp_page();
    open_skin_list(&mut driver);
    assert!(driver.is_visible("Auto"));
    let detail = driver.chrome_mut().detail_mut().expect("detail");
    detail.set_view(BlockView::Eq);
    driver.settle();
    driver
        .chrome_mut()
        .detail_mut()
        .expect("detail")
        .set_view(BlockView::Tone);
    driver.settle();
    assert!(
        !driver.is_visible("Auto"),
        "back on GEAR the list is closed"
    );

    open_skin_list(&mut driver);
    assert!(driver.is_visible("Auto"));
    driver.chrome_mut().set_tuner(true);
    driver.settle();
    driver.chrome_mut().set_tuner(false);
    driver.settle();
    assert!(
        driver.is_visible("Skin") && !driver.is_visible("Auto"),
        "after the tuner had the stage the list is closed"
    );
}

#[test]
fn picking_a_skin_redraws_the_render_and_is_one_undo_step() {
    let (mut driver, backend) = amp_page();
    let before = rect_of(&driver, "Amp render").expect("render");
    open_skin_list(&mut driver);
    let _ = driver.click("High-gain head").expect("pick");
    driver.settle();
    assert_eq!(block_skin(&backend, "blk-2").as_deref(), Some("highgain"));
    assert!(!driver.is_visible("Auto"), "the list closes");
    assert!(driver.shows_text("High-gain head"), "the chip names it");
    let after = rect_of(&driver, "Amp render").expect("render");
    assert_ne!(before, after, "the head draws instead of the combo");

    open_skin_list(&mut driver);
    let checked: Vec<_> = driver
        .controls()
        .iter()
        .filter(|control| control.checked && AMP_SKINS.contains(&control.label.as_str()))
        .map(|control| control.label.clone())
        .collect();
    assert_eq!(checked, vec!["High-gain head".to_owned()]);
    let _ = driver.click("Skin").expect("close");
    driver.settle();

    let _ = driver.click("Undo").expect("undo");
    driver.settle();
    assert_eq!(
        block_skin(&backend, "blk-2"),
        None,
        "one undo is Auto again"
    );
    assert!(driver.shows_text("Auto \u{b7} Chime combo"));
    assert_eq!(rect_of(&driver, "Amp render"), Some(before));

    open_skin_list(&mut driver);
    let _ = driver.click("High-gain head").expect("pick");
    driver.settle();
    open_skin_list(&mut driver);
    let _ = driver.click("Auto").expect("auto");
    driver.settle();
    assert_eq!(block_skin(&backend, "blk-2"), None, "Auto clears it");
}

#[test]
fn an_override_of_another_category_falls_back_to_auto() {
    for skin in ["od-green", "cab-412-modern", "no-such-skin"] {
        let (mut driver, backend) = edited_driver("main-mono", |chain| {
            chain["chain"][1]["skinOverride"] = skin.into();
        });
        let _ = driver.click(VOX).expect("amp");
        driver.settle();
        assert!(driver.is_visible("Amp render"), "{skin}");
        assert!(
            driver.shows_text("Auto \u{b7} Chime combo"),
            "{skin} is ignored"
        );
        open_skin_list(&mut driver);
        assert!(
            driver
                .controls()
                .iter()
                .any(|control| control.label == "Auto" && control.checked),
            "{skin}"
        );
        let _ = driver.click("Auto").expect("auto");
        driver.settle();
        assert_eq!(
            block_skin(&backend, "blk-2"),
            None,
            "picking Auto clears the ignored {skin}"
        );
    }
    let (mut driver, _) = edited_driver("main-mono", |chain| {
        chain["chain"][1]["skinOverride"] = "british".into();
    });
    let _ = driver.click(VOX).expect("amp");
    driver.settle();
    assert!(driver.shows_text("British head"), "an amp skin is honoured");
}

// ---------------------------------------------------------------- Replace in place

const VOX_TITLE: &str = "'02 Vox AC30/6 Top Boost";

/// The Vox block as the backend holds it.
fn vox(backend: &MockBackend) -> ronie_core::ChainBlock {
    block_titled(backend, "AC30").expect("the Vox block")
}

/// The Vox tone of the fixture chain.
fn vox_tone() -> ronie_core::Tone {
    vox(&MockBackend::from_scenario(&scenario("main-mono")))
        .tone
        .expect("the Vox tone")
}

/// A signed-out driver over `library` with the Vox page on the stage, whose params changed
/// from the defaults: bypassed, more gain in, less mix, and its EQ on.
fn tuned_vox_page(library: &[ronie_core::Tone]) -> (UiDriver, Arc<MockBackend>) {
    use ronie_ui::backend::UiAction;
    let (mut driver, backend) = menu_driver("main-mono", false, library);
    let block = vox(&backend);
    let mut params = block.params.clone();
    params.enabled = false;
    params.input_gain = 0.8;
    params.output_gain = 0.3;
    params.mix = 0.6;
    params.eq.enabled = true;
    backend
        .dispatch(UiAction::SetBlockParams {
            block_id: block.id.clone(),
            params,
        })
        .expect("tune the Vox");
    driver.settle();
    let _ = driver.click(VOX).expect("amp");
    driver.settle();
    (driver, backend)
}

fn open_replace(driver: &mut UiDriver) {
    let _ = driver.click("Replace").expect("Replace");
    driver.settle();
}

/// The position of block `id` in the left lane.
fn index_of(backend: &MockBackend, id: &BlockId) -> Option<usize> {
    backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .position(|block| block.id == *id)
}

#[test]
fn replace_opens_the_menu_on_the_blocks_category() {
    let (mut driver, _) = amp_page();
    open_replace(&mut driver);
    assert!(
        driver.shows_text("REPLACE \u{b7} AMP"),
        "the menu opens on the amps"
    );
    assert!(driver.shows_text("keeps trims, EQ and place"));
    assert!(driver.is_visible("Search amps in your library"));
    assert!(!driver.is_visible("LANE A"), "a replace keeps the lane");
    assert!(
        driver.chrome_mut().browser_mut().is_none(),
        "the tone browser stays closed"
    );
    assert!(
        matches!(
            driver.chrome().gallery().selection(),
            GallerySelection::BlockDetail(_)
        ),
        "the block stays on the stage"
    );
    let replace = rect_of(&driver, "Replace").expect("the button");
    let search = rect_of(&driver, "Search amps in your library").expect("the field");
    assert!(
        search.top() > replace.bottom(),
        "the menu opens under the button"
    );

    open_replace(&mut driver);
    assert!(
        !driver.shows_text("keeps trims, EQ and place"),
        "a second click closes the menu"
    );
}

#[test]
fn back_keeps_replacing_the_block() {
    let (mut driver, _) = amp_page();
    open_replace(&mut driver);
    let _ = driver.click("Back to categories").expect("back");
    driver.settle();
    assert!(driver.is_visible("CAB"), "the categories show");
    assert!(driver.shows_text("keeps trims, EQ and place"));
    let _ = driver.click("CAB").expect("CAB");
    driver.settle();
    assert!(driver.shows_text("REPLACE \u{b7} CAB"));
}

#[test]
fn swapping_a_head_keeps_its_place_and_settings() {
    let (mut driver, backend) = tuned_vox_page(&[amp_tone(601, "Plexi Lead")]);
    let old = vox(&backend);
    let place = index_of(&backend, &old.id);
    open_replace(&mut driver);
    assert!(driver.shows_text("SWAP"));
    let _ = driver.click("Plexi Lead").expect("library row");
    driver.settle();

    let block = backend
        .chain_state(0)
        .chain
        .block(&old.id)
        .cloned()
        .expect("the block keeps its id");
    assert_eq!(
        block.tone.as_ref().map(|tone| tone.title.as_str()),
        Some("Plexi Lead")
    );
    assert_eq!(block.params, old.params, "trims, EQ and bypass stay");
    assert_eq!(
        index_of(&backend, &old.id),
        place,
        "the block keeps its place"
    );
    assert_eq!(lane_titles(&backend, false).len(), 3, "no block was added");
    assert!(
        !driver.shows_text("keeps trims, EQ and place"),
        "the menu closed"
    );
    assert!(
        matches!(driver.chrome().gallery().selection(), GallerySelection::BlockDetail(id) if *id == old.id),
        "the block stays on the stage"
    );
}

#[test]
fn one_undo_restores_the_replaced_block() {
    let (mut driver, backend) = tuned_vox_page(&[amp_tone(601, "Plexi Lead")]);
    let old = vox(&backend);
    open_replace(&mut driver);
    let _ = driver.click("Plexi Lead").expect("library row");
    driver.settle();
    driver.chrome_mut().undo().expect("undo");
    driver.settle();
    assert_eq!(
        backend.chain_state(0).chain.block(&old.id),
        Some(&old),
        "one undo brings back the old tone and its params"
    );
}

#[test]
fn the_current_tone_reads_current() {
    use ronie_ui::backend::UiAction;
    let (mut driver, backend) = tuned_vox_page(&[vox_tone(), amp_tone(603, "Not Loaded")]);
    open_replace(&mut driver);
    assert!(driver.shows_text("CURRENT"));
    assert!(driver.shows_text("SWAP"));
    assert!(!driver.shows_text("IN CHAIN") && !driver.shows_text("ADD"));
    let before = backend.actions().len();
    let _ = driver.click(VOX_TITLE).expect("the current row");
    driver.settle();
    assert!(
        !backend.actions()[before..]
            .iter()
            .any(|action| matches!(action, UiAction::LoadTone { .. })),
        "the current tone loads nothing"
    );
    assert!(
        driver.shows_text("keeps trims, EQ and place"),
        "the menu stays open"
    );
}

#[test]
fn replace_via_the_browser_and_a_file_keeps_settings() {
    let (mut driver, backend) = tuned_vox_page(&[]);
    let old = vox(&backend);
    open_replace(&mut driver);
    let _ = driver
        .click("OPEN THE TONE BROWSER")
        .expect("the browser link");
    driver.settle();
    assert!(
        driver.chrome_mut().browser_mut().is_some(),
        "the browser opened"
    );
    driver.chrome_mut().pick_tone(amp_tone(611, "Browsed Amp"));
    for _ in 0..4 {
        driver.settle();
    }
    let block = backend
        .chain_state(0)
        .chain
        .block(&old.id)
        .cloned()
        .expect("the block keeps its id");
    assert_eq!(
        block.tone.as_ref().map(|tone| tone.title.as_str()),
        Some("Browsed Amp")
    );
    assert_eq!(block.params, old.params, "the browser keeps the settings");
    assert_eq!(lane_titles(&backend, false).len(), 3, "no block was added");

    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("closet-cab.wav");
    write_ir(&path);
    driver
        .chrome_mut()
        .replace_with_file(&old.id.0, &path)
        .expect("the file loads");
    driver.settle();
    let block = backend
        .chain_state(0)
        .chain
        .block(&old.id)
        .cloned()
        .expect("the block keeps its id");
    assert!(block.tone.as_ref().is_some_and(|tone| tone.local));
    assert_eq!(block.params, old.params, "a file keeps the settings");
    assert_eq!(index_of(&backend, &old.id), Some(1));
}

#[test]
fn a_replace_drops_the_skin_override_and_ir_b() {
    use ronie_ui::backend::UiAction;
    let (mut driver, backend) = edited_driver("main-mono", |chain| {
        chain["chain"][2]["tone"]["models"] = serde_json::json!([
            { "id": 1103, "name": "M201" },
            { "id": 1105, "name": "SM57" },
        ]);
    });
    let cab = block_titled(&backend, "Greenback").expect("the cab");
    for action in [
        UiAction::SelectSecondModel {
            block_id: cab.id.clone(),
            model_id: Some(ronie_core::ModelId(1105)),
        },
        UiAction::SetBlockSkin {
            block_id: cab.id.clone(),
            skin: Some("cab-212-chime".to_owned()),
        },
    ] {
        backend.dispatch(action).expect("edit the cab");
    }
    let old = backend
        .chain_state(0)
        .chain
        .block(&cab.id)
        .cloned()
        .expect("cab");
    assert!(old.ir_b.is_some() && old.skin.is_some());
    driver.settle();
    let _ = driver
        .click("Chain: UK Greenback - Marshall 1960TV 4x12 M201")
        .expect("the cab");
    driver.settle();
    open_replace(&mut driver);
    assert!(driver.shows_text("REPLACE \u{b7} CAB"));
    let cab_tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 621, "title": "Closet 1x12", "format": "ir", "gear": "ir",
        "models": [{"id": 6210, "name": "m", "model_url": "https://fixtures.invalid/m/621.wav"}]
    }))
    .expect("tone");
    driver.chrome_mut().replace_tone(&cab.id.0, cab_tone);
    driver.settle();
    let block = backend
        .chain_state(0)
        .chain
        .block(&cab.id)
        .cloned()
        .expect("cab");
    assert_eq!(
        block.tone.as_ref().map(|tone| tone.title.as_str()),
        Some("Closet 1x12")
    );
    assert_eq!(block.ir_b, None, "the second IR belonged to the old tone");
    assert_eq!(block.skin, None, "the skin belonged to the old tone");
    assert_eq!(block.params, old.params);
    driver.chrome_mut().undo().expect("undo");
    driver.settle();
    assert_eq!(backend.chain_state(0).chain.block(&cab.id), Some(&old));
}

// ---------------------------------------------------------------- Visible block actions

/// Plays a double-click on a control.
fn double_click(driver: &mut UiDriver, label: &str) {
    let at = rect_of(driver, label)
        .unwrap_or_else(|| panic!("{label} is missing"))
        .center();
    let button = |pressed| eframe::egui::Event::PointerButton {
        pos: at,
        button: eframe::egui::PointerButton::Primary,
        pressed,
        modifiers: eframe::egui::Modifiers::NONE,
    };
    driver.play(vec![eframe::egui::Event::PointerMoved(at)]);
    for _ in 0..2 {
        driver.play(vec![button(true)]);
        driver.play(vec![button(false)]);
    }
    driver.settle();
}

fn checked(driver: &UiDriver, label: &str) -> bool {
    driver
        .controls()
        .iter()
        .find(|control| control.label == label)
        .unwrap_or_else(|| panic!("{label} is missing"))
        .checked
}

#[test]
fn the_page_shows_bypass_replace_and_remove_buttons() {
    let (mut driver, backend) = amp_page();
    let rects: Vec<_> = ["Bypass", "Replace", "Remove"]
        .into_iter()
        .map(|label| rect_of(&driver, label).unwrap_or_else(|| panic!("{label} is missing")))
        .collect();
    for rect in &rects {
        assert!(
            (rect.height() - 28.0).abs() < 0.5,
            "a pill is 28 pt tall: {rect:?}"
        );
    }
    assert!(
        rects[0].right() < rects[1].left() && rects[1].right() < rects[2].left(),
        "BYPASS, REPLACE, REMOVE from left to right"
    );
    assert!(
        (rects[1].left() - rects[0].right() - 8.0).abs() < 0.5,
        "8 pt between the pills"
    );
    for text in ["BYPASS", "REPLACE", "REMOVE"] {
        assert!(driver.shows_text(text), "the pill reads {text}");
    }
    assert!(
        !checked(&driver, "Bypass"),
        "an active block is not bypassed"
    );
    let _ = driver.click("Bypass").expect("Bypass");
    driver.settle();
    assert!(!vox(&backend).params.enabled);
    assert!(checked(&driver, "Bypass"), "BYPASS reads as selected");
    let _ = driver.click("Bypass").expect("Bypass");
    driver.settle();
    assert!(vox(&backend).params.enabled);
    assert!(!checked(&driver, "Bypass"));
}

#[test]
fn the_strip_menu_bypasses_replaces_duplicates_and_removes() {
    let (mut driver, backend) = stage_driver("main-mono");
    let count = |backend: &MockBackend, needle: &str| {
        backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .filter(|block| {
                block
                    .tone
                    .as_ref()
                    .is_some_and(|tone| tone.title.contains(needle))
            })
            .count()
    };
    let strip_action = |driver: &mut UiDriver, row: &str| {
        let _ = driver.secondary_click(VOX).expect("block button");
        driver.settle();
        let _ = driver.click(row).expect("menu row");
        driver.settle();
        assert!(!strip_menu_open(driver), "{row} closes the menu");
    };

    strip_action(&mut driver, "BYPASS");
    assert!(!vox(&backend).params.enabled, "BYPASS bypasses");
    strip_action(&mut driver, "BYPASS");
    assert!(vox(&backend).params.enabled, "BYPASS toggles");

    strip_action(&mut driver, "REPLACE\u{2026}");
    assert!(
        driver.shows_text("REPLACE \u{b7} AMP"),
        "REPLACE opens the replace menu on the amps"
    );
    assert!(driver.shows_text("keeps trims, EQ and place"));
    let icon = rect_of(&driver, VOX).expect("the icon");
    let search = rect_of(&driver, "Search amps in your library").expect("the field");
    assert!(
        search.top() > icon.bottom(),
        "the menu opens under the icon"
    );
    let _ = driver.press_key(eframe::egui::Key::Escape);
    assert!(!driver.shows_text("keeps trims, EQ and place"));

    strip_action(&mut driver, "REMOVE");
    assert_eq!(count(&backend, "AC30"), 0, "REMOVE removes the block");
    driver.chrome_mut().undo().expect("undo");
    driver.settle();
    assert_eq!(count(&backend, "AC30"), 1, "one undo brings it back");

    strip_action(&mut driver, "DUPLICATE");
    assert_eq!(count(&backend, "AC30"), 2, "DUPLICATE copies the block");
}

#[test]
fn removing_the_open_block_from_the_strip_opens_its_neighbour() {
    let (mut driver, backend) = amp_page();
    let _ = driver.secondary_click(VOX).expect("block button");
    driver.settle();
    let _ = driver.click("REMOVE").expect("REMOVE");
    driver.settle();
    assert!(block_titled(&backend, "AC30").is_none());
    assert!(
        matches!(
            driver.chrome().gallery().selection(),
            GallerySelection::BlockDetail(_)
        ),
        "a neighbour opens: {:?}",
        driver.chrome().gallery().selection()
    );
}

#[test]
fn double_click_still_bypasses() {
    let (mut driver, backend) = stage_driver("main-mono");
    double_click(&mut driver, VOX);
    assert!(!vox(&backend).params.enabled, "a double-click bypasses");
    assert!(!strip_menu_open(&driver), "a double-click opens no menu");
    // Half a second later, so that egui counts a new double-click and not a triple one.
    for _ in 0..30 {
        driver.frame();
    }
    double_click(&mut driver, VOX);
    assert!(
        vox(&backend).params.enabled,
        "a second double-click turns it on"
    );
}

#[test]
fn delete_removes_the_selected_block_and_undo_brings_it_back() {
    for key in [eframe::egui::Key::Delete, eframe::egui::Key::Backspace] {
        let (mut driver, backend) = amp_page();
        let old = vox(&backend);
        let _ = driver.press_key(key);
        assert!(
            block_titled(&backend, "AC30").is_none(),
            "{key:?} removes the open block"
        );
        assert!(
            matches!(
                driver.chrome().gallery().selection(),
                GallerySelection::BlockDetail(_)
            ),
            "a neighbour opens"
        );
        let others = backend.chain_state(0).chain.left.len();
        driver.chrome_mut().undo().expect("undo");
        driver.settle();
        assert_eq!(
            backend.chain_state(0).chain.block(&old.id),
            Some(&old),
            "one undo brings it back"
        );
        assert!(backend.chain_state(0).chain.left.len() >= others);
    }
}

#[test]
fn delete_while_typing_removes_nothing() {
    let (mut driver, backend) = menu_driver("main-mono", false, &[]);
    let _ = driver.click(VOX).expect("amp");
    driver.settle();
    let blocks = |backend: &MockBackend| backend.chain_state(0).chain.left.clone();
    let before = blocks(&backend);

    // The add menu's search field.
    open_amp(&mut driver);
    let _ = driver
        .fill("Search amps in your library", "vox")
        .expect("search");
    for key in [eframe::egui::Key::Backspace, eframe::egui::Key::Delete] {
        let _ = driver.press_key(key);
    }
    assert_eq!(
        blocks(&backend),
        before,
        "typing in the search removes nothing"
    );
    let _ = driver.press_key(eframe::egui::Key::Escape);
    let _ = driver.press_key(eframe::egui::Key::Escape);

    // The preset name.
    let _ = driver.click("Save preset").expect("save");
    driver.settle();
    let _ = driver.fill("Name", "Lead").expect("the name field");
    for key in [eframe::egui::Key::Backspace, eframe::egui::Key::Delete] {
        let _ = driver.press_key(key);
    }
    assert_eq!(
        blocks(&backend),
        before,
        "typing a preset name removes nothing"
    );
}

#[test]
fn delete_with_a_menu_open_removes_nothing() {
    let (mut driver, backend) = amp_page();
    open_menu(&mut driver);
    let _ = driver.press_key(eframe::egui::Key::Delete);
    assert!(
        block_titled(&backend, "AC30").is_some(),
        "the add menu is open"
    );
    let _ = driver.press_key(eframe::egui::Key::Escape);
    let _ = driver.secondary_click(VOX).expect("block button");
    driver.settle();
    let _ = driver.press_key(eframe::egui::Key::Delete);
    assert!(
        block_titled(&backend, "AC30").is_some(),
        "the strip menu is open"
    );
}

#[test]
fn right_click_on_plus_or_an_empty_slot_opens_no_menu() {
    let (mut driver, _) = slot_driver("main-mono", false, &[], add_overdrive);
    let _ = driver.secondary_click("Add a block").expect("plus");
    driver.settle();
    assert!(!strip_menu_open(&driver), "the + has no block menu");
    let _ = driver
        .secondary_click("Add a pedal after these")
        .expect("slot");
    driver.settle();
    assert!(!strip_menu_open(&driver), "a slot has no block menu");
}

// ---------------------------------------------------------------- Smart placement for +

const MUFF: &str = "Green Russian Big Muff + BD-2";
const GREENBACK: &str = "UK Greenback - Marshall 1960TV 4x12 M201";

/// Adds `tone` from the + menu, through its category row.
fn add_from_plus(driver: &mut UiDriver, row: &str, tone: &str) {
    open_menu(driver);
    let _ = driver.click(row).expect("category row");
    driver.settle();
    let _ = driver.click(tone).expect("library row");
    driver.settle();
}

#[test]
fn a_pedal_from_plus_lands_before_the_amp() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[library_pedal()], |_| {});
    add_from_plus(&mut driver, "PEDAL", "Library Drive");
    let titles = lane_titles(&backend, false);
    assert_eq!(titles[0], MUFF, "{titles:?}");
    assert_eq!(titles[1], "Library Drive", "{titles:?}");
    assert!(titles[2].contains("AC30"), "{titles:?}");
}

#[test]
fn an_amp_from_plus_lands_before_the_cab() {
    let (mut driver, backend) =
        slot_driver("main-mono", false, &[amp_tone(601, "Plexi Lead")], |_| {});
    add_from_plus(&mut driver, "AMP", "Plexi Lead");
    let titles = lane_titles(&backend, false);
    assert_eq!(titles.len(), 4, "{titles:?}");
    assert_eq!(titles[2], "Plexi Lead", "{titles:?}");
    assert_eq!(titles[3], GREENBACK, "{titles:?}");
}

#[test]
fn a_cab_from_plus_lands_after_the_amp() {
    let (mut driver, backend) = slot_driver(
        "main-mono",
        false,
        &[gear_tone(802, "Library Cab", "cab")],
        |_| {},
    );
    add_from_plus(&mut driver, "CAB", "Library Cab");
    let titles = lane_titles(&backend, false);
    assert!(titles[1].contains("AC30"), "{titles:?}");
    assert_eq!(titles[2], "Library Cab", "{titles:?}");
    assert_eq!(titles[3], GREENBACK, "{titles:?}");
}

#[test]
fn outboard_lands_at_the_end() {
    let (mut driver, backend) = slot_driver(
        "main-mono",
        false,
        &[gear_tone(803, "Library Comp", "outboard")],
        |_| {},
    );
    add_from_plus(&mut driver, "OUTBOARD", "Library Comp");
    let titles = lane_titles(&backend, false);
    assert_eq!(
        titles.last().map(String::as_str),
        Some("Library Comp"),
        "{titles:?}"
    );
}

#[test]
fn placement_works_in_lane_b() {
    let (mut driver, backend) = slot_driver("main-stereo", false, &[library_pedal()], |_| {});
    let left = lane_titles(&backend, false);
    open_menu(&mut driver);
    let _ = driver.click("LANE B").expect("lane B");
    driver.settle();
    let _ = driver.click("PEDAL").expect("PEDAL row");
    driver.settle();
    let _ = driver.click("Library Drive").expect("library row");
    driver.settle();
    let right = lane_titles(&backend, true);
    assert_eq!(right[0], "Library Drive", "{right:?}");
    assert_eq!(right[1], "Roland JC-40", "{right:?}");
    assert_eq!(lane_titles(&backend, false), left);
}

#[test]
fn a_pedal_run_slot_in_lane_b_stays_in_lane_b() {
    let library = [library_pedal(), gear_tone(803, "Second Drive", "pedal")];
    let (mut driver, backend) = slot_driver("main-stereo", false, &library, |_| {});
    // A pedal goes before lane B's amp, through the + menu.
    open_menu(&mut driver);
    let _ = driver.click("LANE B").expect("lane B");
    driver.settle();
    let _ = driver.click("PEDAL").expect("PEDAL row");
    driver.settle();
    let _ = driver.click("Library Drive").expect("library row");
    driver.settle();
    let left = lane_titles(&backend, false);
    assert_eq!(lane_titles(&backend, true)[0], "Library Drive");
    let _ = driver
        .click("Chain: Library Drive")
        .expect("the lane B pedal");
    driver.settle();
    let _ = driver.click("Add a pedal after these").expect("the slot");
    driver.settle();
    let _ = driver.click("Second Drive").expect("library row");
    driver.settle();
    let right = lane_titles(&backend, true);
    assert_eq!(right[0], "Library Drive", "{right:?}");
    assert_eq!(right[1], "Second Drive", "{right:?}");
    assert_eq!(right[2], "Roland JC-40", "{right:?}");
    assert_eq!(lane_titles(&backend, false), left);
}

#[test]
fn a_lane_without_gear_adds_at_the_end() {
    let keep_the_pedal = |chain: &mut Vec<serde_json::Value>| {
        chain.retain(|block| block["blockId"] != "blk-2" && block["blockId"] != "blk-3");
    };
    let library = [library_pedal(), gear_tone(802, "Library Cab", "cab")];
    for (row, title) in [("PEDAL", "Library Drive"), ("CAB", "Library Cab")] {
        let (mut driver, backend) = slot_driver("main-mono", false, &library, keep_the_pedal);
        assert_eq!(lane_titles(&backend, false), vec![MUFF.to_owned()]);
        add_from_plus(&mut driver, row, title);
        assert_eq!(
            lane_titles(&backend, false),
            vec![MUFF.to_owned(), title.to_owned()]
        );
    }
}

#[test]
fn the_menu_says_where_it_will_add() {
    let (mut driver, _) = slot_driver("main-mono", false, &[], |_| {});
    open_menu(&mut driver);
    assert!(driver.shows_text("placed by type"));
    assert!(!driver.shows_text("to the end of the chain"));
    for (row, text) in [
        ("PEDAL", "before '02 Vox AC30/6 To\u{2026}"),
        ("AMP", "before UK Greenback - Ma\u{2026}"),
        ("CAB", "after '02 Vox AC30/6 To\u{2026}"),
        ("OUTBOARD", "to the end of the chain"),
    ] {
        let _ = driver.click(row).expect("category row");
        driver.settle();
        assert!(driver.shows_text(text), "{row} should say {text}");
        let _ = driver.click("Back to categories").expect("back");
        driver.settle();
    }
}

#[test]
fn an_amp_page_has_no_side_slots() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver.click(VOX).expect("amp");
    driver.settle();
    for name in ["Add a pedal before the amp", "Add a cab after the amp"] {
        assert!(rect_of(&driver, name).is_none(), "{name}");
    }
    let (mut driver, _) = edited_driver("main-mono", |chain| {
        chain["chain"].as_array_mut().expect("chain").remove(2);
    });
    let _ = driver.click(VOX).expect("amp");
    driver.settle();
    for name in ["Add a pedal before the amp", "Add a cab after the amp"] {
        assert!(rect_of(&driver, name).is_none(), "{name} with no cab");
    }
}

#[test]
fn a_pedal_run_still_ends_with_a_slot() {
    let (driver, _) = pedalboard_driver();
    assert!(rect_of(&driver, "Add a pedal after these").is_some());
}

#[test]
fn the_browser_from_plus_places_by_the_tone_that_lands() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[], |_| {});
    open_menu(&mut driver);
    let _ = driver.click("PEDAL").expect("PEDAL row");
    driver.settle();
    let _ = driver.click("OPEN THE TONE BROWSER").expect("browser link");
    driver.settle();
    let chrome = driver.chrome_mut();
    chrome.pick_tone(local_pedal(905, "Browser Drive"));
    chrome.sync_browser();
    driver.settle();
    let titles = lane_titles(&backend, false);
    assert_eq!(titles[1], "Browser Drive", "{titles:?}");
}

#[test]
fn a_replace_never_moves_the_block() {
    let (mut driver, backend) = slot_driver("main-mono", false, &[library_pedal()], |_| {});
    let before = lane_titles(&backend, false);
    let id = block_titled(&backend, "AC30").expect("amp").id.0;
    driver
        .chrome_mut()
        .replace_tone(&id, gear_tone(804, "Replacement Pedal", "pedal"));
    driver.settle();
    let after = lane_titles(&backend, false);
    assert_eq!(after[1], "Replacement Pedal", "{after:?}");
    assert_eq!(after.len(), before.len());
}

// ---------------------------------------------------------------- Dark mode

use eframe::egui::{Color32, Shape};
use ronie_ui::shell::Appearance;

/// What the amp page of `main-mono` paints in Stage mode, in `appearance`, after `setup`
/// ran on the driver.
fn appearance_shapes(appearance: Appearance, setup: impl FnOnce(&mut UiDriver)) -> Vec<Shape> {
    let (mut driver, _) = amp_page();
    driver.chrome_mut().set_appearance(appearance);
    driver.settle();
    setup(&mut driver);
    // Enough frames for a menu's fade-in to end.
    for _ in 0..12 {
        driver.frame();
    }
    flat(driver.frame_shapes())
}

fn luminance(color: Color32) -> f32 {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    if a < 200 {
        return 0.0;
    }
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

fn rects(shapes: &[Shape]) -> Vec<eframe::egui::epaint::RectShape> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Rect(rect) if rect.brush.is_none() => Some(rect.clone()),
            _ => None,
        })
        .collect()
}

fn text_colors(shapes: &[Shape]) -> Vec<Color32> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Text(text) => Some(
                text.galley
                    .job
                    .sections
                    .iter()
                    .map(|section| section.format.color)
                    .chain(std::iter::once(text.fallback_color))
                    .chain(text.override_text_color)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}

const INK: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);

fn assert_no_light_panels(shapes: &[Shape], surface: &str) {
    for rect in rects(shapes) {
        let big = rect.rect.width() > 24.0 && rect.rect.height() > 16.0;
        assert!(
            !(big && luminance(rect.fill) > 0.85),
            "{surface}: a light panel {:?} at {:?} is painted on the dark window",
            rect.fill,
            rect.rect
        );
    }
    for color in text_colors(shapes) {
        assert!(
            ![
                INK,
                Color32::from_rgb(0x33, 0x33, 0x33),
                Color32::from_rgb(0x5f, 0x5f, 0x5c),
                Color32::from_rgb(0x6b, 0x6b, 0x68),
            ]
            .contains(&color),
            "{surface}: light-page text {color:?} on the dark window"
        );
    }
}

#[test]
fn dark_stage_paints_no_light_panels() {
    let shapes = appearance_shapes(Appearance::Dark, |_| {});
    assert!(
        rects(&shapes)
            .iter()
            .any(|rect| rect.fill == Color32::BLACK && rect.rect.width() > 900.0),
        "the window is black"
    );
    assert_no_light_panels(&shapes, "gear page");
    for tab in ["EQ", "INFO"] {
        let shapes = appearance_shapes(Appearance::Dark, |driver| {
            let _ = driver.click(tab).expect("tab");
            driver.settle();
        });
        assert_no_light_panels(&shapes, tab);
    }
    let shapes = appearance_shapes(Appearance::Dark, |driver| {
        let _ = driver.click("Skin").expect("skin chip");
        driver.settle();
    });
    assert_no_light_panels(&shapes, "skin list");
}

#[test]
fn dark_page_buttons_use_the_dark_tokens() {
    let dark = Look::stage_dark();
    let shapes = appearance_shapes(Appearance::Dark, |_| {});
    let pills = rects(&shapes);
    let pill = |rect: &&eframe::egui::epaint::RectShape| {
        (rect.rect.height() - 28.0).abs() < 0.5 && rect.rect.width() > 60.0
    };
    assert!(
        pills
            .iter()
            .filter(pill)
            .any(|rect| rect.fill == dark.panel),
        "the pills are filled #1c1c1e"
    );
    assert!(
        pills
            .iter()
            .filter(pill)
            .any(|rect| rect.stroke.color == dark.control_edge),
        "BYPASS and REPLACE have the dark control edge"
    );
    assert!(
        pills
            .iter()
            .filter(pill)
            .any(|rect| rect.stroke.color == dark.danger_edge),
        "REMOVE has the dark red edge"
    );
    assert!(
        text_colors(&shapes).contains(&dark.danger),
        "REMOVE reads in the dark red"
    );
    // A bypassed block's BYPASS turns solid white with black content.
    let shapes = appearance_shapes(Appearance::Dark, |driver| {
        let _ = driver.click("Bypass").expect("bypass");
        driver.settle();
    });
    assert!(
        rects(&shapes)
            .iter()
            .filter(pill)
            .any(|rect| rect.fill == dark.solid),
        "a pressed pill is solid white"
    );
}

#[test]
fn dark_menus_have_an_outline() {
    let dark = Look::stage_dark();
    let open = |driver: &mut UiDriver| {
        let _ = driver.click("Add a block").expect("the + button");
        driver.settle();
        assert!(
            driver.is_visible("OPEN THE TONE BROWSER"),
            "the menu opened"
        );
    };
    let shapes = appearance_shapes(Appearance::Dark, open);
    assert!(
        rects(&shapes).iter().any(|rect| rect.fill == dark.menu),
        "the menu board is #151517"
    );
    assert!(
        rects(&shapes).iter().any(|rect| rect.fill == dark.menu
            && rect.stroke.color == dark.control_edge
            && (rect.stroke.width - 1.0).abs() < 0.01),
        "the menu board has a 1 pt outline"
    );
    let shapes = appearance_shapes(Appearance::Light, open);
    let light = Look::stage();
    assert!(
        rects(&shapes).iter().any(|rect| rect.fill == light.menu
            && rect.stroke.width == 0.0
            && rect.rect.width() < 500.0),
        "the light menu stays #0b0b0b with no outline"
    );
}

#[test]
fn light_stage_is_unchanged() {
    let light = Look::stage();
    let shapes = appearance_shapes(Appearance::Light, |_| {});
    let rects = rects(&shapes);
    assert!(
        rects
            .iter()
            .any(|rect| rect.fill == light.background && rect.rect.width() > 900.0),
        "the light background"
    );
    assert!(
        rects.iter().any(|rect| rect.fill == light.panel
            && rect.stroke.color == Color32::from_rgb(0xcf, 0xcf, 0xcc)),
        "light page pills"
    );
    assert!(
        rects
            .iter()
            .any(|rect| rect.stroke.color == Color32::from_rgb(0xe4, 0xb4, 0xad)),
        "REMOVE keeps its light red edge"
    );
    let dark = Look::stage_dark();
    for rect in &rects {
        assert_ne!(rect.fill, dark.panel, "no dark panel in light mode");
        assert_ne!(rect.fill, dark.menu, "no dark menu in light mode");
    }
    assert!(
        text_colors(&shapes).contains(&INK),
        "light-page text stays #111"
    );
}

fn painted_text(shapes: &[Shape]) -> Vec<String> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

fn dark_mode_toggle(driver: &UiDriver) -> ronie_ui::driver::Control {
    driver
        .controls()
        .iter()
        .find(|control| control.label == "Dark mode")
        .cloned()
        .expect("the Dark mode toggle")
}

#[test]
fn the_utility_bar_offers_dark_mode() {
    let (mut driver, _) = stage_driver("main-mono");
    let toggle = dark_mode_toggle(&driver);
    assert!(!toggle.checked, "the toggle is off in light mode");
    let cpu = driver
        .controls()
        .iter()
        .find(|control| control.label == "Account")
        .map(|control| control.rect)
        .expect("account");
    assert!(
        toggle.rect.left() >= cpu.right(),
        "the toggle sits right of the account"
    );
    let text = painted_text(&driver.frame_shapes());
    assert!(
        text.iter().any(|text| text == "DARK"),
        "the toggle reads DARK"
    );
    assert!(!text.iter().any(|text| text == "LIGHT"));
}

#[test]
fn pressing_dark_mode_turns_the_window_dark() {
    let (mut driver, _) = stage_driver("main-mono");
    let _ = driver.click("Dark mode").expect("toggle");
    driver.settle();
    assert_eq!(Look::get(driver.context()), Look::stage_dark());
    assert!(dark_mode_toggle(&driver).checked, "the toggle is on");
    let shapes = flat(driver.frame_shapes());
    assert!(
        rects(&shapes)
            .iter()
            .any(|rect| rect.fill == Color32::BLACK && rect.rect.width() > 900.0),
        "the window is black"
    );
    let text = painted_text(&shapes);
    assert!(
        text.iter().any(|text| text == "LIGHT"),
        "the toggle reads LIGHT"
    );
    let _ = driver.click("Dark mode").expect("toggle");
    driver.settle();
    assert_eq!(Look::get(driver.context()), Look::stage());
    assert!(!dark_mode_toggle(&driver).checked);
}

#[test]
fn system_dark_is_followed_until_the_player_chooses() {
    use eframe::egui::Theme;
    let (mut driver, _) = stage_driver("main-mono");
    assert_eq!(driver.chrome().appearance(), Appearance::System);
    driver.set_system_theme(Some(Theme::Dark));
    driver.settle();
    assert_eq!(Look::get(driver.context()), Look::stage_dark());
    driver.set_system_theme(Some(Theme::Light));
    driver.settle();
    assert_eq!(
        Look::get(driver.context()),
        Look::stage(),
        "the look follows a change of the system"
    );
    driver.set_system_theme(Some(Theme::Dark));
    driver.settle();
    // The player picks light while the system is dark; the choice wins from now on.
    let _ = driver.click("Dark mode").expect("toggle");
    driver.settle();
    assert_eq!(driver.chrome().appearance(), Appearance::Light);
    assert_eq!(Look::get(driver.context()), Look::stage());
    driver.set_system_theme(Some(Theme::Dark));
    driver.settle();
    assert_eq!(Look::get(driver.context()), Look::stage());
}

#[test]
fn the_choice_survives_a_restart() {
    let directory = tempfile::tempdir().expect("settings root");
    let store = || {
        ronie_core::SettingsStore::standalone(directory.path(), ronie_core::UI_PREFERENCES_SETTINGS)
    };
    let (mut driver, _) = stage_driver("main-mono");
    driver.chrome_mut().use_settings(store());
    driver.settle();
    let _ = driver.click("Dark mode").expect("toggle");
    driver.settle();
    assert_eq!(driver.chrome().appearance(), Appearance::Dark);

    let (mut restarted, _) = stage_driver("main-mono");
    assert_eq!(restarted.chrome().appearance(), Appearance::System);
    restarted.chrome_mut().use_settings(store());
    restarted.settle();
    assert_eq!(restarted.chrome().appearance(), Appearance::Dark);
    assert_eq!(Look::get(restarted.context()), Look::stage_dark());

    let _ = restarted.click("Dark mode").expect("toggle");
    restarted.settle();
    let (mut again, _) = stage_driver("main-mono");
    again.chrome_mut().use_settings(store());
    assert_eq!(again.chrome().appearance(), Appearance::Light);
}

#[test]
fn scenarios_do_not_read_the_host_theme() {
    use eframe::egui::Theme;
    use ronie_ui::native_scenario::{NativeScenario, reach_scenario};
    let light = NativeScenario::new(&scenario("main-detail"), None, None).expect("scenario");
    assert_eq!(light.options.appearance, Some(Appearance::Light));
    let dark = NativeScenario::new(&scenario("main-detail-dark"), None, None).expect("scenario");
    assert_eq!(dark.options.appearance, Some(Appearance::Dark));

    let mut driver = reach_scenario(&scenario("main-detail")).expect("reached");
    driver.chrome_mut().set_view_mode(ViewMode::RonieFlow);
    driver.set_system_theme(Some(Theme::Dark));
    driver.settle();
    assert_eq!(
        Look::get(driver.context()),
        Look::stage(),
        "a dark host leaves a scenario light"
    );
    let mut driver = reach_scenario(&scenario("main-detail-dark")).expect("reached");
    driver.chrome_mut().set_view_mode(ViewMode::RonieFlow);
    driver.set_system_theme(Some(Theme::Light));
    driver.settle();
    assert_eq!(Look::get(driver.context()), Look::stage_dark());
}

#[test]
fn main_detail_dark_draws_the_dark_stage_layout_without_a_flag() {
    use ronie_ui::native_scenario::{NativeScenario, reach_scenario};
    let dark = scenario("main-detail-dark");
    let run = NativeScenario::new(&dark, None, None).expect("scenario");
    assert_eq!(run.options.view_mode, ViewMode::RonieFlow);
    let mut driver = reach_scenario(&dark).expect("reached");
    driver.settle();
    assert_eq!(Look::get(driver.context()), Look::stage_dark());

    let light = scenario("main-detail");
    let run = NativeScenario::new(&light, None, None).expect("scenario");
    assert_eq!(
        run.options.view_mode,
        ViewMode::ReferenceDefault,
        "a scenario with no layout keeps the reference layout"
    );
    assert_ne!(
        dark.state["description"], light.state["description"],
        "the dark scenario describes itself"
    );
}

#[test]
fn a_scenario_rejects_an_unknown_appearance_or_layout() {
    use ronie_ui::native_scenario::NativeScenario;
    for (key, value) in [("appearance", "dusk"), ("layout", "sideways")] {
        let mut broken = scenario("main-detail-dark");
        broken.state[key] = serde_json::Value::String(value.to_owned());
        let error = NativeScenario::new(&broken, None, None)
            .err()
            .unwrap_or_else(|| panic!("{key} {value:?} is rejected"));
        assert!(error.contains(value), "{error}");
    }
}

#[test]
fn a_dark_mode_choice_that_cannot_be_saved_says_so_and_still_applies() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (mut driver, _) = stage_driver("main-mono");
    // A directory at this path makes each read and each write fail.
    driver
        .chrome_mut()
        .use_settings(ronie_core::SettingsStore::new(directory.path()));
    driver.settle();
    let _ = driver.click("Dark mode").expect("toggle");
    driver.settle();
    assert_eq!(driver.chrome().appearance(), Appearance::Dark);
    assert_eq!(Look::get(driver.context()), Look::stage_dark());
    let toast = driver
        .chrome()
        .visible_toast()
        .expect("the failed save shows a toast");
    assert!(
        toast.starts_with("The dark mode choice was not saved: ") && !toast.contains("backend"),
        "the toast names the settings problem, not a backend: {toast}"
    );
}

#[test]
fn a_hovered_eq_band_chip_stands_out_in_dark() {
    let chips = |shapes: &[Shape]| -> Vec<Color32> {
        rects(shapes)
            .into_iter()
            .filter(|rect| {
                (rect.rect.width() - 22.0).abs() < 0.5 && (rect.rect.height() - 22.0).abs() < 0.5
            })
            .map(|rect| rect.fill)
            .collect()
    };
    let dark = Look::stage_dark();
    let shapes = appearance_shapes(Appearance::Dark, |driver| {
        let _ = driver.click("EQ").expect("tab");
        driver.settle();
        driver.hover("Select band 3").expect("band chip");
    });
    let fills = chips(&shapes);
    let resting: Vec<_> = fills.iter().filter(|fill| **fill != dark.accent).collect();
    assert!(resting.len() >= 2, "{fills:?}");
    assert!(
        resting.contains(&&dark.chip_hover),
        "the hovered chip is drawn in the hover colour: {fills:?}"
    );
    assert!(
        resting.contains(&&dark.fill_soft),
        "the other chips keep the resting colour: {fills:?}"
    );
}
