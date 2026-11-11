//! Browser query, filter, return-state, profile, and favorite tests.

use ronie_core::{Page, Tone, ToneId};
use ronie_ui::{
    default::{BrowserController, CatalogSeed, CatalogStore, FilterMenu, Profile},
    native_scenario::{ScenarioScript, Step},
    testbed::{UiScenario, load_scenarios},
};

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn tone() -> Tone {
    let backend = ronie_ui::backend::MockBackend::from_scenario(&scenario("main-detail"));
    ronie_ui::backend::UiBackend::chain_state(&backend, 0)
        .chain
        .left[0]
        .tone
        .clone()
        .expect("tone")
}

fn page() -> Page<Tone> {
    Page {
        data: vec![tone()],
        page: 1,
        page_size: 20,
        total: 43,
        total_pages: 3,
    }
}

#[test]
fn query_results_paginate_and_stale_pages_never_replace_newer_results() {
    let mut store = CatalogStore::new();
    let first = store.begin_query();
    let second = store.begin_query();
    assert!(second > first);
    assert!(!store.complete_query(first, page(), false));
    assert!(store.complete_query(second, page(), false));
    assert_eq!(store.result().expect("result").total, 43);
    assert!(store.next_page());
    let third = store.current_generation();
    let mut newer = page();
    newer.page = 2;
    assert!(store.complete_query(third, newer, false));
    assert_eq!(store.result().expect("result").page, 2);
    assert!(store.previous_page());
}

#[test]
fn signed_out_preview_is_gear_only_and_auth_changes_clear_cached_results() {
    let mut store = CatalogStore::new();
    store.set_gear("pedal");
    let generation = store.begin_query();
    assert!(store.complete_query(generation, page(), true));
    assert!(store.result_is_trending());
    store.drop_session_results();
    assert!(store.result().is_none());
    assert_eq!(store.query().gear, "pedal");
}

#[test]
fn profile_streams_park_catalog_filters_and_calibration_is_suppressed_for_ir() {
    let mut store = CatalogStore::new();
    store.set_profile(Profile::Downloaded);
    assert!(store.catalog_filters_parked());
    assert!(!store.filter_enabled(FilterMenu::Format));
    store.set_gear("cab");
    store.set_calibrated(true);
    assert!(!store.query().calibrated_applies());
    store.set_format("ir");
    assert!(!store.query().calibrated_applies());
    store.set_profile(Profile::None);
    assert!(!store.catalog_filters_parked());
}

#[test]
fn close_and_return_preserve_result_query_and_scroll_without_a_refetch() {
    let mut browser = BrowserController::new();
    let generation = browser.store_mut().begin_query();
    assert!(
        browser
            .store_mut()
            .complete_query(generation, page(), false)
    );
    browser.set_scroll(240.0);
    browser.close();
    let generation = browser.store().current_generation();
    browser.reopen();
    assert_eq!(browser.store().current_generation(), generation);
    assert_eq!(browser.scroll(), 240.0);
    assert!(browser.store().result().is_some());
}

#[test]
fn empty_error_and_retry_states_are_explicit() {
    let mut store = CatalogStore::new();
    let generation = store.begin_query();
    assert!(store.complete_query(
        generation,
        Page {
            data: Vec::new(),
            page: 1,
            page_size: 20,
            total: 0,
            total_pages: 0,
        },
        false,
    ));
    assert!(store.is_empty());
    let generation = store.begin_query();
    assert!(store.fail_query(generation, "offline"));
    assert_eq!(store.error(), Some("offline"));
    let retry = store.begin_query();
    assert!(retry > generation);
    assert_eq!(store.error(), None);
}

#[test]
fn favorite_failure_rolls_back_optimistic_count_and_state() {
    let mut store = CatalogStore::new();
    let generation = store.begin_query();
    assert!(store.complete_query(generation, page(), false));
    store.begin_favorite(ToneId(104), true);
    let updated = store.result().expect("result");
    assert_eq!(updated.data[0].raw["is_favorite"], true);
    assert_eq!(updated.data[0].raw["favorites_count"], 1);
    assert!(store.finish_favorite(ToneId(104), false));
    let updated = store.result().expect("result");
    assert!(
        !updated.data[0].raw["is_favorite"]
            .as_bool()
            .unwrap_or(false)
    );
    assert_eq!(
        updated.data[0].raw["favorites_count"].as_u64().unwrap_or(0),
        0
    );
}

fn tones(ids: &[i64]) -> Page<Tone> {
    Page {
        data: ids
            .iter()
            .map(|id| {
                Tone::from_value(serde_json::json!({
                    "id": id,
                    "title": format!("Tone {id}"),
                    "format": "nam",
                    "models": [],
                }))
                .expect("tone")
            })
            .collect(),
        page: 1,
        page_size: 12,
        total: ids.len() as u64,
        total_pages: 1,
    }
}

fn is_favorite(store: &CatalogStore, id: i64) -> bool {
    store
        .result()
        .and_then(|page| page.data.iter().find(|tone| tone.id == ToneId(id)))
        .is_some_and(|tone| tone.raw["is_favorite"] == true)
}

#[test]
fn favorites_of_two_tones_finish_independently() {
    let mut store = CatalogStore::new();
    let generation = store.begin_query();
    assert!(store.complete_query(generation, tones(&[1, 2]), false));

    assert!(store.begin_favorite(ToneId(1), true));
    assert!(store.begin_favorite(ToneId(2), true));
    assert!(store.finish_favorite(ToneId(1), false));

    assert!(!is_favorite(&store, 1), "the first write failed");
    assert!(is_favorite(&store, 2), "the second write is still active");
    assert!(store.finish_favorite(ToneId(2), true));
    assert!(is_favorite(&store, 2));
}

#[test]
fn a_second_favorite_on_a_tone_waits_for_the_first() {
    let mut store = CatalogStore::new();
    let generation = store.begin_query();
    assert!(store.complete_query(generation, tones(&[1]), false));

    assert!(store.begin_favorite(ToneId(1), true));
    assert!(
        !store.begin_favorite(ToneId(1), false),
        "one write per tone"
    );
    assert!(is_favorite(&store, 1));
    assert!(store.finish_favorite(ToneId(1), false));
    assert!(!is_favorite(&store, 1));
    assert!(store.begin_favorite(ToneId(1), true));
    assert!(!store.begin_favorite(ToneId(9), true), "no such tone");
}

#[test]
fn a_favorite_that_fails_after_a_newer_result_changes_nothing() {
    let mut store = CatalogStore::new();
    let generation = store.begin_query();
    assert!(store.complete_query(generation, tones(&[1]), false));
    assert!(store.begin_favorite(ToneId(1), true));

    let generation = store.begin_query();
    let mut newer = tones(&[1]);
    newer.data[0].raw["is_favorite"] = serde_json::json!(true);
    newer.data[0].raw["title"] = serde_json::json!("Newer");
    assert!(store.complete_query(generation, newer, false));

    assert!(!store.finish_favorite(ToneId(1), false));
    let tone = &store.result().expect("result").data[0];
    assert_eq!(tone.raw["title"], "Newer", "an old copy must not return");
    assert!(is_favorite(&store, 1));
}

#[test]
fn a_close_rolls_back_a_favorite_and_its_late_completion_changes_nothing() {
    let mut store = CatalogStore::new();
    let generation = store.begin_query();
    assert!(store.complete_query(generation, tones(&[1]), false));
    assert!(store.begin_favorite(ToneId(1), true));

    store.close();

    assert!(!is_favorite(&store, 1));
    assert!(!store.finish_favorite(ToneId(1), true));
    assert!(!is_favorite(&store, 1));
}

#[test]
fn a_server_page_of_zero_has_no_previous_page() {
    let mut store = CatalogStore::new();
    let generation = store.begin_query();
    let mut zero = page();
    zero.page = 0;
    assert!(store.complete_query(generation, zero, false));
    let generation = store.current_generation();

    assert!(!store.previous_page());

    assert_eq!(store.current_generation(), generation, "no request started");
    assert_eq!(store.result().expect("result").page, 0);
    assert!(store.next_page(), "the next page is still page one");
    assert_eq!(store.result().expect("result").page, 1);
}

#[test]
fn the_largest_server_page_has_no_next_page() {
    let mut store = CatalogStore::new();
    let generation = store.begin_query();
    let mut largest = page();
    largest.page = u64::MAX;
    largest.total_pages = u64::MAX;
    assert!(store.complete_query(generation, largest, false));
    let generation = store.current_generation();

    assert!(!store.next_page());

    assert_eq!(store.current_generation(), generation, "no request started");
}

#[test]
fn insert_selection_opens_browser_and_cached_tone_returns_to_gallery() {
    use ronie_ui::{backend::UiBackend, default::ChromeController};

    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "main-empty",
    )));
    let target = backend
        .chain_state(0)
        .chain
        .left
        .last()
        .expect("insert")
        .id
        .0
        .clone();
    let mut chrome = ChromeController::new(backend.clone(), Vec::new(), true, true);
    chrome.gallery_mut().activate(&target);
    chrome.sync_detail();
    chrome.sync_browser();
    assert!(chrome.browser().is_some());
    let generation = chrome.load_flow_mut().begin_load(tone()).expect("load");
    chrome.sync_browser();
    assert_eq!(
        chrome.browser().and_then(|browser| browser.pending_tone()),
        Some(tone().id)
    );
    assert!(chrome.load_flow_mut().complete_cached(generation, tone()));
    chrome.sync_browser();
    assert!(chrome.browser().is_none());
    assert!(
        backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .any(|block| block.tone.is_some())
    );
}

#[test]
fn every_browser_scenario_has_a_consistent_state_transition() {
    for scenario in load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .filter(|scenario| scenario.section == ronie_ui::testbed::UiSection::Browser)
    {
        let authenticated = scenario.state["auth"].as_bool().unwrap_or(false);
        let mut store = CatalogStore::new();
        let generation = store.begin_query();
        if scenario.state["api"]["search"] == "error" {
            assert!(store.fail_query(generation, "request failed"));
            assert!(store.error().is_some());
        } else if scenario.state["api"]["search"]["data"].is_array() {
            let total = scenario.state["api"]["search"]["total"]
                .as_u64()
                .unwrap_or(0);
            assert!(store.complete_query(
                generation,
                Page {
                    data: Vec::new(),
                    page: 1,
                    page_size: 20,
                    total,
                    total_pages: 0,
                },
                !authenticated,
            ));
            assert_eq!(store.is_empty(), total == 0);
        } else {
            assert!(store.complete_query(generation, page(), !authenticated));
            assert!(store.result().is_some());
        }
    }
}

#[test]
#[ignore = "native window capture"]
fn capture_browser_at_two_times_scale() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let capture =
        root.join(".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-9-browser.png");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "main-empty",
    )));
    let options =
        ronie_ui::shell::NativeShellOptions {
            selftest: true,
            capture: Some(capture.clone()),
            report: Some(root.join(
                ".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-9-browser.json",
            )),
            scale: Some(2.0),
            backend: Some(backend),
            standalone: false,
            presets: Vec::new(),
            authenticated: true,
            hints_enabled: true,
            ..Default::default()
        };
    ronie_ui::shell::run_scripted(
        options,
        ScenarioScript::new(vec![
            Step::OpenBrowser,
            Step::Catalog(CatalogSeed::Page {
                page: page(),
                trending: false,
            }),
        ]),
    )
    .expect("native browser capture");
    assert!(capture.exists());
}

fn driver_on_empty_chain() -> ronie_ui::driver::UiDriver {
    let scenario = scenario("main-empty");
    let backend = ronie_ui::backend::MockBackend::from_scenario(&scenario);
    ronie_ui::native_scenario::driver_for_scenario(&scenario, backend).expect("driver")
}

#[test]
fn back_arrow_closes_the_browser_and_add_tone_opens_it_again() {
    let mut driver = driver_on_empty_chain();
    let _ = driver.click_first("Add Tone").expect("first open");
    assert!(driver.chrome_mut().browser().is_some());

    let _ = driver.click("Close tone browser").expect("back arrow");
    driver.settle();
    assert!(driver.chrome_mut().browser().is_none());
    assert!(
        driver.is_visible("Add Tone"),
        "{:?}",
        driver.visible_labels()
    );

    let _ = driver.click_first("Add Tone").expect("second open");
    driver.settle();
    assert!(
        driver.chrome_mut().browser().is_some(),
        "a closed flow must not cancel the next open"
    );
}

#[test]
fn a_late_completion_after_the_back_arrow_changes_nothing() {
    let mut driver = driver_on_empty_chain();
    let _ = driver.click_first("Add Tone").expect("open");
    let generation = driver
        .chrome_mut()
        .load_flow_mut()
        .begin_load(tone())
        .expect("load");
    let _ = driver.click("Close tone browser").expect("back arrow");
    driver.settle();

    assert!(
        !driver
            .chrome_mut()
            .load_flow_mut()
            .complete_cached(generation, tone())
    );
    assert!(
        driver.is_visible("Add Tone"),
        "{:?}",
        driver.visible_labels()
    );
}

#[test]
fn add_tone_opens_again_after_a_tone_lands() {
    let mut driver = driver_on_empty_chain();
    let _ = driver.click_first("Add Tone").expect("first open");
    let generation = driver
        .chrome_mut()
        .load_flow_mut()
        .begin_load(tone())
        .expect("load");
    assert!(
        driver
            .chrome_mut()
            .load_flow_mut()
            .complete_cached(generation, tone())
    );
    driver.settle();
    assert!(driver.chrome_mut().browser().is_none());

    let _ = driver.click_first("Add Tone").expect("second open");
    driver.settle();
    assert!(
        driver.chrome_mut().browser().is_some(),
        "a finished load must not cancel the next open"
    );
}

#[test]
fn a_tile_opens_its_detail_after_a_tone_lands() {
    let mut driver = driver_on_empty_chain();
    let _ = driver.click_first("Add Tone").expect("open");
    let generation = driver
        .chrome_mut()
        .load_flow_mut()
        .begin_load(tone())
        .expect("load");
    assert!(
        driver
            .chrome_mut()
            .load_flow_mut()
            .complete_cached(generation, tone())
    );
    driver.settle();

    let _ = driver.click(&tone().title).expect("landed tile");
    driver.settle();
    assert!(
        driver.chrome_mut().detail().is_some(),
        "a finished load must not close a detail that opens later"
    );
}

#[test]
fn the_header_logo_returns_to_the_main_screen_from_the_browser() {
    let mut driver = driver_on_empty_chain();
    let _ = driver.click_first("Add Tone").expect("open");
    assert!(driver.chrome_mut().browser().is_some());

    let _ = driver.click("Back to main screen").expect("logo");
    driver.settle();
    assert!(driver.chrome_mut().browser().is_none());
    assert!(
        driver.is_visible("Add Tone"),
        "{:?}",
        driver.visible_labels()
    );

    let _ = driver.click_first("Add Tone").expect("second open");
    driver.settle();
    assert!(driver.chrome_mut().browser().is_some());
}

#[test]
fn the_header_logo_returns_to_the_main_screen_from_a_detail_and_keeps_the_chain() {
    let scenario = scenario("main-detail");
    let backend = ronie_ui::backend::MockBackend::from_scenario(&scenario);
    let mut driver =
        ronie_ui::native_scenario::driver_for_scenario(&scenario, backend).expect("driver");
    let title = tone().title;
    if driver.chrome_mut().detail().is_none() {
        let _ = driver.click(&title).expect("tile");
        driver.settle();
    }
    assert!(driver.chrome_mut().detail().is_some());

    let interaction = driver.click("Back to main screen").expect("logo");
    driver.settle();
    assert!(driver.chrome_mut().detail().is_none());
    assert!(driver.is_visible(&title), "{:?}", driver.visible_labels());
    assert!(
        interaction.effects.is_empty(),
        "going home must not change the chain: {:?}",
        interaction.effects
    );
}

#[test]
fn the_header_logo_returns_to_the_main_screen_from_the_tuner() {
    let mut driver = driver_on_empty_chain();
    driver.chrome_mut().set_tuner(true);
    driver.settle();
    assert!(driver.chrome_mut().tuner_visible());

    let _ = driver.click("Back to main screen").expect("logo");
    driver.settle();
    assert!(!driver.chrome_mut().tuner_visible());
}

#[test]
fn a_load_in_progress_does_not_land_after_the_header_logo() {
    let mut driver = driver_on_empty_chain();
    let _ = driver.click_first("Add Tone").expect("open");
    let generation = driver
        .chrome_mut()
        .load_flow_mut()
        .begin_load(tone())
        .expect("load");
    let _ = driver.click("Back to main screen").expect("logo");
    driver.settle();
    assert!(
        !driver
            .chrome_mut()
            .load_flow_mut()
            .complete_cached(generation, tone())
    );
    assert!(!driver.is_visible(&tone().title));
}

#[test]
fn the_browser_keeps_its_filter_after_the_back_arrow_and_after_the_logo() {
    for leave in ["Close tone browser", "Back to main screen"] {
        let mut driver = driver_on_empty_chain();
        let _ = driver.click_first("Add Tone").expect("open");
        let _ = driver.click("Pedal").expect("gear filter");
        driver.settle();
        let _ = driver.click(leave).expect("leave");
        driver.settle();
        assert!(driver.chrome_mut().browser().is_none(), "{leave}");

        let _ = driver.click_first("Add Tone").expect("return");
        driver.settle();
        let query = driver
            .chrome_mut()
            .browser()
            .expect("browser")
            .store()
            .query()
            .clone();
        assert_eq!(query.gear, "pedal", "{leave}");
    }
}
