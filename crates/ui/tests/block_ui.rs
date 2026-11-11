//! Block detail, model selection, metadata, calibration, and loading tests.

use std::sync::Arc;

use ronie_core::{BlockId, ModelId, Tone, ToneId, ToneModel};
use ronie_ui::{
    backend::{MockBackend, UiAction, UiBackend},
    default::{
        BlockDetail, BlockInfoPanel, BlockView, ChromeController, InfoPrompt, ModelOption,
        ModelSelect, TileVisualState,
    },
    driver::UiDriver,
    native_scenario::{ScenarioScript, Step},
    services::{
        ModelList, ModelSwitchFlow,
        session::SessionStatus::{NoService, SignedIn, SignedOut},
    },
    testbed::{UiScenario, load_scenarios},
};

/// A file URL of an absolute path on this system. A Windows path needs a drive.
fn file_url(path: &str) -> String {
    if cfg!(windows) {
        format!("file:///C:{path}")
    } else {
        format!("file://{path}")
    }
}

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn mock_backend(scenario: &UiScenario) -> Arc<MockBackend> {
    Arc::new(MockBackend::from_scenario(scenario))
}

fn options() -> Vec<ModelOption> {
    (1104..1110)
        .map(|id| ModelOption {
            id: ModelId(id),
            name: format!("Model {id}"),
        })
        .collect()
}

#[test]
fn model_steppers_and_dropdown_page_to_the_active_row() {
    let mut select = ModelSelect::new(false);
    select.set_options(options(), 6);
    select.set_active(ModelId(1104));
    assert!(!select.can_step(-1, SignedIn));
    assert!(select.can_step(1, SignedIn));
    assert_eq!(select.visible_range(), 0..5);
    assert_eq!(select.step(1, SignedIn), Some(ModelId(1105)));
    assert_eq!(select.active(), Some(ModelId(1105)));
    select.set_active(ModelId(1109));
    assert_eq!(select.visible_range(), 1..6);
    assert!(!select.can_step(1, SignedIn));
    select.open(SignedIn);
    assert!(select.is_open());
    select.select(ModelId(1106), SignedIn);
    assert_eq!(select.active(), Some(ModelId(1106)));
    assert!(!select.is_open());
}

#[test]
fn model_page_supersession_rejects_stale_and_failed_pages_without_losing_the_active_model() {
    let mut select = ModelSelect::new(false);
    select.set_active(ModelId(1104));
    let first = select.begin_fetch(ToneId(104));
    let second = select.begin_fetch(ToneId(104));
    assert!(second > first);
    assert!(!select.complete_fetch(first, ToneId(104), options(), 6));
    assert!(!select.complete_fetch(second, ToneId(105), options(), 6));
    assert!(select.complete_fetch(second, ToneId(104), options(), 6));
    assert_eq!(select.options().len(), 6);
    let third = select.begin_fetch(ToneId(104));
    assert!(!select.fail_fetch(second));
    assert!(select.fail_fetch(third));
    assert_eq!(select.active(), Some(ModelId(1104)));
    assert_eq!(select.options().len(), 6);
}

#[test]
fn catalog_picker_is_disabled_signed_out_but_local_full_list_switches_signed_out() {
    let mut select = ModelSelect::new(false);
    select.set_options(options(), 6);
    assert!(select.disabled(SignedOut));
    assert_eq!(select.select(ModelId(1105), SignedOut), None);

    let mut scenario = scenario("main-detail");
    let tone = &mut scenario.state["chain"]["chain"][0]["tone"];
    tone["local"] = serde_json::json!(true);
    tone["models"] = serde_json::json!(
        (1104..1110)
            .map(|id| serde_json::json!({
                "id": id,
                "name": format!("Local {id}"),
                "model_url": file_url(&format!("/tmp/local-{id}.nam"))
            }))
            .collect::<Vec<_>>()
    );
    let backend = mock_backend(&scenario);
    let mut detail = BlockDetail::new(backend.clone(), "blk-1", false).expect("detail");
    detail
        .select_model(ModelId(1109), SignedOut)
        .expect("local switch");
    let block = backend.chain_state(0).chain.left[0].clone();
    assert_eq!(block.active_model_id, Some(ModelId(1109)));
}

#[test]
fn a_deferred_picker_keeps_the_active_model_until_the_switch_lands() {
    let mut select = ModelSelect::new(false);
    select.set_options(options(), 6);
    select.set_active(ModelId(1104));
    select.set_deferred(true);

    assert_eq!(select.step(1, SignedIn), Some(ModelId(1105)));
    assert_eq!(select.active(), Some(ModelId(1104)));
    assert_eq!(select.target(), Some(ModelId(1105)));
    assert_eq!(
        select.step(1, SignedIn),
        Some(ModelId(1106)),
        "from the target"
    );
    assert_eq!(select.select(ModelId(1109), SignedIn), Some(ModelId(1109)));
    assert_eq!(select.active(), Some(ModelId(1104)));
    assert_eq!(select.target(), Some(ModelId(1109)));
    assert_eq!(select.visible_range(), 1..6, "the list shows the target");

    select.clear_target();
    assert_eq!(select.target(), None);
    assert_eq!(select.visible_range(), 0..5);

    assert_eq!(select.select(ModelId(1105), SignedIn), Some(ModelId(1105)));
    assert_eq!(select.select(ModelId(1104), SignedIn), Some(ModelId(1104)));
    assert_eq!(select.target(), None, "the active model needs no switch");
}

#[test]
fn a_page_for_one_picker_cannot_complete_the_fetch_of_another_picker() {
    let mut closed = ModelSelect::new(false);
    let mut next = ModelSelect::new(false);
    let old = closed.begin_fetch(ToneId(104));
    let new = next.begin_fetch(ToneId(104));

    assert_ne!(old, new);
    assert!(!next.complete_fetch(old, ToneId(104), options(), 6));
    assert!(next.loading());
    assert!(!next.fail_fetch(old));
    assert!(next.complete_fetch(new, ToneId(104), options(), 6));
}

fn catalog_model(id: i64) -> ToneModel {
    ToneModel::from_value(
        &serde_json::json!({
            "id": id,
            "name": format!("Model {id}"),
            "model_url": format!("https://models.example/{id}.nam"),
        }),
        false,
    )
    .expect("model")
}

fn model_list(ids: &[i64]) -> ModelList {
    ModelList {
        models: ids.iter().copied().map(catalog_model).collect(),
        total: ids.len() as u64,
    }
}

fn attached_detail(backend: &Arc<MockBackend>) -> BlockDetail {
    let shared: Arc<dyn UiBackend> = backend.clone();
    let mut detail = BlockDetail::new(shared.clone(), "blk-1", false).expect("detail");
    detail.attach_model_switching(ModelSwitchFlow::new(shared));
    detail
}

fn listed_detail(backend: &Arc<MockBackend>, ids: &[i64]) -> BlockDetail {
    let mut detail = attached_detail(backend);
    let request = detail.take_models_request(SignedIn).expect("a list is due");
    assert!(detail.complete_models(request.generation, request.tone, model_list(ids)));
    detail
}

#[test]
fn an_attached_detail_asks_for_its_models_one_time_and_only_signed_in() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = attached_detail(&backend);

    assert!(detail.take_models_request(SignedOut).is_none());
    let request = detail.take_models_request(SignedIn).expect("signed in");
    assert_eq!(request.tone, ToneId(104));
    assert!(detail.model_select().loading());
    assert!(detail.take_models_request(SignedIn).is_none(), "one time");

    assert!(detail.fail_models(request.generation));
    assert!(!detail.model_select().loading());
    assert_eq!(detail.model_select().active(), Some(ModelId(1104)));
    assert!(detail.take_models_request(SignedIn).is_none(), "no loop");
}

#[test]
fn a_model_list_keeps_the_active_model_that_the_service_omits() {
    let backend = mock_backend(&scenario("main-detail"));
    let detail = listed_detail(&backend, &[1105, 1106]);

    let ids = detail
        .model_select()
        .options()
        .iter()
        .map(|option| option.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, [ModelId(1104), ModelId(1105), ModelId(1106)]);
    assert_eq!(detail.model_select().active(), Some(ModelId(1104)));
}

#[test]
fn a_remote_selection_sends_no_action_until_its_bytes_arrive() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105]);
    let revision = backend.chain_state(0).revision;

    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let request = detail.take_switch_request().expect("a switch");

    assert!(detail.take_switch_request().is_none(), "one time");
    assert_eq!(request.model.id, ModelId(1105));
    assert_eq!(request.tone.id, ToneId(104));
    assert_eq!(backend.chain_state(0).revision, revision);
    assert_eq!(detail.model_select().active(), Some(ModelId(1104)));
    assert_eq!(detail.load_state(), TileVisualState::Loading);
}

#[test]
fn a_failed_switch_clears_the_choice_and_the_same_model_can_start_again() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105]);
    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let first = detail.take_switch_request().expect("a switch");
    let generation = first.ticket.generation();

    assert!(!detail.fail_switch(generation + 1), "not this switch");
    assert_eq!(detail.model_select().target(), Some(ModelId(1105)));
    assert!(detail.fail_switch(generation));
    assert_eq!(detail.model_select().target(), None);
    assert_eq!(detail.model_select().active(), Some(ModelId(1104)));
    assert_eq!(detail.load_state(), TileVisualState::Ready);

    detail.select_model(ModelId(1105), SignedIn).expect("retry");
    let second = detail.take_switch_request().expect("the retry");
    assert!(second.ticket.generation() > generation);
}

#[test]
fn a_second_choice_of_the_same_model_starts_no_second_download() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105]);
    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let _first = detail.take_switch_request().expect("a switch");

    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("the same selection");

    assert!(detail.take_switch_request().is_none());
    assert_eq!(detail.model_select().target(), Some(ModelId(1105)));
}

#[test]
fn a_choice_of_the_active_model_stops_the_switch() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105]);
    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let generation = detail
        .take_switch_request()
        .expect("a switch")
        .ticket
        .generation();

    detail
        .select_model(ModelId(1104), SignedIn)
        .expect("the active model");

    assert!(detail.take_switch_request().is_none());
    assert_eq!(detail.model_select().target(), None);
    assert_eq!(detail.load_state(), TileVisualState::Ready);
    assert!(!detail.finish_switch(generation), "the switch has ended");
}

#[test]
fn a_finished_switch_shows_the_new_model() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105]);
    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let request = detail.take_switch_request().expect("a switch");
    let tone = request.tone.clone();
    let mut tone = Tone {
        models: vec![request.model.clone()],
        ..tone
    }
    .for_storage(ModelId(1105))
    .expect("stored tone");
    tone.title = "Stored".to_owned();

    assert!(
        request
            .ticket
            .publish(tone, ModelId(1105), vec![1, 2, 3].into())
            .expect("publish")
    );
    assert!(detail.finish_switch(request.ticket.generation()));

    assert_eq!(detail.model_select().active(), Some(ModelId(1105)));
    assert_eq!(detail.model_select().target(), None);
    assert_eq!(detail.block().active_model_id, Some(ModelId(1105)));
    assert_eq!(detail.load_state(), TileVisualState::Ready);
}

#[test]
fn a_block_replacement_stops_the_switch_and_asks_for_the_list_of_the_new_tone() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105]);
    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let request = detail.take_switch_request().expect("a switch");
    let tone = Tone::from_value(serde_json::json!({
        "id": 900,
        "title": "Other",
        "format": "ir",
        "models": [{"id": 901, "name": "Other", "model_url": "https://models.example/901.wav"}],
    }))
    .expect("tone");
    backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: BlockId("blk-1".to_owned()),
            tone,
            model_id: ModelId(901),
        })
        .expect("replace");

    detail.sync();

    assert_eq!(detail.load_state(), TileVisualState::Ready);
    assert_eq!(detail.model_select().active(), Some(ModelId(901)));
    assert_eq!(detail.model_select().options().len(), 1);
    assert!(!detail.fail_switch(request.ticket.generation()));
    assert!(
        !request
            .ticket
            .publish(request.tone.clone(), ModelId(1105), vec![1].into())
            .expect("no publication"),
    );
    let next = detail.take_models_request(SignedIn).expect("a new list");
    assert_eq!(next.tone, ToneId(900));
}

#[test]
fn an_undo_during_a_switch_shows_the_model_that_the_chain_holds() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105, 1106]);
    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let request = detail.take_switch_request().expect("a switch");
    backend
        .dispatch(UiAction::SelectModel {
            block_id: BlockId("blk-1".to_owned()),
            model_id: ModelId(1106),
        })
        .expect("another active model");

    detail.sync();

    assert_eq!(detail.model_select().active(), Some(ModelId(1106)));
    assert_eq!(detail.model_select().target(), None);
    assert!(!detail.finish_switch(request.ticket.generation()));
}

#[test]
fn a_drop_of_the_detail_stops_its_switch() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105]);
    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let request = detail.take_switch_request().expect("a switch");

    drop(detail);

    assert!(
        !request
            .ticket
            .publish(request.tone.clone(), ModelId(1105), vec![1].into())
            .expect("no publication")
    );
}

#[test]
fn a_session_change_stops_the_model_work_and_asks_again_at_the_next_sign_in() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = listed_detail(&backend, &[1104, 1105]);
    detail
        .select_model(ModelId(1105), SignedIn)
        .expect("selection");
    let request = detail.take_switch_request().expect("a switch");

    detail.abandon_model_work();

    assert_eq!(detail.model_select().target(), None);
    assert!(detail.take_models_request(SignedOut).is_none());
    assert!(detail.take_models_request(SignedIn).is_some());
    assert!(
        !request
            .ticket
            .publish(request.tone.clone(), ModelId(1105), vec![1].into())
            .expect("no publication")
    );
}

#[test]
fn a_local_tone_on_an_attached_detail_asks_for_nothing_and_switches_at_once() {
    let mut scenario = scenario("main-detail");
    let tone = &mut scenario.state["chain"]["chain"][0]["tone"];
    tone["local"] = serde_json::json!(true);
    tone["models"] = serde_json::json!(
        (1104..1106)
            .map(|id| serde_json::json!({
                "id": id,
                "name": format!("Local {id}"),
                "model_url": file_url(&format!("/tmp/local-{id}.nam"))
            }))
            .collect::<Vec<_>>()
    );
    let backend = mock_backend(&scenario);
    let mut detail = attached_detail(&backend);

    assert!(detail.take_models_request(SignedIn).is_none());
    detail
        .select_model(ModelId(1105), SignedOut)
        .expect("local switch");

    assert!(detail.take_switch_request().is_none());
    assert_eq!(
        backend.chain_state(0).chain.left[0].active_model_id,
        Some(ModelId(1105))
    );
    assert_eq!(detail.model_select().active(), Some(ModelId(1105)));
}

#[test]
fn detail_controls_write_each_block_parameter_once() {
    use ronie_ui::backend::UiBackend;

    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = BlockDetail::new(backend.clone(), "blk-1", false).expect("detail");
    let initial = backend.chain_state(0).revision;
    detail.set_input_gain(0.7).expect("input");
    detail.set_output_gain(0.6).expect("output");
    detail.set_mix(0.4).expect("mix");
    detail.set_normalize(false).expect("normalize");
    detail.set_slim_full(true).expect("size");
    detail.set_enabled(false).expect("power");
    detail.set_eq_enabled(true).expect("eq power");
    detail.set_eq_pre(true).expect("eq pre");
    let block = backend.chain_state(0).chain.left[0].clone();
    assert_eq!(block.params.input_gain, 0.7);
    assert_eq!(block.params.output_gain, 0.6);
    assert_eq!(block.params.mix, 0.4);
    assert!(!block.params.normalize);
    assert_eq!(block.params.slim_size, 1.0);
    assert!(!block.params.enabled);
    assert!(block.params.eq.enabled && block.params.eq.pre);
    assert_eq!(backend.chain_state(0).revision, initial + 8);
}

#[test]
fn calibration_and_downstream_handoff_use_sane_metadata() {
    let mut calibrated = scenario("main-detail");
    calibrated.state["chain"]["chain"][0]["tone"]["output_level_dbu"] = serde_json::json!(-12.5);
    let backend = mock_backend(&calibrated);
    let detail = BlockDetail::new(backend, "blk-1", true).expect("detail");
    assert!(detail.nam_downstream());
    assert_eq!(detail.calibration_dbu(), Some(-12.5));
    assert!(detail.normalize_overridden());

    let mut invalid = scenario("main-detail");
    invalid.state["chain"]["chain"][0]["tone"]["output_level_dbu"] = serde_json::json!(90.0);
    let mock = mock_backend(&invalid);
    let detail = BlockDetail::new(mock, "blk-1", true).expect("detail");
    assert!(!detail.normalize_overridden());
}

#[test]
fn metadata_expands_and_signed_out_info_offers_a_login_and_public_link() {
    let scenario = scenario("main-detail");
    let backend = mock_backend(&scenario);
    let mut tone = backend.chain_state(0).chain.left[0]
        .tone
        .clone()
        .expect("tone");
    tone.raw["description"] = serde_json::json!(
        "A detailed capture with several useful lines of metadata that wraps at narrow widths and includes another complete sentence for the expanded view."
    );
    tone.raw["makes"] = serde_json::json!([" Example Make ", "", "Second Make"]);
    tone.raw["tags"] = serde_json::json!(["drive", "verified", "drive"]);
    let mut info = BlockInfoPanel::default();
    info.set_page_url("https://fixtures.invalid/tone/104");
    info.set_tone(Some(tone.clone()));
    let signed_out = info.layout(220, SignedOut);
    assert_eq!(signed_out.prompt, Some(InfoPrompt::SignIn));
    assert_eq!(signed_out.page_url, "https://fixtures.invalid/tone/104");
    let collapsed = info.layout(220, SignedIn);
    assert_eq!(collapsed.prompt, None);
    assert_eq!(info.layout(220, NoService), collapsed, "the details show");
    assert_eq!(collapsed.makes, vec!["Example Make", "Second Make"]);
    assert_eq!(collapsed.tags, vec!["drive", "verified"]);
    assert!(collapsed.show_more);
    info.toggle_more();
    let expanded = info.layout(220, SignedIn);
    assert!(expanded.description_lines >= collapsed.description_lines);
    assert!(expanded.height >= collapsed.height);
}

#[test]
fn detail_view_switch_share_and_remove_follow_the_same_backend() {
    use ronie_ui::backend::UiBackend;

    let backend = mock_backend(&scenario("main-detail"));
    let mut detail = BlockDetail::new(backend.clone(), "blk-1", false).expect("detail");
    detail.set_view(BlockView::Eq);
    assert_eq!(detail.view(), BlockView::Eq);
    detail.set_view(BlockView::Info);
    assert_eq!(detail.view(), BlockView::Info);
    detail.set_view(BlockView::Tone);
    assert!(detail.share_url().contains("green-russian"));
    detail.remove().expect("remove");
    assert!(detail.closed());
    let chain = backend.chain_state(0).chain;
    assert!(
        chain
            .block(&ronie_core::BlockId("blk-1".to_owned()))
            .is_none()
    );
    assert_eq!(chain.left[0].id.0, "blk-2", "the later tones move left");
}

/// The tone information page of a block whose tone has this page URL.
fn info_page_with_url(url: &str) -> UiDriver {
    let mut fixture = scenario("main-detail-info");
    fixture.state["chain"]["chain"][1]["tone"]["url"] = serde_json::Value::from(url);
    let backend = MockBackend::from_scenario(&fixture);
    let mut chrome = ChromeController::new(Arc::new(backend.clone()), Vec::new(), true, true);
    chrome.open_detail("blk-2").expect("detail");
    chrome
        .detail_mut()
        .expect("detail")
        .set_view(BlockView::Info);
    UiDriver::new(chrome, backend)
}

#[test]
fn the_view_on_ronnie_button_opens_a_web_page() {
    for url in [
        "https://www.fixtures.invalid/tones/1",
        "HTTP://www.fixtures.invalid/tones/1",
    ] {
        let mut driver = info_page_with_url(url);
        let opened = driver
            .click("View on TONE3000")
            .expect("button")
            .opened_urls;
        assert_eq!(opened, [url]);
    }
}

#[test]
fn a_tone_url_that_is_not_a_web_page_gets_no_button_and_no_share_link() {
    for url in [
        "file:///etc/passwd",
        "FILE:///etc/passwd",
        "javascript:alert(1)",
    ] {
        let driver = info_page_with_url(url);
        assert!(
            !driver.is_visible("View on TONE3000"),
            "{url}: {:?}",
            driver.visible_labels()
        );
        assert!(driver.is_visible("Tone info"), "the page is on the screen");
        let detail = driver.chrome().detail().expect("detail");
        assert_eq!(detail.share_url(), "", "{url}");
    }
}

#[test]
fn gallery_selection_opens_and_closes_the_detail_takeover() {
    let backend = mock_backend(&scenario("main-detail"));
    let mut chrome = ChromeController::new(backend, Vec::new(), true, true);
    chrome.gallery_mut().activate("blk-1");
    chrome.sync_detail();
    assert!(chrome.detail().is_some());
    chrome.detail_mut().expect("detail").set_view(BlockView::Eq);
    assert_eq!(chrome.detail().expect("detail").view(), BlockView::Eq);
    chrome
        .detail_mut()
        .expect("detail")
        .set_view(BlockView::Tone);
    chrome.detail_mut().expect("detail").close();
    chrome.sync_detail();
    assert!(chrome.detail().is_none());
}

#[test]
fn direct_open_detail_survives_the_next_sync() {
    use ronie_ui::default::ChromeController;

    let backend = mock_backend(&scenario("main-detail"));
    let mut chrome = ChromeController::new(backend, Vec::new(), true, true);
    chrome.open_detail("blk-1").expect("open detail");
    chrome.sync_detail();
    assert!(chrome.detail().is_some());
}

#[test]
fn failed_detail_retries_to_loading_and_loading_detail_is_explicit() {
    let backend = mock_backend(&scenario("load-detail-failed"));
    let mut detail = BlockDetail::new(backend, "blk-2", false).expect("detail");
    assert_eq!(detail.load_state(), TileVisualState::Failed);
    detail.retry().expect("retry");
    assert_eq!(detail.load_state(), TileVisualState::Loading);

    let mock = mock_backend(&scenario("load-detail-loading"));
    let detail = BlockDetail::new(mock, "blk-2", false).expect("detail");
    assert_eq!(detail.load_state(), TileVisualState::Loading);
}

/// How many dark layers one frame of a block page paints over a gear icon.
fn gear_overlays(scenario_id: &str, block: &str) -> usize {
    use eframe::egui;

    let backend = mock_backend(&scenario(scenario_id));
    let mut detail = BlockDetail::new(backend, block, false).expect("detail");
    let context = egui::Context::default();
    ronie_ui::fonts::Fonts::install(&context);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 614.0),
        )),
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| detail.show(ui, SignedIn));
    output.textures_delta.clear();
    output
        .shapes
        .iter()
        .filter(|clipped| {
            matches!(
                &clipped.shape,
                egui::Shape::Rect(rect) if rect.fill == egui::Color32::from_black_alpha(166)
            )
        })
        .count()
}

#[test]
fn a_gear_icon_stays_dark_behind_a_failed_or_loading_message() {
    assert_eq!(gear_overlays("load-detail-failed", "blk-2"), 1);
    assert_eq!(gear_overlays("load-detail-loading", "blk-2"), 1);
    assert_eq!(gear_overlays("main-detail", "blk-1"), 0);
}

#[test]
#[ignore = "native window capture"]
fn capture_detail_at_two_times_scale() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let capture =
        root.join(".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-8-detail.png");
    let backend = mock_backend(&scenario("main-detail"));
    let options =
        ronie_ui::shell::NativeShellOptions {
            selftest: true,
            capture: Some(capture.clone()),
            report: Some(root.join(
                ".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-8-detail.json",
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
        ScenarioScript::new(vec![Step::OpenDetail("blk-1".to_owned())]),
    )
    .expect("native detail capture");
    assert!(capture.exists());
}

#[test]
#[ignore = "native window capture"]
fn capture_eq_at_two_times_scale() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let capture =
        root.join(".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-8-eq.png");
    let backend = mock_backend(&scenario("main-detail-eq-sliders"));
    let options = ronie_ui::shell::NativeShellOptions {
        selftest: true,
        capture: Some(capture.clone()),
        report: Some(
            root.join(".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-8-eq.json"),
        ),
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
            Step::OpenDetail("blk-1".to_owned()),
            Step::DetailView(BlockView::Eq),
        ]),
    )
    .expect("native EQ capture");
    assert!(capture.exists());
}
