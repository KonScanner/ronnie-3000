//! Chromatic tuner readings, transitions, accessibility, and backend effects.

use ronie_dsp::analysis::TunerReading;
use ronie_ui::{
    backend::MockBackend,
    default::{ChromeController, TunerFeed, TunerStatus, lit_count, pitch_from_frequency},
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

fn reading(scenario: &UiScenario) -> TunerReading {
    TunerReading {
        frequency_hz: scenario.state["tuner"]["frequency"]
            .as_f64()
            .expect("frequency"),
        confidence: scenario.state["tuner"]["confidence"]
            .as_f64()
            .expect("confidence"),
        level_db: scenario.state["tuner"]["level"].as_f64().expect("level"),
    }
}

#[test]
fn five_tuner_scenarios_have_exact_notes_status_and_lit_bars() {
    let cases = [
        ("main-tuner-idle", "", 0, TunerStatus::Idle, (0, 0)),
        ("main-tuner-intune", "A", 2, TunerStatus::InTune, (1, 1)),
        ("main-tuner-flat", "G", 3, TunerStatus::Flat, (3, 0)),
        ("main-tuner-sharp", "E", 4, TunerStatus::Sharp, (0, 4)),
        ("main-tuner-accidental", "F♯", 4, TunerStatus::Sharp, (0, 1)),
    ];
    for (id, note, octave, status, bars) in cases {
        let scenario = scenario(id);
        let mut feed = TunerFeed::default();
        for tick in 0..20 {
            feed.update(reading(&scenario), tick * 33);
        }
        let state = feed.state();
        assert_eq!(state.has_signal, id != "main-tuner-idle", "{id}");
        assert_eq!(state.note, note, "{id}");
        assert_eq!(state.octave, octave, "{id}");
        assert_eq!(state.status(), status, "{id}");
        assert_eq!(state.lit_bars(), bars, "{id}");
    }
    let pitch = pitch_from_frequency(371.9184396939457);
    assert_eq!(pitch.name, "F♯");
    assert!((pitch.cents - 8.979_305).abs() < 0.001);
    assert_eq!(lit_count(5.0), 1);
    assert_eq!(lit_count(50.0), 6);
}

#[test]
fn feed_smooths_cents_and_holds_the_last_note_for_nine_hundred_ms() {
    let mut feed = TunerFeed::default();
    feed.update(
        TunerReading {
            frequency_hz: 110.0,
            confidence: 0.95,
            level_db: -18.0,
        },
        0,
    );
    let sharp = 110.0 * 2.0_f64.powf(20.0 / 1200.0);
    feed.update(
        TunerReading {
            frequency_hz: sharp,
            confidence: 0.95,
            level_db: -18.0,
        },
        33,
    );
    assert!((feed.state().cents - 6.0).abs() < 0.001);
    let silence = TunerReading {
        frequency_hz: 0.0,
        confidence: 0.0,
        level_db: -60.0,
    };
    feed.update(silence, 932);
    assert!(feed.state().has_signal);
    assert_eq!(feed.state().note, "A");
    feed.update(silence, 933);
    assert!(!feed.state().has_signal);

    let mut low_confidence = TunerFeed::default();
    low_confidence.update(
        TunerReading {
            frequency_hz: 110.0,
            confidence: 0.5,
            level_db: -18.0,
        },
        0,
    );
    assert!(!low_confidence.state().has_signal);
}

#[test]
fn tuner_accessibility_reports_idle_note_octave_and_direction() {
    let mut feed = TunerFeed::default();
    assert_eq!(feed.state().accessibility_text(), "No pitch detected");
    for tick in 0..20 {
        feed.update(reading(&scenario("main-tuner-sharp")), tick * 33);
    }
    assert_eq!(
        feed.state().accessibility_text(),
        "E4, 335.0 Hz, 28 cents sharp"
    );
    for tick in 20..40 {
        feed.update(reading(&scenario("main-tuner-intune")), tick * 33);
    }
    assert_eq!(feed.state().accessibility_text(), "A2, 110.0 Hz, in tune");
}

#[test]
fn chrome_reads_the_backend_only_while_the_tuner_takeover_is_open() {
    let scenario = scenario("main-tuner-intune");
    let backend = std::sync::Arc::new(MockBackend::from_scenario(&scenario));
    let mut chrome = ChromeController::new(backend, Vec::new(), false, true);
    chrome.sync_tuner(0);
    assert!(!chrome.tuner().state().has_signal);
    chrome.set_tuner(true);
    chrome.sync_tuner(33);
    assert!(chrome.tuner_visible());
    assert!(chrome.tuner().state().has_signal);
    assert_eq!(chrome.tuner().state().note, "A");
    chrome.set_tuner(false);
    chrome.sync_tuner(66);
    assert!(!chrome.tuner().state().has_signal);
}

#[test]
#[ignore = "native window capture"]
fn capture_tuner_at_two_times_scale() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let capture =
        root.join(".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-11-tuner.png");
    let backend = std::sync::Arc::new(MockBackend::from_scenario(&scenario("main-tuner-intune")));
    let options =
        ronie_ui::shell::NativeShellOptions {
            selftest: true,
            capture: Some(capture.clone()),
            report: Some(root.join(
                ".superpowers/sdd/2026-10-31-ronie-linux-standalone-and-ui/task-11-tuner.json",
            )),
            scale: Some(2.0),
            backend: Some(backend),
            authenticated: true,
            ..Default::default()
        };
    ronie_ui::shell::run_scripted(options, ScenarioScript::new(vec![Step::Tuner]))
        .expect("native tuner capture");
    assert_eq!(
        image::image_dimensions(capture).expect("capture dimensions"),
        (2048, 1228)
    );
}
