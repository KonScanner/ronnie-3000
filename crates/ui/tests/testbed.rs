//! Neutral scenario and backend-effect tests.

use std::collections::BTreeMap;

use ronie_ui::{
    backend::{MockBackend, UiDrive, UiEffect, UiError, UiEvent},
    testbed::{BLOCKED_FIXTURES, UiScenario, UiSection, load_scenarios, selftest},
};

#[test]
fn official_scenarios_have_exact_section_counts() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("fixtures parse");
    // The 103 official scenarios, 4 scenarios of the tuning picker, 3 of OPTIMIZE, and 3 of
    // the key page and the saved key.
    assert_eq!(scenarios.len(), 113);
    let mut counts = BTreeMap::new();
    for scenario in scenarios {
        *counts.entry(scenario.section).or_insert(0_usize) += 1;
    }
    assert_eq!(counts.get(&UiSection::Main), Some(&17));
    assert_eq!(counts.get(&UiSection::Loading), Some(&14));
    assert_eq!(counts.get(&UiSection::Banners), Some(&10));
    assert_eq!(counts.get(&UiSection::Browser), Some(&37));
    assert_eq!(counts.get(&UiSection::Settings), Some(&12));
    assert_eq!(counts.get(&UiSection::Chrome), Some(&23));
}

#[test]
fn each_fixture_parameter_is_a_number_in_its_range() {
    use ronie_core::params::ParameterCatalog;
    use ronie_ui::backend::UiBackend;

    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("fixtures parse");
    for scenario in &scenarios {
        let backend = MockBackend::from_scenario(scenario);
        for spec in ParameterCatalog::v1().iter() {
            let value = backend.parameter(spec.id).expect("catalog parameter").0;
            assert!(
                value >= spec.min && value <= spec.max,
                "{} {}: {value}",
                scenario.id,
                spec.id.0
            );
        }
    }
}

#[test]
fn public_fixtures_have_no_private_names_or_urls() {
    let scenarios = include_str!("../../../fixtures/ui-scenarios.json");
    let visual = include_str!("../../../fixtures/ui-visual-manifest.json");
    let restricted = ["to", "ne3000"].join("");
    for content in [scenarios, visual] {
        let lower = content.to_lowercase();
        assert!(!lower.contains(&restricted));
        assert!(!lower.contains("t3k-fixtures.local"));
        assert!(!lower.contains("/home/"));
        assert!(!lower.contains("access_token"));
        assert!(!lower.contains("refresh_token"));
    }
}

#[test]
fn every_fixture_backend_answers_every_scenario() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("fixtures parse");
    for scenario in scenarios {
        if BLOCKED_FIXTURES.iter().any(|(id, _)| *id == scenario.id) {
            assert!(
                selftest::run(&scenario).is_err(),
                "{} now passes, so remove it from the blocked list",
                scenario.id
            );
            continue;
        }
        let report =
            selftest::run(&scenario).unwrap_or_else(|error| panic!("{}: {error}", scenario.id));
        assert!(
            report.effects_verified,
            "{} must verify backend effects",
            scenario.id
        );
    }
}

#[test]
fn unknown_drive_target_is_an_error() {
    let scenario = UiScenario {
        id: "unknown-drive".to_owned(),
        section: UiSection::Main,
        state: serde_json::json!({"chain": {"revision": 0}}),
        drives: vec![UiDrive::Click {
            target: "missing-control".to_owned(),
        }],
        viewport: Default::default(),
    };
    assert!(matches!(
        selftest::run(&scenario),
        Err(UiError::UnknownTarget(target)) if target == "missing-control"
    ));
}

#[test]
fn pixels_cannot_pass_without_expected_backend_effect() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("fixtures parse");
    let scenario = scenarios
        .iter()
        .find(|scenario| scenario.id == "chrome-toast-saved")
        .expect("preset scenario exists");
    let backend = MockBackend::from_scenario(scenario);
    let mut backend = backend;
    backend.set_receipt_effect_override(UiEffect::None);
    assert!(matches!(
        selftest::run_with_backend(scenario, backend),
        Err(UiError::MissingBackendEffect {
            expected: UiEffect::PresetSaved,
            actual: UiEffect::None,
        })
    ));
}

#[test]
fn a_parameter_action_is_logged_with_its_write_and_effect() {
    use ronie_core::params::{ParameterId, ParameterValue};
    use ronie_ui::backend::{GesturePhase, UiAction, UiBackend};

    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("fixtures parse");
    let backend = MockBackend::from_scenario(&scenarios[0]);
    let write = |value: f32| UiAction::SetParameter {
        id: ParameterId("outputLevel"),
        value: ParameterValue(value),
        gesture: GesturePhase::End,
    };

    let receipt = backend.dispatch(write(0.25)).expect("write");
    assert_eq!(receipt.effect, UiEffect::ParameterChanged);
    assert_eq!(receipt.revision, backend.chain_state(0).revision);
    assert_eq!(backend.actions(), vec![write(0.25)]);
    assert_eq!(
        backend.events(),
        vec![UiEvent::Effect(UiEffect::ParameterChanged)]
    );

    assert!(backend.dispatch(write(f32::INFINITY)).is_err());
    assert_eq!(backend.actions(), vec![write(0.25), write(f32::INFINITY)]);
    assert_eq!(backend.events().len(), 1, "a refused write has no effect");
}

#[test]
fn click_targets_are_scrolled_into_view_first() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("fixtures parse");
    let scenario = scenarios
        .iter()
        .find(|scenario| scenario.id == "chrome-toast-saved")
        .expect("preset scenario exists");
    let backend = MockBackend::from_scenario(scenario);
    selftest::run_with_backend(scenario, backend.clone()).expect("scenario passes");
    let events = backend.events();
    assert!(matches!(events.first(), Some(UiEvent::Scrolled { .. })));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, UiEvent::Effect(UiEffect::PresetSaved)))
    );
}

#[test]
fn complete_selftest_passes() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("fixtures parse");
    let report = selftest::run_all(&scenarios).expect("selftest passes");
    assert_eq!(report.passed, 113 - BLOCKED_FIXTURES.len());
    assert_eq!(report.failed, BLOCKED_FIXTURES.len());
    assert_eq!(
        report.blocked,
        BLOCKED_FIXTURES
            .iter()
            .map(|(id, _)| (*id).to_owned())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_click_on_a_control_that_the_real_ui_does_not_show_fails() {
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("fixtures parse");
    let scenario = scenarios
        .iter()
        .find(|scenario| scenario.id == "chrome-toast-saved")
        .expect("preset scenario exists");
    let scenario = UiScenario {
        drives: vec![UiDrive::Click {
            target: "Save".to_owned(),
        }],
        ..scenario.clone()
    };
    assert!(matches!(
        selftest::run(&scenario),
        Err(UiError::DriveFailed { target, .. }) if target == "Save"
    ));
}

#[test]
fn visual_manifest_is_separate_from_neutral_state() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/ui-visual-manifest.json"))
            .expect("visual manifest parses");
    let scenarios = manifest["scenarios"].as_array().expect("manifest rows");
    assert_eq!(scenarios.len(), 113);
    for row in scenarios {
        assert_eq!(row["global_mismatch_max"], 0.019);
        assert_eq!(row["worst_tile_64_max"], 0.24);
        assert_eq!(row["backend_effect_required"], true);
        assert_eq!(row["status"], "unverified");
        assert!(row.get("state").is_none());
        assert!(row.get("drives").is_none());
    }
}
