//! Settings takeover, parameter scope, forms, and standalone device workflows.

use std::sync::Arc;

use ronie_core::params::{ParameterCatalog, ParameterId, ParameterValue};
use ronie_ui::{
    backend::{DeviceAction, MockBackend, NavigationAction, UiAction, UiBackend},
    default::{
        AudioDeviceState, BannerStore, FormRect, SettingsController, SettingsTab, SystemDeviceState,
    },
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

fn backend(id: &str) -> Arc<MockBackend> {
    Arc::new(MockBackend::from_scenario(&scenario(id)))
}

#[test]
fn all_eleven_settings_scenarios_mount_the_expected_takeover_and_sections() {
    for id in [
        "settings-plugin",
        "settings-advanced",
        "settings-midi-empty",
        "settings-midi-learning",
        "settings-midi-picker",
        "settings-presets",
        "settings-system",
        "settings-system-mic-denied",
        "settings-system-feedback-risk",
        "settings-system-input-muted",
        "settings-system-rate96",
    ] {
        let system = id.starts_with("settings-system");
        let settings = SettingsController::new(
            backend(id),
            system,
            if system {
                SettingsTab::System
            } else {
                SettingsTab::Plugin
            },
        );
        assert_eq!(settings.standalone(), system, "{id}");
        assert_eq!(
            settings.tab(),
            if system {
                SettingsTab::System
            } else {
                SettingsTab::Plugin
            },
            "{id}"
        );
        if system {
            let sections = settings.system().section_names();
            for expected in ["Hear Yourself", "Audio", "MIDI Inputs"] {
                assert!(sections.contains(&expected), "{id}: missing {expected}");
            }
            assert!(settings.system().device().is_some(), "{id}");
        } else {
            let sections = settings.plugin().section_names();
            for expected in [
                "Show Info Bar",
                "Show Gate",
                "Show Pitch Shift",
                "NAM A2 Size",
                "Calibration",
                "Oversampling",
                "Multi-Core Processing",
                "MIDI Mapping",
                "Presets",
                "Diagnostics",
            ] {
                assert!(sections.contains(&expected), "{id}: missing {expected}");
            }
        }
    }
}

#[test]
fn settings_column_is_centered_and_capped_at_four_hundred_eighty_pixels() {
    assert_eq!(
        ronie_ui::default::forms::column_bounds(1024.0),
        (296.0, 432.0)
    );
    assert_eq!(
        ronie_ui::default::forms::column_bounds(300.0),
        (24.0, 252.0)
    );
}

#[test]
fn fractional_form_boxes_snap_edges_and_keep_absolute_subpixel_phase() {
    let placement = FormRect::new(4.2, 10.2, 100.5, 10.15).snap(0.25);
    assert_eq!(placement.x, 4);
    assert_eq!(placement.y, 10);
    assert_eq!(placement.width, 101);
    assert_eq!(placement.height, 11);
    assert!((placement.phase - 0.45).abs() < 0.001);
    let child = FormRect::new(0.0, 1.1, 20.0, 2.2).snap(placement.phase);
    assert_eq!(child.y, 2);
    assert_eq!(child.height, 2);
}

#[test]
fn settings_tab_cycle_stays_inside_the_takeover_without_default_focus() {
    let mut settings =
        SettingsController::new(backend("settings-plugin"), false, SettingsTab::Plugin);
    assert!(settings.focused().is_none());
    let order = settings.focus_order();
    assert!(!order.is_empty());
    for _ in 0..=order.len() {
        assert!(settings.tab_focus(false));
    }
    assert_eq!(settings.focused(), Some(&order[0]));
    assert!(settings.tab_focus(true));
    assert_eq!(settings.focused(), Some(order.last().expect("last")));
    settings.close();
    assert!(settings.closed());
    assert!(settings.focused().is_none());
}

#[test]
fn explicit_machine_default_edits_are_separate_from_host_restores_and_presets() {
    let backend = backend("settings-advanced");
    let mut settings = SettingsController::new(backend.clone(), false, SettingsTab::Plugin);
    backend
        .set_parameter(
            ParameterId("inputCalibrationLevel"),
            ParameterValue(10.0),
            ronie_ui::backend::GesturePhase::Change,
        )
        .expect("host restore");
    assert!(!backend.actions().iter().any(|action| matches!(
        action,
        UiAction::PersistMachineDefault(ParameterId("inputCalibrationLevel"))
    )));

    settings
        .plugin_mut()
        .set_calibrated(true)
        .expect("calibrate");
    settings
        .plugin_mut()
        .set_oversampling(true)
        .expect("oversampling");
    settings
        .plugin_mut()
        .set_oversampling_factor(2)
        .expect("rate");
    settings
        .plugin_mut()
        .set_nam_full_size(true)
        .expect("full size");
    settings
        .plugin_mut()
        .set_multi_core(false)
        .expect("single core");
    let actions = backend.actions();
    for id in ["calibrateInput", "osEnabled", "osFactor"] {
        assert!(actions.contains(&UiAction::SetParameter {
            id: ParameterId(id),
            value: match id {
                "calibrateInput" | "osEnabled" => ParameterValue(1.0),
                _ => ParameterValue(2.0),
            },
            gesture: ronie_ui::backend::GesturePhase::Change,
        }));
        assert!(actions.contains(&UiAction::PersistMachineDefault(ParameterId(id))));
    }
    assert!(actions.contains(&UiAction::SetNamFullSize(true)));
    assert!(actions.contains(&UiAction::SetMultiCore(false)));

    for spec in ParameterCatalog::v1().iter().filter(|spec| {
        matches!(
            spec.id.0,
            "calibrateInput" | "inputCalibrationLevel" | "osEnabled" | "osFactor"
        )
    }) {
        assert!(spec.scope.session);
        assert!(spec.scope.machine_default);
        assert!(!spec.scope.preset);
    }
}

#[test]
fn calibration_draft_survives_rebuilds_and_commits_clamped_real_units() {
    let backend = backend("settings-advanced");
    let mut first = SettingsController::new(backend.clone(), false, SettingsTab::Plugin);
    first
        .plugin_mut()
        .edit_calibration_draft("13.7")
        .expect("draft");
    drop(first);
    let mut rebuilt = SettingsController::new(backend.clone(), false, SettingsTab::Plugin);
    assert_eq!(
        rebuilt.plugin().calibration_draft().as_deref(),
        Some("13.7")
    );
    rebuilt
        .plugin_mut()
        .commit_calibration_draft()
        .expect("commit");
    assert_eq!(
        backend.parameter(ParameterId("inputCalibrationLevel")),
        Some(ParameterValue(13.7))
    );
    rebuilt
        .plugin_mut()
        .edit_calibration_draft("not-a-number")
        .expect("draft");
    rebuilt
        .plugin_mut()
        .commit_calibration_draft()
        .expect("reject");
    assert_eq!(
        backend.parameter(ParameterId("inputCalibrationLevel")),
        Some(ParameterValue(13.7))
    );
    rebuilt
        .plugin_mut()
        .edit_calibration_draft("-999")
        .expect("draft");
    rebuilt
        .plugin_mut()
        .commit_calibration_draft()
        .expect("clamp");
    assert_eq!(
        backend.parameter(ParameterId("inputCalibrationLevel")),
        Some(ParameterValue(-60.0))
    );
}

#[test]
fn device_rate_buffer_and_device_changes_use_complete_typed_configs() {
    let backend = backend("settings-system");
    let settings = SettingsController::new(backend.clone(), true, SettingsTab::System);
    settings.system().set_sample_rate(96_000).expect("rate");
    settings.system().set_buffer_size(256).expect("buffer");
    settings
        .system()
        .set_input_device(Some("Guitar Input".to_owned()))
        .expect("input");
    settings
        .system()
        .set_output_device(Some("Studio Monitors".to_owned()))
        .expect("output");
    settings.system().set_backend("JACK").expect("backend");
    let actions = backend.actions();
    assert!(actions.contains(&UiAction::Device(DeviceAction::Configure(
        ronie_ui::backend::AudioDeviceConfig {
            sample_rate: Some(96_000.0),
            buffer_frames: Some(256),
            input: Some("Guitar Input".to_owned()),
            output: Some("Studio Monitors".to_owned()),
            midi_inputs: vec!["midi-1".to_owned()],
        }
    ))));
    assert!(
        actions.contains(&UiAction::Device(DeviceAction::SelectBackend(
            "JACK".to_owned()
        )))
    );
}

#[test]
fn input_channel_selection_is_ordered_bounded_and_switches_modes() {
    let backend = backend("settings-system");
    let settings = SettingsController::new(backend.clone(), true, SettingsTab::System);
    let mut device = settings.system().device().expect("device").clone();
    device
        .input_channels
        .push(ronie_ui::default::AudioInputChannel {
            index: 2,
            name: "Input 3".to_owned(),
            active: false,
        });
    backend.set_device_settings(Some(device));
    settings.system().select_channel(1).expect("second");
    settings.system().select_channel(2).expect("third");
    assert_eq!(settings.system().active_channels(), vec![1, 2]);
    settings.system().select_channel(2).expect("minimum");
    assert_eq!(settings.system().active_channels(), vec![1, 2]);
    settings.system().set_stereo(false).expect("mono");
    assert_eq!(settings.system().active_channels(), vec![2]);
    settings.system().set_stereo(true).expect("stereo");
    assert_eq!(settings.system().active_channels(), vec![2, 0]);
    assert_eq!(settings.system().input_levels().len(), 2);
}

#[test]
fn inline_system_warnings_match_the_global_banner_rules() {
    for (id, expected) in [
        ("settings-system-mic-denied", "mic-denied"),
        ("settings-system-feedback-risk", "feedback-risk"),
        ("settings-system-input-muted", "input-muted"),
        ("settings-system-rate96", "rate-not-48k"),
    ] {
        let backend = backend(id);
        let settings = SettingsController::new(backend, true, SettingsTab::System);
        let device: AudioDeviceState = settings.system().device().expect("device").banner.clone();
        let mut banners = BannerStore::new();
        banners.evaluate(Some(&device), 0);
        assert_eq!(
            banners.active().map(|banner| banner.id.as_str()),
            Some(expected),
            "{id}"
        );
        assert!(
            settings.system().inline_banner_ids().contains(&expected),
            "{id}"
        );
    }
}

#[test]
fn disconnected_devices_keep_a_visible_stale_selection_and_inline_error() {
    let backend = backend("settings-system");
    let settings = SettingsController::new(backend.clone(), true, SettingsTab::System);
    let mut device: SystemDeviceState = settings.system().device().expect("device").clone();
    device.banner.device_open = false;
    device.input_devices.clear();
    backend.set_device_settings(Some(device));
    let device = settings.system().device().expect("device");
    assert!(device.inline_error().is_some());
    assert!(
        device
            .input_options()
            .iter()
            .any(|option| option.label.contains("disconnected"))
    );
}

#[test]
fn preferences_and_preset_folder_use_typed_boundaries() {
    let backend = backend("settings-presets");
    let settings = SettingsController::new(backend.clone(), false, SettingsTab::Plugin);
    settings
        .plugin()
        .set_preference("t3k.showPitchControl", true)
        .expect("preference");
    settings.plugin().open_presets_folder().expect("folder");
    assert_eq!(backend.preference("t3k.showPitchControl"), Some(true));
    assert!(
        backend
            .actions()
            .contains(&UiAction::Navigate(NavigationAction::OpenPresetsFolder))
    );
}

#[test]
fn faceplate_effect_preferences_hide_and_reveal_controls() {
    let backend = backend("settings-plugin");
    let faceplate = ronie_ui::default::Faceplate::new(backend.clone());
    assert!(
        faceplate
            .visible_controls(false, false)
            .contains(&"gateThreshold")
    );
    // Pitch Shift shows by default, so the tuning picker is easy to find.
    assert!(
        faceplate
            .visible_controls(false, false)
            .contains(&"pitchSemitones")
    );
    backend
        .dispatch(UiAction::SetPreference {
            key: "t3k.showPitchControl".to_owned(),
            value: false,
        })
        .expect("hide pitch");
    assert!(
        !faceplate
            .visible_controls(false, false)
            .contains(&"pitchSemitones")
    );
    backend
        .dispatch(UiAction::SetPreference {
            key: "t3k.showPitchControl".to_owned(),
            value: true,
        })
        .expect("show pitch");
    assert!(
        faceplate
            .visible_controls(false, false)
            .contains(&"pitchSemitones")
    );
    backend
        .dispatch(UiAction::SetPreference {
            key: "t3k.showGateControl".to_owned(),
            value: false,
        })
        .expect("hide gate");
    backend
        .set_parameter(
            ParameterId("gateEnabled"),
            ParameterValue(0.0),
            ronie_ui::backend::GesturePhase::Change,
        )
        .expect("gate off");
    assert!(
        !faceplate
            .visible_controls(false, false)
            .contains(&"gateThreshold")
    );
    backend
        .set_parameter(
            ParameterId("gateEnabled"),
            ParameterValue(1.0),
            ronie_ui::backend::GesturePhase::Change,
        )
        .expect("gate on");
    assert!(
        faceplate
            .visible_controls(false, false)
            .contains(&"gateThreshold")
    );
}

#[test]
fn block_size_and_normalization_preferences_control_visible_chips() {
    let backend = backend("main-detail");
    let mut detail =
        ronie_ui::default::BlockDetail::new(backend.clone(), "blk-1", false).expect("block detail");
    assert!(!detail.show_slim_size_control());
    assert!(!detail.show_normalize_control());
    backend
        .dispatch(UiAction::SetPreference {
            key: "t3k.showBlockSizeControl".to_owned(),
            value: true,
        })
        .expect("show size");
    backend
        .dispatch(UiAction::SetPreference {
            key: "t3k.showBlockNormalizeControl".to_owned(),
            value: true,
        })
        .expect("show normalize");
    assert!(detail.show_slim_size_control());
    assert!(detail.show_normalize_control());
    backend
        .dispatch(UiAction::SetPreference {
            key: "t3k.showBlockSizeControl".to_owned(),
            value: false,
        })
        .expect("hide size");
    detail.set_slim_full(true).expect("different size");
    assert!(detail.show_slim_size_control());
}

#[test]
fn output_test_tone_and_pair_selection_use_typed_device_actions() {
    let backend = backend("settings-system");
    let settings = SettingsController::new(backend.clone(), true, SettingsTab::System);
    settings.system().play_test_tone().expect("test tone");
    settings.system().set_output_pair(1).expect("output pair");
    assert!(
        backend
            .actions()
            .contains(&UiAction::Device(DeviceAction::PlayTestTone))
    );
    assert!(
        backend
            .actions()
            .contains(&UiAction::Device(DeviceAction::SetOutputPair(1)))
    );
}

fn native_options(backend: Arc<MockBackend>, name: &str) -> ronie_ui::shell::NativeShellOptions {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    ronie_ui::shell::NativeShellOptions {
        selftest: true,
        capture: Some(root.join(format!(
            ".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-12-{name}.png"
        ))),
        report: Some(root.join(format!(
            ".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-12-{name}.json"
        ))),
        scale: Some(2.0),
        backend: Some(backend),
        authenticated: true,
        ..Default::default()
    }
}

#[test]
#[ignore = "native window capture"]
fn capture_plugin_settings_at_two_times_scale() {
    let backend = backend("settings-advanced");
    let options = native_options(backend, "plugin-settings");
    let capture = options.capture.clone();
    ronie_ui::shell::run_scripted(
        options,
        ScenarioScript::new(vec![Step::Settings(SettingsTab::Plugin)]),
    )
    .expect("native plugin settings capture");
    assert_eq!(
        image::image_dimensions(capture.expect("capture path")).expect("capture dimensions"),
        (2048, 1228)
    );
}

#[test]
#[ignore = "native window capture"]
fn capture_system_settings_at_two_times_scale() {
    let backend = backend("settings-system");
    let options = native_options(backend, "system-settings");
    let capture = options.capture.clone();
    ronie_ui::shell::run_scripted(
        options,
        ScenarioScript::new(vec![Step::Settings(SettingsTab::System)]),
    )
    .expect("native system settings capture");
    assert_eq!(
        image::image_dimensions(capture.expect("capture path")).expect("capture dimensions"),
        (2048, 1228)
    );
}

/// A driver on the system page of a device state that a test sets.
fn system_page(device: SystemDeviceState) -> (ronie_ui::driver::UiDriver, Arc<MockBackend>) {
    let backend = backend("settings-system");
    backend.set_device_settings(Some(device));
    let mut chrome =
        ronie_ui::default::ChromeController::new(backend.clone(), Vec::new(), false, true);
    chrome.open_system_settings();
    let mut driver = ronie_ui::driver::UiDriver::new(chrome, (*backend).clone());
    driver.settle();
    (driver, backend)
}

fn scenario_device() -> SystemDeviceState {
    let backend = backend("settings-system");
    backend.device_settings().expect("device")
}

fn row(id: &str, label: &str) -> ronie_ui::default::AudioDeviceOption {
    ronie_ui::default::AudioDeviceOption {
        id: id.to_owned(),
        label: label.to_owned(),
    }
}

#[test]
fn one_click_selects_one_row_of_two_rows_with_one_label() {
    let mut device = scenario_device();
    device.input_devices = vec![row("card-a", "One Name"), row("card-b", "One Name")];
    device.selected_input = "card-a".to_owned();
    device.banner.input_device = "One Name".to_owned();
    let (mut driver, backend) = system_page(device);

    let _ = driver.click("Input device").expect("open the list");
    driver.settle();
    let mut rows: Vec<_> = driver
        .controls()
        .iter()
        .filter(|control| control.label == "One Name")
        .map(|control| control.rect.center())
        .collect();
    rows.sort_by(|a, b| a.y.total_cmp(&b.y));
    assert_eq!(rows.len(), 2, "{:?}", driver.visible_labels());
    let _ = driver.click_at(rows[1]);
    driver.settle();

    let chosen = backend
        .actions()
        .into_iter()
        .find_map(|action| match action {
            UiAction::Device(DeviceAction::Configure(config)) => config.input,
            _ => None,
        });
    assert_eq!(chosen.as_deref(), Some("card-b"));
    let state = backend.device_settings().expect("device");
    assert_eq!(state.selected_input, "card-b");
}

#[test]
fn a_fixture_can_give_a_row_as_a_name_or_with_an_identity() {
    let value = serde_json::json!({
        "bufferSize": 256,
        "sampleRate": 48000.0,
        "inputDevice": "Plain Name",
        "inputDeviceId": "plain",
        "inputDevices": ["Other Name", {"id": "plain", "label": "Plain Name"}],
        "outputDevice": "Only Name",
        "outputDevices": ["Only Name"],
    });

    let device = SystemDeviceState::from_ui_value(&value).expect("device");

    assert_eq!(device.input_devices[0], row("Other Name", "Other Name"));
    assert_eq!(device.input_devices[1], row("plain", "Plain Name"));
    assert_eq!(device.selected_input, "plain");
    assert_eq!(device.selected_output, "Only Name");
    assert_eq!(device.config().input.as_deref(), Some("plain"));
}

#[test]
fn the_page_has_its_short_part_and_advanced_opens_the_rest() {
    let (mut driver, _backend) = system_page(scenario_device());

    for label in [
        "Input device",
        "Output device",
        "Test",
        "Sample rate",
        "Buffer size",
        "Advanced",
    ] {
        assert!(
            driver.is_visible(label),
            "{label}: {:?}",
            driver.visible_labels()
        );
    }
    for label in ["Mono", "Stereo", "Hear Yourself"] {
        assert!(!driver.is_visible(label), "{label} waits behind Advanced");
    }

    let _ = driver.click("Advanced").expect("open");
    driver.settle();

    for label in ["Mono", "Stereo", "Hear Yourself"] {
        assert!(
            driver.is_visible(label),
            "{label}: {:?}",
            driver.visible_labels()
        );
    }
}

#[test]
fn a_device_with_one_channel_shows_no_mono_or_stereo() {
    let mut device = scenario_device();
    device.input_channels.truncate(1);
    let (mut driver, _backend) = system_page(device);

    let _ = driver.click("Advanced").expect("open");
    driver.settle();

    assert!(!driver.is_visible("Mono"), "{:?}", driver.visible_labels());
    assert!(!driver.is_visible("Stereo"));
    assert!(!driver.is_visible("Input channel 1"));
}

#[test]
fn a_driver_row_appears_with_more_than_one_driver() {
    let mut device = scenario_device();
    device.device_types = vec!["System".to_owned(), "ALSA".to_owned(), "JACK".to_owned()];
    device.banner.current_type = "System".to_owned();
    let (mut driver, backend) = system_page(device);

    let _ = driver.click("Advanced").expect("open");
    driver.settle();
    let _ = driver.click("ALSA").expect("choose");
    driver.settle();

    assert!(
        backend
            .actions()
            .contains(&UiAction::Device(DeviceAction::SelectBackend(
                "ALSA".to_owned()
            )))
    );
}

#[test]
fn the_test_button_asks_for_the_tone() {
    let (mut driver, backend) = system_page(scenario_device());

    let _ = driver.click("Test").expect("test");
    driver.settle();

    assert!(
        backend
            .actions()
            .contains(&UiAction::Device(DeviceAction::PlayTestTone))
    );
}

#[test]
fn retry_on_an_error_reopens_the_stream() {
    let mut device = scenario_device();
    device.banner.device_open = false;
    device.error = Some("Fake Interface is in use by another program.".to_owned());
    let (mut driver, backend) = system_page(device);

    let _ = driver.click("Retry").expect("retry");
    driver.settle();

    assert!(
        backend
            .actions()
            .contains(&UiAction::Device(DeviceAction::Restart))
    );
}
