//! Full Default scenario inventory and visual-comparison contracts.

use eframe::egui::{Color32, ColorImage};
use ronie_core::ToneQuery;
use ronie_ui::{
    backend::MockBackend,
    default::{BlockView, CatalogStatus, FilterMenu, Overlay, SettingsTab},
    driver::UiDriver,
    native_scenario::{NativeScenario, reach_scenario},
    testbed::{UiScenario, UiSection, drive_scenario, load_scenarios},
    visual::{GLOBAL_MISMATCH_MAX, WORST_TILE_MAX, compare_images},
};

#[test]
fn visual_manifest_covers_every_default_scenario_exactly_once() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenario fixtures");
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/ui-visual-manifest.json"))
            .expect("visual manifest");
    let rows = manifest["scenarios"].as_array().expect("manifest rows");
    assert_eq!(scenarios.len(), 114);
    assert_eq!(rows.len(), scenarios.len());
    for scenario in scenarios {
        let matching = rows.iter().filter(|row| row["id"] == scenario.id).count();
        assert_eq!(
            matching, 1,
            "scenario {} must have one visual row",
            scenario.id
        );
        let row = rows
            .iter()
            .find(|row| row["id"] == scenario.id)
            .expect("visual row");
        assert_eq!(row["backend_effect_required"], true);
        assert_eq!(row["global_mismatch_max"], GLOBAL_MISMATCH_MAX);
        assert_eq!(row["worst_tile_64_max"], WORST_TILE_MAX);
    }
}

#[test]
fn comparison_counts_global_and_worst_tile_mismatches_and_writes_diffs() {
    let reference = ColorImage::new([128, 128], vec![Color32::BLACK; 128 * 128]);
    let mut candidate = reference.clone();
    candidate.pixels[0] = Color32::WHITE;
    let directory = tempfile::tempdir().expect("tempdir");
    let diff = directory.path().join("diff.png");
    let comparison = compare_images(&reference, &candidate, Some(&diff)).expect("comparison");
    assert_eq!(comparison.mismatched, 1);
    assert_eq!(comparison.mismatch_ratio, 1.0 / 16_384.0);
    assert_eq!(comparison.worst_tile_ratio, 1.0 / 4_096.0);
    assert_eq!(comparison.worst_tile, [0, 0]);
    assert!(diff.exists());
}

#[test]
fn comparison_rejects_different_dimensions_and_accepts_identical_images() {
    let one = ColorImage::new([64, 64], vec![Color32::BLACK; 64 * 64]);
    let other = ColorImage::new([32, 32], vec![Color32::BLACK; 32 * 32]);
    assert!(compare_images(&one, &other, None).is_err());
    let comparison = compare_images(&one, &one, None).expect("identical");
    assert_eq!(comparison.mismatched, 0);
    assert_eq!(comparison.worst_tile_ratio, 0.0);
}

/// The start of the contextual help that a scenario shows.
fn hint_start(id: &str) -> Option<&'static str> {
    Some(match id {
        "chrome-gate-deck" => "Gate:",
        "chrome-pitch-deck" | "chrome-pitch-tuning" | "chrome-pitch-tuning-bass5" => "Pitch Shift:",
        "chrome-pitch-optimize" | "chrome-pitch-optimize-off" | "chrome-pitch-optimize-delay" => {
            "OPTIMIZE:"
        }
        "chrome-spread-deck" | "chrome-spread-on" => "Offset:",
        "chrome-align-deck" => "Align:",
        "chrome-auto-balance" => "Auto Balance:",
        "chrome-input-mode" => "Input Mode:",
        "chrome-account-signed-in" | "chrome-account-signed-out" => "Account:",
        "browser-gear" | "browser-signed-out-gear" => "Gear:",
        "browser-search" | "browser-search-typed" => "Search:",
        "browser-profile-menu" => "Profile:",
        "browser-filters-expanded" => "Filters:",
        "browser-sort-menu" | "browser-zoom-menu" => "Sort:",
        "browser-sort-cleared" | "browser-sort-newest" | "browser-format-ir" => "Click the name:",
        "browser-format-menu" => "Format:",
        "browser-tags-menu" | "browser-tags-picked" => "Tags:",
        "browser-creators-menu" => "Creators:",
        "browser-calibrated-ir" => "Not for impulse responses:",
        "browser-filters-active-collapsed" => "Fewer filters:",
        "chrome-preset-save" => "Save Preset:",
        "chrome-preset-browse" => "Presets:",
        "chrome-tile-menu" | "chrome-toast-saved" => "'02 Vox AC30/6 Top Boost.",
        "main-detail-info" | "main-detail-info-signed-out" => "Info:",
        "main-detail-eq-sliders" => "EQ:",
        "main-detail-eq-curve" => "Curve:",
        _ if id.starts_with("main-tuner") => "Tuner:",
        _ => return None,
    })
}

/// What one scenario's name promises, read back from the running controller.
fn assert_named_state(scenario: &UiScenario, driver: &UiDriver) {
    let id = scenario.id.as_str();
    let chrome = driver.chrome();
    let shows = |labels: &[&str]| {
        for label in labels {
            assert!(
                driver.is_visible(label),
                "{id}: {label:?} is not on the screen: {:?}",
                driver.visible_labels()
            );
        }
    };

    match hint_start(id) {
        Some(start) => assert!(chrome.hint().starts_with(start), "{id}: {}", chrome.hint()),
        None => assert_eq!(chrome.hint(), "", "{id}"),
    }

    assert_eq!(chrome.tuner_visible(), id.starts_with("main-tuner"), "{id}");
    if id.starts_with("main-tuner") {
        shows(&["Close tuner"]);
    }
    assert_eq!(
        driver.shows_text("Sounds as D"),
        id == "main-tuner-shifted",
        "{id}: only a shifted signal names the note that it sounds as"
    );

    let detail = id.starts_with("main-detail") || id.starts_with("load-detail");
    assert_eq!(
        chrome.detail().map(|detail| detail.block().id.0.as_str()),
        detail.then_some("blk-2"),
        "{id}"
    );
    if let Some(block) = chrome.detail() {
        let expected = if id.starts_with("main-detail-info") {
            BlockView::Info
        } else if id.starts_with("main-detail-eq") {
            BlockView::Eq
        } else {
            BlockView::Tone
        };
        assert_eq!(block.view(), expected, "{id}");
    }
    match id {
        "main-detail-eq-sliders" => shows(&["Band 2 gain", "Curve"]),
        "main-detail-eq-curve" => shows(&["Band 2 frequency and gain", "Frequency"]),
        "main-detail-model-select" => shows(&["AC30-6 TB Brl 2", "AC30-6 TB Nrm 3"]),
        _ => {}
    }

    let settings_expected = scenario.section == UiSection::Settings;
    assert_eq!(chrome.settings().is_some(), settings_expected, "{id}");
    if let Some(settings) = chrome.settings() {
        let tab = if id.starts_with("settings-system") {
            SettingsTab::System
        } else {
            SettingsTab::Plugin
        };
        assert_eq!(settings.tab(), tab, "{id}");
        if id == "settings-midi-picker" {
            shows(&["Control to map", "Gate Hold", "Align Crossover"]);
        }
    }

    let overlay = match id {
        "chrome-account-signed-out" | "chrome-account-signed-in" => Overlay::Account,
        "chrome-preset-save" => Overlay::SavePreset,
        "chrome-preset-browse" => Overlay::PresetBrowser,
        _ => Overlay::None,
    };
    assert_eq!(chrome.overlay(), overlay, "{id}");
    match id {
        "chrome-account-signed-in" => shows(&["Logout", "Settings"]),
        "chrome-preset-save" => shows(&["Name", "Save"]),
        "chrome-preset-browse" => shows(&["Search presets"]),
        "chrome-gate-deck" => shows(&["Hold: 20 ms", "Range: 80 dB"]),
        "chrome-pitch-deck" => shows(&["Tonality: Off", "Buffer: 30 ms", "Tuning: E Standard"]),
        "chrome-pitch-tuning" => shows(&[
            "Instrument: Guitar 6-string",
            "Tuning: D Standard (-2)",
            "Pitch: -2 st",
        ]),
        "chrome-pitch-tuning-bass5" => shows(&[
            "Instrument: Bass 5-string",
            "Tuning: A Standard (-2)",
            "Tonality: Off",
        ]),
        "chrome-pitch-optimize-delay" => {
            shows(&[
                "Optimize settings",
                "Tuning: D Standard (-2)",
                "Buffer: 20 ms",
            ]);
            assert!(driver.shows_text("Delay: about 16 ms"), "{id}");
            let backend = ronie_ui::backend::MockBackend::from_scenario(scenario);
            assert!(ronie_ui::default::tuning::optimize(&backend), "{id}");
        }
        "chrome-pitch-optimize" | "chrome-pitch-optimize-off" => {
            assert!(
                !driver.shows_text("Delay: about 16 ms"),
                "{id}: no stream, so no delay"
            );
            let on = id == "chrome-pitch-optimize";
            shows(&["Optimize settings", "STEP", "Tuning: D Standard (-2)"]);
            shows(&[if on { "Buffer: 40 ms" } else { "Buffer: 30 ms" }]);
            let backend = ronie_ui::backend::MockBackend::from_scenario(scenario);
            assert_eq!(ronie_ui::default::tuning::optimize(&backend), on, "{id}");
        }
        "chrome-pitch-caption" => {
            shows(&["Pitch: -2 st"]);
            assert!(!driver.is_visible("Tonality: Off"), "{id}: no deck is open");
        }
        "chrome-spread-deck" => shows(&["Wobble: 25%", "Diffuse"]),
        "chrome-align-deck" => shows(&["Enable Align", "Diffuse"]),
        "chrome-input-mode" => shows(&["Left", "Right"]),
        "chrome-tile-menu" => shows(&["Copy", "Load File"]),
        _ => {}
    }
    let toast = match id {
        "chrome-toast-saved" => Some("Preset Saved"),
        "chrome-auto-balance" => Some("Listening"),
        _ => None,
    };
    assert_eq!(chrome.visible_toast(), toast, "{id}");

    let key_page = id.starts_with("chrome-key-setup");
    assert_eq!(chrome.key_page().is_some(), key_page, "{id}");
    if let Some(page) = chrome.key_page() {
        assert!(chrome.auth_active(), "{id}: the page replaces the chain");
        shows(&[
            "Back",
            "Open TONE3000",
            "TONE3000 key",
            "Save key",
            "Not now",
        ]);
        assert_eq!(
            page.error().is_some(),
            id == "chrome-key-setup-error",
            "{id}"
        );
    }
    assert_eq!(
        driver.is_visible("Remove key"),
        id == "settings-system-key-saved",
        "{id}"
    );
    if id == "settings-system-updates" {
        shows(&["Check for updates"]);
    }

    let signing_in = id == "browser-signed-out-signing-in"
        || (id.starts_with("load-oauth") && !id.ends_with("-returning"));
    if signing_in {
        assert!(chrome.auth_active(), "{id}");
    }
    match id {
        "load-oauth-leaving" | "load-oauth-browser-failed" => shows(&["Copy Link"]),
        "load-oauth-copied" => shows(&["Copied"]),
        "load-oauth-device-expired" => shows(&["New code"]),
        "load-oauth-error" => shows(&["Try again"]),
        "load-oauth-device" | "load-oauth-device-requesting" => {
            assert!(!driver.is_visible("Copy Link"), "{id}");
        }
        _ => {}
    }

    if scenario.section == UiSection::Banners {
        assert!(chrome.window_chrome().banner, "{id}");
    }
    assert_eq!(
        chrome.connection().problem().is_some(),
        id == "load-offline-modal",
        "{id}"
    );
    assert_eq!(
        chrome.updates().notice().is_some(),
        matches!(id, "load-update-notice" | "settings-plugin"),
        "{id}"
    );

    assert_eq!(
        chrome.browser().is_some(),
        scenario.section == UiSection::Browser,
        "{id}"
    );
    if let Some(browser) = chrome.browser() {
        let status = match id {
            "browser-loading" | "browser-signed-out-loading" => CatalogStatus::Loading,
            "browser-error" | "browser-signed-out-error" => CatalogStatus::Error,
            _ => CatalogStatus::Ready,
        };
        assert_eq!(browser.store().status(), status, "{id}");
        assert_eq!(
            browser.sign_in_page(),
            id == "browser-signed-out-card",
            "{id}"
        );
        assert_eq!(
            browser.pending_tone().is_some(),
            id == "browser-picking",
            "{id}"
        );
        match id {
            "browser-scrolled" => assert_eq!(browser.scroll(), 100.0),
            // The grid stops a request at the end of its content, and these results all fit.
            "browser-return" => assert_eq!(browser.scroll(), 0.0),
            "browser-signed-out-footer" => assert!(browser.scroll() > 100.0),
            _ => assert_eq!(browser.scroll(), 0.0, "{id}"),
        }
        let zoom = scenario.state["zoom"].as_f64().unwrap_or(1.0) as f32;
        assert_eq!(browser.zoom(), zoom, "{id}");
        let menu = match id {
            "browser-profile-menu" => Some(FilterMenu::Profile),
            "browser-sort-menu" | "browser-zoom-menu" => Some(FilterMenu::Sort),
            "browser-format-menu" => Some(FilterMenu::Format),
            "browser-tags-menu" | "browser-tags-picked" => Some(FilterMenu::Tags),
            "browser-creators-menu" => Some(FilterMenu::Creators),
            _ => None,
        };
        assert_eq!(browser.filter_menu(), menu, "{id}");
        if status == CatalogStatus::Ready {
            let signed_in = scenario.state["auth"].as_bool().unwrap_or(false);
            assert_eq!(browser.store().result_is_trending(), !signed_in, "{id}");
        }
        match id {
            "browser-search-typed" | "browser-profile-search" => {
                assert_eq!(browser.store().query().text, "vox", "{id}");
            }
            "browser-gear" | "browser-signed-out-gear" | "browser-return" => {
                assert_eq!(browser.store().query().gear, "pedal", "{id}");
            }
            "browser-signed-out-empty" => assert_eq!(browser.store().query().gear, "space"),
            "browser-verified" => assert!(browser.store().query().verified),
            "browser-profile-menu" => shows(&["Recently used", "Favorites"]),
            "browser-sort-menu" | "browser-zoom-menu" => shows(&["Newest", "Oldest"]),
            "browser-filters-expanded" => shows(&["Calibrated", "Makes"]),
            _ => {}
        }
    }
}

#[test]
fn every_scenario_reaches_its_named_state_with_no_window() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenario fixtures");
    assert_eq!(scenarios.len(), 114);
    for scenario in scenarios {
        let driver = reach_scenario(&scenario).unwrap_or_else(|error| panic!("{error}"));
        assert_named_state(&scenario, &driver);
    }
}

/// The surfaces that a click can open, in a form that two routes can compare.
#[derive(Debug, PartialEq)]
struct Surfaces {
    overlay: Overlay,
    tuner: bool,
    detail: Option<(String, BlockView)>,
    settings: Option<SettingsTab>,
    auth: bool,
    browser: Option<ToneQuery>,
}

fn surfaces(driver: &UiDriver) -> Surfaces {
    let chrome = driver.chrome();
    Surfaces {
        overlay: chrome.overlay(),
        tuner: chrome.tuner_visible(),
        detail: chrome
            .detail()
            .map(|detail| (detail.block().id.0.clone(), detail.view())),
        settings: chrome.settings().map(|settings| settings.tab()),
        auth: chrome.auth_active(),
        browser: chrome
            .browser()
            .map(|browser| browser.store().query().clone()),
    }
}

/// Scenarios whose capture state is not the state that their fixture drives reach.
const STEPS_DIFFER_FROM_INPUT: &[(&str, &str)] = &[
    (
        "load-offline-modal",
        "the mock stays online, so Add Tone opens the browser; the capture shows the modal alone",
    ),
    (
        "browser-sort-cleared",
        "the clear-filter drive reaches the backend and not the real filter chip",
    ),
    (
        "browser-signed-out-signed-in",
        "the capture shows the browser after sign-in; the drive stops on the sign-in page",
    ),
    (
        "browser-picking",
        "the mock finishes the load, so the browser closes; the capture holds the busy card",
    ),
    (
        "browser-profile-search",
        "the submit drive reaches the backend and not the real search field",
    ),
];

#[test]
fn real_input_opens_the_surfaces_that_the_steps_open() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenario fixtures");
    let mut compared = 0;
    for scenario in scenarios {
        let backend = MockBackend::from_scenario(&scenario);
        let Ok(Some(driven)) = drive_scenario(&scenario, backend) else {
            continue;
        };
        let reached = reach_scenario(&scenario).unwrap_or_else(|error| panic!("{error}"));
        let known = STEPS_DIFFER_FROM_INPUT
            .iter()
            .any(|(id, _)| *id == scenario.id);
        if known {
            assert_ne!(
                surfaces(&driven),
                surfaces(&reached),
                "{} now matches, so remove it from STEPS_DIFFER_FROM_INPUT",
                scenario.id
            );
        } else {
            assert_eq!(surfaces(&driven), surfaces(&reached), "{}", scenario.id);
            compared += 1;
        }
    }
    // 50 official scenarios, the two pitch decks with a tuning, the shifted tuner, and the three
    // pitch decks with OPTIMIZE on, off, and with the delay, and the key page.
    assert_eq!(compared, 57, "the scenarios with real input changed");
}

#[test]
fn artwork_cover_uv_preserves_aspect_with_a_centered_crop() {
    let uv = ronie_ui::default::tile::cover_uv([1600, 1000], [224, 224]);
    assert!((uv.min.x - 0.1875).abs() < 0.0001);
    assert_eq!(uv.min.y, 0.0);
    assert!((uv.max.x - 0.8125).abs() < 0.0001);
    assert_eq!(uv.max.y, 1.0);
    let tall = ronie_ui::default::tile::cover_uv([1000, 1600], [224, 224]);
    assert_eq!(tall.min.x, 0.0);
    assert!((tall.min.y - 0.1875).abs() < 0.0001);
    assert_eq!(tall.max.x, 1.0);
    assert!((tall.max.y - 0.8125).abs() < 0.0001);
}

#[test]
fn documented_visual_thresholds_match_the_reference_floor() {
    assert_eq!(GLOBAL_MISMATCH_MAX, 0.019);
    assert_eq!(WORST_TILE_MAX, 0.24);
}

#[test]
fn comparison_excludes_only_the_ronie_wordmark() {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-mono")
        .expect("mono scenario");
    let directory = tempfile::tempdir().expect("tempdir");
    let reference = directory.path().join("reference");
    let candidate = directory.path().join("candidate");
    std::fs::create_dir_all(&reference).expect("reference directory");
    std::fs::create_dir_all(&candidate).expect("candidate directory");
    let expected = image::RgbaImage::from_pixel(2048, 1228, image::Rgba([0, 0, 0, 255]));
    expected
        .save(reference.join("main-mono.png"))
        .expect("reference PNG");
    let mut actual = expected.clone();
    for y in 40..88 {
        for x in 48..368 {
            actual.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
        }
    }
    actual
        .save(candidate.join("main-mono.png"))
        .expect("brand-only PNG");
    let report = directory.path().join("report.json");
    ronie_ui::native_scenario::compare_suite(
        &reference,
        &candidate,
        &report,
        std::slice::from_ref(&scenario),
    )
    .expect("the Ronnie wordmark is an intentional branding difference");

    for y in 500..564 {
        for x in 1000..1064 {
            actual.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
        }
    }
    actual
        .save(candidate.join("main-mono.png"))
        .expect("control PNG");
    assert!(
        ronie_ui::native_scenario::compare_suite(&reference, &candidate, &report, &[scenario])
            .is_err(),
        "a changed control pixel must still fail"
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report).expect("report")).expect("JSON");
    assert_eq!(report["scenarios"][0]["mismatched"], 4096);
}

#[test]
fn a_scenario_with_no_reference_image_fails_and_the_report_lists_it() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .filter(|scenario| matches!(scenario.id.as_str(), "main-mono" | "main-empty"))
        .collect::<Vec<_>>();
    let directory = tempfile::tempdir().expect("tempdir");
    let reference = directory.path().join("reference");
    let candidate = directory.path().join("candidate");
    std::fs::create_dir_all(&reference).expect("reference directory");
    std::fs::create_dir_all(&candidate).expect("candidate directory");
    let image = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 255]));
    for scenario in &scenarios {
        image
            .save(candidate.join(format!("{}.png", scenario.id)))
            .expect("candidate PNG");
    }
    image
        .save(reference.join("main-mono.png"))
        .expect("reference PNG");
    let report_path = directory.path().join("report.json");

    let result =
        ronie_ui::native_scenario::compare_suite(&reference, &candidate, &report_path, &scenarios);

    assert!(
        result.is_err(),
        "a scenario with no reference does not pass"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).expect("report")).expect("JSON");
    let rows = report["scenarios"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "each scenario has a row");
    let missing = rows
        .iter()
        .find(|row| row["id"] == "main-empty")
        .expect("new scenario row");
    assert_eq!(missing["missing_reference"], true);
    assert_eq!(missing["passed"], false);
    let compared = rows
        .iter()
        .find(|row| row["id"] == "main-mono")
        .expect("compared row");
    assert_eq!(compared["passed"], true);
}

#[test]
fn detail_info_scenario_includes_the_tone_description() {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-detail-info")
        .expect("info scenario");
    let run = NativeScenario::new(&scenario, None, None).expect("native scenario");
    let backend = run.options.backend.expect("backend");
    let block = backend
        .chain_state(0)
        .chain
        .block(&ronie_core::BlockId("blk-2".to_owned()))
        .cloned()
        .expect("detail block");
    let description = block
        .tone
        .and_then(|tone| tone.raw["description"].as_str().map(str::to_owned))
        .expect("tone description");
    assert!(description.contains("Suhr Reactive Load"));
}

#[test]
fn detail_scenarios_open_the_saved_detail_block() {
    let scenarios =
        load_scenarios(include_str!("../../../fixtures/ui-scenarios.json")).expect("scenarios");
    for scenario in scenarios
        .into_iter()
        .filter(|scenario| scenario.id.starts_with("main-detail"))
    {
        let driver = reach_scenario(&scenario).expect("named state");
        assert_eq!(
            driver
                .chrome()
                .detail()
                .map(|detail| detail.block().id.0.as_str()),
            Some("blk-2"),
            "{}",
            scenario.id
        );
        let run = NativeScenario::new(&scenario, None, None).expect("native scenario");
        let backend = run.options.backend.expect("backend");
        assert!(
            backend
                .chain_state(0)
                .chain
                .block(&ronie_core::BlockId("blk-2".to_owned()))
                .is_some(),
            "{} must retain its detail block",
            scenario.id
        );
    }
}

#[test]
fn browser_catalog_uses_the_shared_fixture_order() {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "browser-search")
        .expect("browser scenario");
    let driver = reach_scenario(&scenario).expect("named state");
    let ids = driver
        .chrome()
        .browser()
        .expect("browser")
        .store()
        .result()
        .expect("catalog")
        .data
        .iter()
        .map(|tone| tone.id.0)
        .collect::<Vec<_>>();
    assert_eq!(ids, [101, 102, 103, 104, 105, 106]);
}

#[test]
fn model_picker_scenario_uses_the_complete_mock_page() {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-detail-model-select")
        .expect("model picker scenario");
    let run = NativeScenario::new(&scenario, None, None).expect("native scenario");
    let backend = run.options.backend.expect("backend");
    let block = backend
        .chain_state(0)
        .chain
        .block(&ronie_core::BlockId("blk-2".to_owned()))
        .cloned()
        .expect("detail block");
    let models = block.tone.expect("tone").models;
    assert_eq!(models.len(), 5);
    assert_eq!(models[0].id, ronie_core::ModelId(1101));
    assert_eq!(models[1].name, "AC30-6 TB Brl 2");
    assert_eq!(models[4].name, "AC30-6 TB Nrm 3");
}
