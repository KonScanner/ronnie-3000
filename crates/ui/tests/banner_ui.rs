//! Audio banners, window choreography, toast timing, and modal surfaces.

use std::collections::BTreeMap;

use eframe::egui::{Color32, ColorImage};
use ronie_ui::{
    accessibility::{AccessibleRole, Announcer},
    backend::{ChainFault, ChainFaultKind},
    default::{
        AudioDeviceState, BannerAction, BannerStore, BannerVariant, ModalKind, ModalScrim,
        ModalStack, ToastController, WindowStep, banner_window_transition,
    },
    driver::UiDriver,
    native_scenario::{ScenarioScript, Step},
    shell::Chrome,
    testbed::{UiScenario, load_scenarios},
};

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn device(id: &str) -> AudioDeviceState {
    AudioDeviceState::from_ui_value(&scenario(id).state["device"]).expect("device state")
}

#[test]
fn banner_store_defaults_to_an_empty_in_memory_store() {
    assert!(BannerStore::default().active().is_none());
}

#[test]
fn all_ten_banner_scenarios_select_the_expected_rule() {
    let cases = [
        (
            "banner-mic-denied",
            "Microphone access is off.",
            "Allow Access",
            BannerAction::OpenMicSettings,
            false,
        ),
        (
            "banner-no-input",
            "No audio input.",
            "Open Settings",
            BannerAction::OpenSettings,
            false,
        ),
        (
            "banner-disconnected",
            "No audio input.",
            "Open Settings",
            BannerAction::OpenSettings,
            false,
        ),
        (
            "banner-no-output",
            "No audio output.",
            "Open Settings",
            BannerAction::OpenSettings,
            false,
        ),
        (
            "banner-input-muted",
            "Input is muted.",
            "Open Settings",
            BannerAction::OpenSettings,
            false,
        ),
        (
            "banner-feedback-muted",
            "Input muted to prevent feedback.",
            "Open Settings",
            BannerAction::OpenSettings,
            false,
        ),
        (
            "banner-feedback-risk",
            "Feedback risk.",
            "Open Settings",
            BannerAction::OpenSettings,
            true,
        ),
        (
            "banner-asio",
            "Lower latency available.",
            "Switch to ASIO",
            BannerAction::SwitchToAsio,
            true,
        ),
        (
            "banner-buffer",
            "Noticeable delay?",
            "Open Settings",
            BannerAction::OpenSettings,
            true,
        ),
        (
            "banner-rate",
            "Running at 44.1 kHz.",
            "Open Settings",
            BannerAction::OpenSettings,
            true,
        ),
    ];
    for (id, headline, action_label, action, dismissable) in cases {
        let state = device(id);
        let mut store = BannerStore::new();
        store.evaluate(Some(&state), 0);
        let banner = store.active().expect("active banner");
        let rule_id = match id {
            "banner-mic-denied" => "mic-denied",
            "banner-no-input" | "banner-disconnected" => "no-input",
            "banner-no-output" => "no-output",
            "banner-input-muted" | "banner-feedback-muted" => "input-muted",
            "banner-feedback-risk" => "feedback-risk",
            "banner-asio" => "asio-nudge",
            "banner-buffer" => "buffer-latency",
            "banner-rate" => "rate-not-48k",
            _ => "unknown",
        };
        assert_eq!(banner.id, rule_id, "{id}");
        assert_eq!(banner.headline, headline, "{id}");
        assert_eq!(banner.action_label, action_label, "{id}");
        assert_eq!(banner.action, action, "{id}");
        assert_eq!(banner.dismissable, dismissable, "{id}");
    }
    let mut store = BannerStore::new();
    store.evaluate(Some(&device("banner-disconnected")), 0);
    assert!(
        store
            .active()
            .expect("banner")
            .body
            .contains("disconnected")
    );
    store.evaluate(Some(&device("banner-buffer")), 0);
    assert!(
        store
            .active()
            .expect("banner")
            .body
            .contains("about 43 ms with a buffer of 1024 samples")
    );
    store.evaluate(Some(&device("banner-rate")), 0);
    assert!(store.active().expect("banner").body.contains("48 kHz"));
}

#[test]
fn feedback_mute_precedes_risk_and_cannot_be_dismissed() {
    let state = device("banner-feedback-muted");
    let mut store = BannerStore::new();
    store.evaluate(Some(&state), 0);
    assert_eq!(store.active().expect("muted").id, "input-muted");
    assert!(!store.dismiss("input-muted", 0).expect("non-dismissable"));
    assert_eq!(store.active().expect("still muted").id, "input-muted");

    let risk = device("banner-feedback-risk");
    store.evaluate(Some(&risk), 0);
    assert_eq!(store.active().expect("risk").id, "feedback-risk");
}

#[test]
fn dismissible_banner_snooze_persists_for_twenty_four_hours() {
    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let state = device("banner-buffer");
    let mut store = BannerStore::with_settings(settings.clone());
    store.evaluate(Some(&state), 1_000);
    assert!(store.dismiss("buffer-latency", 1_000).expect("dismiss"));
    assert!(store.active().is_none());
    let properties = settings.read().expect("preferences");
    assert!(properties.get("t3k.dismissedBanners").is_some());

    let mut restored = BannerStore::with_settings(settings);
    restored.evaluate(Some(&state), 2_000);
    assert!(restored.active().is_none());
    restored.evaluate(Some(&state), 1_000 + 86_400_000);
    assert_eq!(restored.active().expect("expired").id, "buffer-latency");
}

#[test]
fn a_dismissal_in_another_process_reaches_a_visible_banner() {
    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let state = device("banner-buffer");
    let mut store = BannerStore::with_settings(settings.clone());
    store.evaluate(Some(&state), 1_000);
    assert_eq!(store.active().expect("banner").id, "buffer-latency");

    let mut other = BannerStore::with_settings(settings);
    other.evaluate(Some(&state), 1_000);
    assert!(other.dismiss("buffer-latency", 1_000).expect("dismiss"));

    // The time here is a frame clock. A slow machine needs more frames, not more of this time.
    let mut now = 1_000;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while store.active().is_some() {
        assert!(std::time::Instant::now() < deadline, "no read found it");
        std::thread::sleep(std::time::Duration::from_millis(1));
        now += 16;
        store.evaluate(Some(&state), now);
    }
}

#[test]
fn initial_window_banner_height_requires_an_active_rule() {
    assert!(!ronie_ui::shell::initial_banner_visible(&device(
        "settings-system"
    )));
    assert!(ronie_ui::shell::initial_banner_visible(&device(
        "banner-buffer"
    )));
}

#[test]
fn window_grows_before_banner_and_shrinks_after_it_hides() {
    let without = Chrome {
        banner: false,
        hints: true,
    };
    let with = Chrome {
        banner: true,
        hints: true,
    };
    assert_eq!(
        banner_window_transition(without, with, 2.0),
        vec![WindowStep::Resize(2048, 1316), WindowStep::ShowBanner]
    );
    assert_eq!(
        banner_window_transition(with, without, 2.0),
        vec![WindowStep::HideBanner, WindowStep::Resize(2048, 1228)]
    );
}

#[test]
fn toast_replaces_expires_pins_and_announces() {
    let mut toast = ToastController::default();
    toast.show("Preset Saved", 0);
    assert_eq!(toast.message(), Some("Preset Saved"));
    toast.show("Link Copied", 100);
    assert_eq!(toast.message(), Some("Link Copied"));
    assert_eq!(toast.message_at(1_899), Some("Link Copied"));
    assert_eq!(toast.message_at(1_900), None);
    toast.pin("Listening", 2_000);
    assert_eq!(toast.message_at(100_000), Some("Listening"));
    toast.clear();
    assert_eq!(toast.message(), None);
    assert_eq!(
        toast.drain_announcements(),
        vec!["Preset Saved", "Link Copied", "Listening"]
    );

    let mut announcer = Announcer::default();
    announcer.announce("Microphone access is off.");
    assert_eq!(announcer.drain(), vec!["Microphone access is off."]);
}

#[test]
fn modal_backdrop_blurs_at_half_resolution_on_the_ui_path() {
    let image = ColorImage::new([4, 4], vec![Color32::WHITE; 16]);
    let blurred = ModalScrim::blur_snapshot(&image);
    assert_eq!(blurred.size, [2, 2]);
    for pixel in blurred.pixels {
        assert_eq!(pixel.a(), 255);
        assert_eq!(pixel.r(), 255);
        assert_eq!(pixel.r(), pixel.g());
        assert_eq!(pixel.g(), pixel.b());
    }
    let mut scrim = ModalScrim::default();
    assert!(scrim.refresh_due(0));
    assert!(!scrim.refresh_due(124));
    assert!(scrim.refresh_due(125));
    let node = ModalScrim::accessible_node("Secure connection failed");
    assert_eq!(node.role, AccessibleRole::Dialog);
    assert_eq!(node.name, "Secure connection failed");
    node.validate().expect("accessible dialog");
}

#[test]
fn update_and_connection_modals_stack_and_block_background_keys() {
    let mut stack = ModalStack::default();
    stack.restack(true, true);
    assert_eq!(stack.z_order(), &[ModalKind::Update, ModalKind::Connection]);
    assert!(stack.blocks_background_input());
    stack.restack(false, true);
    assert_eq!(stack.z_order(), &[ModalKind::Connection]);
    stack.restack(false, false);
    assert!(stack.z_order().is_empty());
    assert!(!stack.blocks_background_input());
}

#[test]
fn a_device_fault_after_startup_reaches_the_banner_and_only_a_change_does_work() {
    let fixture = scenario("banner-no-input");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&fixture));
    let mut chrome =
        ronie_ui::default::ChromeController::new(backend.clone(), Vec::new(), false, true);
    let mut healthy = device("banner-no-input");
    healthy.device_open = true;
    healthy.input_device = "Guitar".to_owned();
    healthy.output_device = "Monitors".to_owned();
    healthy.has_active_input = true;
    let mut settings =
        ronie_ui::default::SystemDeviceState::from_ui_value(&fixture.state["device"])
            .expect("device settings");
    settings.banner = healthy.clone();
    backend.set_device_settings(Some(settings.clone()));
    let mut shown = Some(healthy);
    chrome.set_device_state(shown.clone(), 0);
    let _ = chrome.take_window_steps(2.0);
    assert!(chrome.banners().active().is_none());

    ronie_ui::shell::sync_device_banner(backend.as_ref(), &mut chrome, &mut shown, 10);
    assert!(chrome.take_window_steps(2.0).is_empty());
    assert!(chrome.banners().active().is_none());

    settings.banner.device_open = false;
    backend.set_device_settings(Some(settings));
    ronie_ui::shell::sync_device_banner(backend.as_ref(), &mut chrome, &mut shown, 20);
    assert_eq!(chrome.banners().active().expect("banner").id, "no-input");
    assert!(shown.as_ref().is_some_and(|device| !device.device_open));
}

fn tone_fault(kind: ChainFaultKind, block: &str, title: &str) -> ChainFault {
    ChainFault {
        kind,
        title: Some(title.to_owned()),
        identity: format!("{kind:?}:{block}"),
    }
}

fn banner_text(store: &BannerStore) -> Option<String> {
    store
        .active()
        .map(|banner| format!("{}{}", banner.headline, banner.body))
}

fn store_with(faults: Vec<ChainFault>) -> BannerStore {
    let mut store = BannerStore::new();
    store.set_chain_faults(faults);
    store.evaluate(None, 0);
    store
}

#[test]
fn each_chain_fault_has_fixed_text_that_names_the_block() {
    let cases = [
        (
            tone_fault(ChainFaultKind::ModelRejected, "a", "Green Muff"),
            "Green Muff cannot run. Ronnie bypasses this block.",
        ),
        (
            tone_fault(ChainFaultKind::MissingModel, "a", "Green Muff"),
            "Green Muff has no model data. Ronnie bypasses this block. Load the tone again.",
        ),
        (
            tone_fault(ChainFaultKind::NonFiniteOutput, "a", "Green Muff"),
            "Green Muff made invalid audio. Ronnie replaces it with silence.",
        ),
        (
            ChainFault::for_chain(ChainFaultKind::NonFiniteOutput),
            "The audio chain made invalid audio. Ronnie replaces it with silence.",
        ),
        (
            ChainFault::for_chain(ChainFaultKind::ChainNotPrepared),
            "The audio chain cannot update. Ronnie keeps the last chain that worked.",
        ),
        (
            tone_fault(ChainFaultKind::ProcessingFailed, "a", "Green Muff"),
            "Green Muff stopped working. Ronnie bypasses this block.",
        ),
    ];
    for (fault, text) in cases {
        let store = store_with(vec![fault]);
        assert_eq!(banner_text(&store).as_deref(), Some(text));
        let banner = store.active().expect("banner");
        assert_eq!(banner.variant, BannerVariant::Error);
        assert!(banner.dismissable);
        assert!(banner.id.starts_with("chain-fault:"), "{}", banner.id);
    }
}

#[test]
fn a_block_with_no_usable_title_still_gets_a_plain_name() {
    for title in [None, Some("   "), Some("\u{202e}\n\t")] {
        let fault = ChainFault {
            kind: ChainFaultKind::ModelRejected,
            title: title.map(str::to_owned),
            identity: "block-7:1:2".to_owned(),
        };
        assert_eq!(
            banner_text(&store_with(vec![fault])).as_deref(),
            Some("A block cannot run. Ronnie bypasses this block."),
        );
    }
}

#[test]
fn a_failed_block_with_no_title_gets_the_plain_name() {
    let fault = ChainFault {
        kind: ChainFaultKind::ProcessingFailed,
        title: None,
        identity: "block-7:1:2".to_owned(),
    };
    assert_eq!(
        banner_text(&store_with(vec![fault])).as_deref(),
        Some("A block stopped working. Ronnie bypasses this block."),
    );
}

#[test]
fn a_failed_block_follows_the_dismissal_rules_and_ends_with_its_fault() {
    let first = tone_fault(ChainFaultKind::ProcessingFailed, "a", "Green Muff");
    let second = tone_fault(ChainFaultKind::ProcessingFailed, "b", "Red Fuzz");
    let mut store = store_with(vec![first.clone()]);
    let id = store.active().expect("banner").id.clone();

    assert!(store.dismiss(&id, 1_000).expect("dismiss"));
    assert!(store.active().is_none(), "hidden for 24 hours");

    store.set_chain_faults(vec![first.clone(), second]);
    store.evaluate(None, 2_000);
    assert_eq!(
        banner_text(&store).as_deref(),
        Some("Red Fuzz stopped working. Ronnie bypasses this block."),
        "another block is another fault"
    );

    store.set_chain_faults(vec![first]);
    store.evaluate(None, 1_000 + 24 * 60 * 60 * 1000);
    assert_eq!(store.active().expect("banner").id, id, "the hiding ended");

    store.set_chain_faults(Vec::new());
    store.evaluate(None, 1_000 + 24 * 60 * 60 * 1000);
    assert!(store.active().is_none(), "a new chain ends the banner");
}

#[test]
fn a_tone_title_from_a_server_cannot_reshape_the_banner() {
    let fault = tone_fault(
        ChainFaultKind::ModelRejected,
        "a",
        "Green\n\u{202e}Muff\t\u{200b}  Deluxe",
    );
    assert_eq!(
        banner_text(&store_with(vec![fault])).as_deref(),
        Some("Green Muff Deluxe cannot run. Ronnie bypasses this block."),
    );

    let long = "x".repeat(500);
    let store = store_with(vec![tone_fault(ChainFaultKind::ModelRejected, "a", &long)]);
    let banner = store.active().expect("banner");
    assert_eq!(
        banner.headline,
        format!("{}… cannot run.", "x".repeat(40)),
        "the fixed words must stay on the one line of the strip"
    );
}

#[test]
fn the_banner_id_holds_only_the_fault_identity() {
    let block = ronie_core::ChainBlock::tone(
        "block-3",
        ronie_core::Tone::from_value(serde_json::json!({
            "id": 42,
            "title": "Green Muff",
            "format": "nam",
            "models": [{"id": 7, "name": "A"}, {"id": 8, "name": "B"}],
        }))
        .expect("tone"),
        ronie_core::ModelId(7),
    );
    let first = ChainFault::for_block(ChainFaultKind::ModelRejected, &block);
    assert_eq!(first.title.as_deref(), Some("Green Muff"));
    let store = store_with(vec![first.clone()]);
    let id = store.active().expect("banner").id.clone();
    assert!(!id.contains("Green"), "{id}");

    let mut other_model = block.clone();
    other_model.active_model_id = Some(ronie_core::ModelId(8));
    assert_ne!(
        first.identity,
        ChainFault::for_block(ChainFaultKind::ModelRejected, &other_model).identity,
        "another model in one block is another fault"
    );
    assert_ne!(
        first.identity,
        ChainFault::for_block(ChainFaultKind::NonFiniteOutput, &block).identity,
        "another problem in one block is another fault"
    );
}

#[test]
fn a_device_error_precedes_a_fault_and_a_fault_precedes_a_device_note() {
    let fault = || tone_fault(ChainFaultKind::ModelRejected, "a", "Green Muff");
    let mut store = BannerStore::new();
    store.set_chain_faults(vec![fault()]);

    store.evaluate(Some(&device("banner-no-input")), 0);
    assert_eq!(store.active().expect("banner").id, "no-input");

    store.evaluate(Some(&device("banner-input-muted")), 0);
    assert!(
        store
            .active()
            .expect("banner")
            .id
            .starts_with("chain-fault:")
    );
    store.evaluate(Some(&device("banner-buffer")), 0);
    assert!(
        store
            .active()
            .expect("banner")
            .id
            .starts_with("chain-fault:")
    );

    // The fault must also stay visible when the user has hidden a device note.
    store.set_chain_faults(Vec::new());
    store.evaluate(Some(&device("banner-buffer")), 0);
    assert!(store.dismiss("buffer-latency", 0).expect("dismiss"));
    assert!(store.active().is_none());
    store.set_chain_faults(vec![fault()]);
    store.evaluate(Some(&device("banner-buffer")), 0);
    assert!(
        store
            .active()
            .expect("banner")
            .id
            .starts_with("chain-fault:")
    );
}

#[test]
fn a_dismissed_fault_stays_hidden_and_the_device_note_behind_it_returns() {
    let mut store = BannerStore::new();
    store.set_chain_faults(vec![tone_fault(
        ChainFaultKind::ModelRejected,
        "a",
        "Green Muff",
    )]);
    store.evaluate(Some(&device("banner-input-muted")), 1_000);
    let id = store.active().expect("fault").id.clone();

    assert!(store.dismiss(&id, 1_000).expect("dismiss"));

    assert_eq!(store.active().expect("note").id, "input-muted");
    store.evaluate(Some(&device("banner-input-muted")), 2_000);
    assert_eq!(store.active().expect("note").id, "input-muted");
}

#[test]
fn a_dismissal_covers_one_fault_only_and_ends_after_one_day() {
    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let first = tone_fault(ChainFaultKind::ModelRejected, "a", "Green Muff");
    let second = tone_fault(ChainFaultKind::MissingModel, "b", "Red Fuzz");

    let mut store = BannerStore::with_settings(settings.clone());
    store.set_chain_faults(vec![first.clone(), second.clone()]);
    store.evaluate(None, 1_000);
    let first_id = store.active().expect("first").id.clone();
    assert!(store.dismiss(&first_id, 1_000).expect("dismiss"));

    assert_eq!(
        banner_text(&store).as_deref(),
        Some("Red Fuzz has no model data. Ronnie bypasses this block. Load the tone again."),
        "the next fault shows at once"
    );

    // A new launch reads the dismissal from the preferences.
    let mut restored = BannerStore::with_settings(settings.clone());
    restored.set_chain_faults(vec![first.clone()]);
    restored.evaluate(None, 2_000);
    assert!(restored.active().is_none());

    // The same fault goes away and comes back.
    restored.set_chain_faults(Vec::new());
    restored.evaluate(None, 3_000);
    restored.set_chain_faults(vec![first]);
    restored.evaluate(None, 4_000);
    assert!(restored.active().is_none());

    restored.evaluate(None, 1_000 + 86_400_000);
    assert_eq!(restored.active().expect("expired").id, first_id);
}

#[test]
fn old_fault_dismissals_do_not_collect_in_the_preferences() {
    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let mut store = BannerStore::with_settings(settings.clone());
    for (index, now) in [(1, 0), (2, 1_000), (3, 90_000_000)] {
        store.set_chain_faults(vec![tone_fault(
            ChainFaultKind::ModelRejected,
            &format!("block-{index}"),
            "Tone",
        )]);
        store.evaluate(None, now);
        let id = store.active().expect("fault").id.clone();
        assert!(store.dismiss(&id, now).expect("dismiss"));
    }
    let saved = settings.read().expect("preferences");
    let saved: BTreeMap<String, i64> =
        serde_json::from_str(saved.get("t3k.dismissedBanners").expect("saved")).expect("JSON");
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert!(saved.keys().all(|id| id.contains("block-3")), "{saved:?}");
}

fn fault_driver(faults: Vec<ChainFault>) -> (UiDriver, ronie_ui::backend::MockBackend) {
    let backend = ronie_ui::backend::MockBackend::from_scenario(&scenario("chrome-toast-saved"));
    backend.set_chain_faults(faults.clone());
    let mut chrome = ronie_ui::default::ChromeController::new(
        std::sync::Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
    );
    chrome.set_chain_faults(faults, 0);
    (UiDriver::new(chrome, backend.clone()), backend)
}

fn painted_text(driver: &mut UiDriver) -> String {
    let context = driver.context().clone();
    let chrome = driver.chrome_mut();
    let input = eframe::egui::RawInput {
        screen_rect: Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(1024.0, 658.0),
        )),
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| {
        eframe::egui::CentralPanel::default()
            .frame(eframe::egui::Frame::NONE)
            .show(ui, |ui| ronie_ui::default::show(ui, chrome));
    });
    output.textures_delta.clear();
    format!("{:?}", output.shapes)
}

#[test]
fn the_fault_banner_paints_in_the_banner_area_and_offers_only_ignore() {
    let (mut driver, _backend) = fault_driver(vec![tone_fault(
        ChainFaultKind::ModelRejected,
        "a",
        "Green Muff",
    )]);

    assert!(driver.chrome().window_chrome().banner);
    assert!(driver.is_visible("Ignore"), "{:?}", driver.visible_labels());
    assert!(!driver.is_visible("Open Settings"));
    assert!(
        driver
            .controls()
            .iter()
            .all(|control| !control.label.is_empty()),
        "the strip must not add a button with no name"
    );
    let painted = painted_text(&mut driver);
    assert!(painted.contains("Green Muff cannot run."), "{painted}");
    assert!(painted.contains(" Ronnie bypasses this block."));

    let accessibility = ronie_ui::shell::audit_chrome_accessibility(driver.chrome_mut());
    assert_eq!(accessibility["unnamed_focusable"], 0, "{accessibility}");
}

#[test]
fn ignore_hides_the_fault_through_real_input_and_a_new_fault_shows_again() {
    let first = tone_fault(ChainFaultKind::ModelRejected, "a", "Green Muff");
    let (mut driver, backend) = fault_driver(vec![first.clone()]);
    let _ = driver.chrome_mut().take_window_steps(1.0);
    let mut shown = vec![first.clone()];

    let _ = driver.click("Ignore").expect("ignore");

    assert!(driver.chrome().banners().active().is_none());
    assert!(!driver.chrome().window_chrome().banner);
    assert_eq!(
        driver.chrome_mut().take_window_steps(1.0),
        vec![WindowStep::HideBanner, WindowStep::Resize(1024, 614)]
    );
    assert!(!painted_text(&mut driver).contains("Green Muff"));

    // The same fault is still in the engine, and it stays hidden.
    ronie_ui::shell::sync_chain_faults(&backend, driver.chrome_mut(), &mut shown, 10);
    driver.frame();
    assert!(driver.chrome().banners().active().is_none());

    let second = tone_fault(ChainFaultKind::MissingModel, "b", "Red Fuzz");
    backend.set_chain_faults(vec![first, second]);
    ronie_ui::shell::sync_chain_faults(&backend, driver.chrome_mut(), &mut shown, 20);
    driver.settle();
    assert!(painted_text(&mut driver).contains("Red Fuzz has no model data."));
    assert_eq!(
        driver.chrome_mut().take_window_steps(1.0),
        vec![WindowStep::Resize(1024, 658), WindowStep::ShowBanner]
    );
}

#[test]
fn a_cleared_fault_removes_the_banner_and_gives_back_the_window_height() {
    let fault = tone_fault(ChainFaultKind::ModelRejected, "a", "Green Muff");
    let (mut driver, backend) = fault_driver(vec![fault.clone()]);
    let _ = driver.chrome_mut().take_window_steps(1.0);
    let mut shown = vec![fault];

    backend.set_chain_faults(Vec::new());
    ronie_ui::shell::sync_chain_faults(&backend, driver.chrome_mut(), &mut shown, 10);

    assert!(driver.chrome().banners().active().is_none());
    assert!(shown.is_empty());
    assert_eq!(
        driver.chrome_mut().take_window_steps(1.0),
        vec![WindowStep::HideBanner, WindowStep::Resize(1024, 614)]
    );
}

#[test]
fn a_sync_with_no_change_in_the_faults_does_no_work() {
    let fault = tone_fault(ChainFaultKind::ModelRejected, "a", "Green Muff");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "chrome-toast-saved",
    )));
    let mut chrome =
        ronie_ui::default::ChromeController::new(backend.clone(), Vec::new(), false, true);
    let mut shown = Vec::new();

    ronie_ui::shell::sync_chain_faults(backend.as_ref(), &mut chrome, &mut shown, 0);
    assert!(chrome.banners().active().is_none());
    assert!(chrome.take_window_steps(1.0).is_empty());

    backend.set_chain_faults(vec![fault.clone()]);
    ronie_ui::shell::sync_chain_faults(backend.as_ref(), &mut chrome, &mut shown, 0);
    assert_eq!(shown, [fault]);
    assert!(chrome.banners().active().is_some());
    assert_eq!(
        chrome.take_window_steps(1.0),
        vec![WindowStep::Resize(1024, 658), WindowStep::ShowBanner]
    );
    assert_eq!(chrome.drain_announcements().len(), 1, "one announcement");

    ronie_ui::shell::sync_chain_faults(backend.as_ref(), &mut chrome, &mut shown, 0);
    assert!(chrome.take_window_steps(1.0).is_empty());
    assert!(chrome.drain_announcements().is_empty());
}

#[test]
fn no_fault_leaves_the_banner_store_and_the_screen_as_they_were() {
    for id in ["banner-buffer", "banner-no-input", "settings-system"] {
        let state = device(id);
        let mut plain = BannerStore::new();
        plain.evaluate(Some(&state), 0);
        let mut empty = BannerStore::new();
        empty.set_chain_faults(Vec::new());
        empty.evaluate(Some(&state), 0);
        assert_eq!(plain.active(), empty.active(), "{id}");
    }

    let painted = |set_empty: bool| {
        let backend =
            ronie_ui::backend::MockBackend::from_scenario(&scenario("chrome-toast-saved"));
        let mut chrome = ronie_ui::default::ChromeController::new(
            std::sync::Arc::new(backend.clone()),
            Vec::new(),
            false,
            true,
        );
        if set_empty {
            chrome.set_chain_faults(Vec::new(), 0);
        }
        let mut driver = UiDriver::new(chrome, backend);
        assert!(!driver.chrome().window_chrome().banner);
        painted_text(&mut driver)
    };
    assert_eq!(painted(false), painted(true));
}

#[test]
fn a_device_banner_with_an_action_keeps_its_button() {
    let backend = ronie_ui::backend::MockBackend::from_scenario(&scenario("banner-buffer"));
    let mut chrome = ronie_ui::default::ChromeController::new(
        std::sync::Arc::new(backend.clone()),
        Vec::new(),
        false,
        true,
    );
    chrome.set_device_state(Some(device("banner-buffer")), 0);
    let driver = UiDriver::new(chrome, backend);
    assert!(driver.is_visible("Open Settings"));
    assert!(driver.is_visible("Ignore"));
}

#[test]
fn the_first_window_has_room_for_a_fault_banner_only_when_it_will_show() {
    let options =
        |backend: &ronie_ui::backend::MockBackend, settings| ronie_ui::shell::NativeShellOptions {
            backend: Some(std::sync::Arc::new(backend.clone())),
            update_settings: settings,
            ..Default::default()
        };
    let backend = ronie_ui::backend::MockBackend::from_scenario(&scenario("chrome-toast-saved"));
    assert!(!ronie_ui::shell::initial_window_has_banner(&options(
        &backend, None
    )));

    let fault = tone_fault(ChainFaultKind::ModelRejected, "a", "Green Muff");
    backend.set_chain_faults(vec![fault.clone()]);
    assert!(ronie_ui::shell::initial_window_has_banner(&options(
        &backend, None
    )));

    let temp = tempfile::tempdir().expect("temp");
    let settings = ronie_core::SettingsStore::new(temp.path().join("ui-preferences.settings"));
    let mut store = BannerStore::with_settings(settings.clone());
    store.set_chain_faults(vec![fault]);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64;
    store.evaluate(None, now);
    let id = store.active().expect("fault").id.clone();
    assert!(store.dismiss(&id, now).expect("dismiss"));
    assert!(!ronie_ui::shell::initial_window_has_banner(&options(
        &backend,
        Some(settings)
    )));
}

#[test]
#[ignore = "native window capture"]
fn capture_chain_fault_banner_at_two_times_scale() {
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "main-mono",
    )));
    backend.set_chain_faults(vec![tone_fault(
        ChainFaultKind::ModelRejected,
        "a",
        "Green Muff",
    )]);
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let capture = directory.join("chain-fault-banner.png");
    let options = ronie_ui::shell::NativeShellOptions {
        selftest: true,
        capture: Some(capture.clone()),
        report: Some(directory.join("chain-fault-banner.json")),
        scale: Some(2.0),
        backend: Some(backend),
        authenticated: true,
        ..Default::default()
    };
    ronie_ui::shell::run_scripted(options, ScenarioScript::new(Vec::new()))
        .expect("native banner capture");
    assert_eq!(
        image::image_dimensions(capture).expect("capture dimensions"),
        (2048, 1316)
    );
}

#[test]
fn chrome_banner_drives_resize_steps_and_backend_settings_actions() {
    let fixture = scenario("banner-no-input");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&fixture));
    let mut chrome =
        ronie_ui::default::ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.set_device_state(Some(device("banner-no-input")), 0);
    assert_eq!(chrome.banners().active().expect("banner").id, "no-input");
    assert_eq!(
        chrome.take_window_steps(2.0),
        vec![WindowStep::Resize(2048, 1316), WindowStep::ShowBanner]
    );
    chrome
        .handle_banner_view_action(
            ronie_ui::default::BannerViewAction::Action(BannerAction::OpenSettings),
            0,
        )
        .expect("open settings");
    assert_eq!(
        chrome.settings().expect("settings page").tab(),
        ronie_ui::default::SettingsTab::System
    );

    let asio_backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(
        &scenario("banner-asio"),
    ));
    let mut asio_chrome =
        ronie_ui::default::ChromeController::new(asio_backend.clone(), Vec::new(), false, true);
    asio_chrome
        .handle_banner_view_action(
            ronie_ui::default::BannerViewAction::Action(BannerAction::SwitchToAsio),
            0,
        )
        .expect("switch backend");
    assert!(
        asio_backend
            .actions()
            .contains(&ronie_ui::backend::UiAction::Device(
                ronie_ui::backend::DeviceAction::SelectBackend("ASIO".to_owned()),
            ),)
    );

    let mic_backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(
        &scenario("banner-mic-denied"),
    ));
    let mut mic_chrome =
        ronie_ui::default::ChromeController::new(mic_backend.clone(), Vec::new(), false, true);
    mic_chrome
        .handle_banner_view_action(
            ronie_ui::default::BannerViewAction::Action(BannerAction::OpenMicSettings),
            0,
        )
        .expect("microphone privacy");
    assert!(
        mic_backend
            .actions()
            .contains(&ronie_ui::backend::UiAction::Navigate(
                ronie_ui::backend::NavigationAction::OpenMicrophonePrivacy,
            ),)
    );
}

#[test]
fn chrome_toast_expires_through_the_shared_clock_path() {
    let scenario = scenario("chrome-toast-saved");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario));
    let mut chrome = ronie_ui::default::ChromeController::new(backend, Vec::new(), false, true);
    chrome.open_save();
    chrome.set_draft("Stadium Lead");
    chrome.save().expect("save");
    chrome.sync_toast(0);
    assert_eq!(chrome.visible_toast(), Some("Preset Saved"));
    chrome.sync_toast(1_799);
    assert_eq!(chrome.visible_toast(), Some("Preset Saved"));
    chrome.sync_toast(1_800);
    assert_eq!(chrome.visible_toast(), None);
    assert_eq!(chrome.drain_announcements(), vec!["Preset Saved"]);
}

#[test]
fn modal_capture_discards_dialog_frames_and_paces_clean_snapshots() {
    let mut capture = ronie_ui::default::ModalCapture::default();
    assert!(capture.request_due(0, false));
    assert!(!capture.request_due(125, false));
    assert!(!capture.request_due(125, true));
    assert!(!capture.complete(true));
    assert!(!capture.request_due(126, true));
    assert!(capture.request_due(126, false));
    assert!(capture.complete(false));
    assert!(!capture.request_due(126, false));
    assert!(capture.request_due(251, false));
}

#[test]
fn modal_capture_takes_one_snapshot_for_each_surface_that_opens() {
    use ronie_ui::default::BackdropUser;

    let mut capture = ronie_ui::default::ModalCapture::default();
    assert!(!capture.request_for(None, 0), "no surface needs an image");

    assert!(capture.request_for(Some(BackdropUser::Dialog), 0));
    assert!(capture.complete(false));
    assert!(!capture.request_for(Some(BackdropUser::Dialog), 10_000));

    assert!(capture.request_for(Some(BackdropUser::BusyCard), 10_000));
    assert!(
        capture.request_for(Some(BackdropUser::Dialog), 10_000),
        "the card closed, and the dialog needs its own image"
    );
    assert!(
        capture.complete(true),
        "this image is clean, although the dialog stopped its wait"
    );

    assert!(!capture.request_for(None, 10_000));
    assert!(capture.request_for(Some(BackdropUser::Dialog), 10_000));
    assert!(!capture.request_for(None, 10_000));
    assert!(!capture.in_flight(), "nothing waits for that image");
}

#[cfg(test)]
fn native_options(
    backend: std::sync::Arc<ronie_ui::backend::MockBackend>,
    name: &str,
) -> ronie_ui::shell::NativeShellOptions {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    ronie_ui::shell::NativeShellOptions {
        selftest: true,
        capture: Some(root.join(format!(
            ".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-11-{name}.png"
        ))),
        report: Some(root.join(format!(
            ".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-11-{name}.json"
        ))),
        scale: Some(2.0),
        backend: Some(backend),
        authenticated: true,
        ..Default::default()
    }
}

#[test]
#[ignore = "native window capture"]
fn capture_banner_at_two_times_scale() {
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "banner-buffer",
    )));
    let options = native_options(backend, "banner");
    let capture = options.capture.clone();
    ronie_ui::shell::run_scripted(options, ScenarioScript::new(Vec::new()))
        .expect("native banner capture");
    assert_eq!(
        image::image_dimensions(capture.expect("capture path")).expect("capture dimensions"),
        (2048, 1316)
    );
}

#[test]
#[ignore = "native window capture"]
fn capture_toast_at_two_times_scale() {
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "chrome-toast-saved",
    )));
    let options = native_options(backend, "toast");
    let capture = options.capture.clone();
    ronie_ui::shell::run_scripted(
        options,
        ScenarioScript::new(vec![Step::Toast("Preset Saved".to_owned())]),
    )
    .expect("native toast capture");
    assert_eq!(
        image::image_dimensions(capture.expect("capture path")).expect("capture dimensions"),
        (2048, 1228)
    );
}

#[test]
#[ignore = "native window capture"]
fn capture_connection_modal_at_two_times_scale() {
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "load-offline-modal",
    )));
    let options = native_options(backend, "connection-modal");
    let capture = options.capture.clone();
    ronie_ui::shell::run_scripted(
        options,
        ScenarioScript::new(vec![Step::RevealInsert, Step::Offline]),
    )
    .expect("native connection capture");
    assert_eq!(
        image::image_dimensions(capture.expect("capture path")).expect("capture dimensions"),
        (2048, 1228)
    );
}

#[test]
#[ignore = "native window capture"]
fn capture_update_modal_at_two_times_scale() {
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "load-update-notice",
    )));
    let options = native_options(backend, "update-modal");
    let payload = serde_json::json!({
        "version": "99.0.0",
        "message_html": "A new version is available.",
        "url": "https://fixtures.invalid/plugin"
    });
    let capture = options.capture.clone();
    ronie_ui::shell::run_scripted(
        options,
        ScenarioScript::new(vec![Step::UpdateOffer(payload)]),
    )
    .expect("native update capture");
    assert_eq!(
        image::image_dimensions(capture.expect("capture path")).expect("capture dimensions"),
        (2048, 1228)
    );
}

#[test]
fn system_modal_paints_scrim_after_background_and_before_dialog() {
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "load-offline-modal",
    )));
    let mut chrome = ronie_ui::default::ChromeController::new(backend, Vec::new(), false, true);
    chrome.set_online(false);
    assert!(!chrome.require_connection("browse"));
    let context = eframe::egui::Context::default();
    ronie_ui::fonts::Fonts::install(&context);
    let input = eframe::egui::RawInput {
        screen_rect: Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(1024.0, 614.0),
        )),
        ..Default::default()
    };
    let mut first = context.run_ui(input.clone(), |ui| {
        eframe::egui::CentralPanel::default()
            .frame(eframe::egui::Frame::NONE)
            .show(ui, |ui| ronie_ui::default::show(ui, &mut chrome));
    });
    first.textures_delta.clear();
    let mut output = context.run_ui(input, |ui| {
        eframe::egui::CentralPanel::default()
            .frame(eframe::egui::Frame::NONE)
            .show(ui, |ui| ronie_ui::default::show(ui, &mut chrome));
    });
    output.textures_delta.clear();
    let gallery = output
        .shapes
        .iter()
        .position(|shape| format!("{:?}", shape.shape).contains("SIGNAL CHAIN"))
        .expect("gallery shapes");
    let scrim = output
        .shapes
        .iter()
        .position(|shape| {
            matches!(
                &shape.shape,
                eframe::egui::Shape::Rect(rect)
                    if rect.fill == eframe::egui::Color32::from_black_alpha(128)
            )
        })
        .expect("scrim shape");
    let dialog = output
        .shapes
        .iter()
        .position(|shape| format!("{:?}", shape.shape).contains("No internet connection"))
        .expect("dialog shapes");
    assert!(
        gallery < scrim && scrim < dialog,
        "expected background before scrim before dialog, got gallery={gallery}, scrim={scrim}, dialog={dialog}"
    );
}

#[test]
fn system_modal_blocks_background_keyboard_input() {
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario(
        "load-offline-modal",
    )));
    let mut chrome = ronie_ui::default::ChromeController::new(backend, Vec::new(), false, true);
    chrome.set_tuner(true);
    chrome.set_online(false);
    assert!(!chrome.require_connection("browse"));
    let context = eframe::egui::Context::default();
    ronie_ui::fonts::Fonts::install(&context);
    let input = |events| eframe::egui::RawInput {
        screen_rect: Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(1024.0, 614.0),
        )),
        events,
        ..Default::default()
    };
    let mut first = context.run_ui(input(Vec::new()), |ui| {
        eframe::egui::CentralPanel::default()
            .frame(eframe::egui::Frame::NONE)
            .show(ui, |ui| ronie_ui::default::show(ui, &mut chrome));
    });
    first.textures_delta.clear();
    let mut output = context.run_ui(
        input(vec![eframe::egui::Event::Key {
            key: eframe::egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: eframe::egui::Modifiers::NONE,
        }]),
        |ui| {
            eframe::egui::CentralPanel::default()
                .frame(eframe::egui::Frame::NONE)
                .show(ui, |ui| ronie_ui::default::show(ui, &mut chrome));
        },
    );
    output.textures_delta.clear();
    assert!(chrome.tuner_visible());
}

#[test]
fn modal_scrim_swallows_background_pointer_input() {
    fn draw(ui: &mut eframe::egui::Ui, scrim: &ModalScrim, clicked: &mut bool) {
        if ui
            .put(
                eframe::egui::Rect::from_min_size(
                    eframe::egui::Pos2::ZERO,
                    eframe::egui::vec2(80.0, 30.0),
                ),
                eframe::egui::Button::new("Background"),
            )
            .clicked()
        {
            *clicked = true;
        }
        scrim.paint(ui, None);
    }

    let context = eframe::egui::Context::default();
    let scrim = ModalScrim::default();
    let mut clicked = false;
    let input = |events| eframe::egui::RawInput {
        screen_rect: Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(200.0, 100.0),
        )),
        events,
        ..Default::default()
    };
    let mut first = context.run_ui(input(Vec::new()), |ui| {
        eframe::egui::CentralPanel::default()
            .frame(eframe::egui::Frame::NONE)
            .show(ui, |ui| draw(ui, &scrim, &mut clicked));
    });
    first.textures_delta.clear();
    let position = eframe::egui::Pos2::new(20.0, 15.0);
    let mut pressed = context.run_ui(
        input(vec![
            eframe::egui::Event::PointerMoved(position),
            eframe::egui::Event::PointerButton {
                pos: position,
                button: eframe::egui::PointerButton::Primary,
                pressed: true,
                modifiers: eframe::egui::Modifiers::NONE,
            },
        ]),
        |ui| {
            eframe::egui::CentralPanel::default()
                .frame(eframe::egui::Frame::NONE)
                .show(ui, |ui| draw(ui, &scrim, &mut clicked));
        },
    );
    pressed.textures_delta.clear();
    let mut released = context.run_ui(
        input(vec![eframe::egui::Event::PointerButton {
            pos: position,
            button: eframe::egui::PointerButton::Primary,
            pressed: false,
            modifiers: eframe::egui::Modifiers::NONE,
        }]),
        |ui| {
            eframe::egui::CentralPanel::default()
                .frame(eframe::egui::Frame::NONE)
                .show(ui, |ui| draw(ui, &scrim, &mut clicked));
        },
    );
    released.textures_delta.clear();
    assert!(!clicked);
}
