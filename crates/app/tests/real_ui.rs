//! The real Default UI on the real standalone adapter, driven by real pointer input.
//!
//! The scenario tests run the same UI on the mock, which keeps no undo history and offers each
//! control. These tests prove what only the real adapter can show.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

use eframe::egui;
use ronie_app::standalone_backend::{FolderOpener, StandaloneBackend};
use ronie_core::{InputMode, PREFERENCES_SETTINGS, params::ParameterId};
use ronie_dsp::PrepareSpec;
use ronie_plugin::{ProcessorBackend, RonieProcessor};
use ronie_ui::{
    backend::{DeviceAction, HostControl, MockBackend, UiAction, UiBackend, UiError},
    default::ChromeController,
    driver::UiDriver,
};

struct Rig {
    backend: Arc<StandaloneBackend>,
    driver: UiDriver,
    root: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        Self::with_opener(None)
    }

    fn with_opener(opener: Option<FolderOpener>) -> Self {
        let root = tempfile::tempdir().expect("temporary root");
        let processor = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        }));
        let backend = StandaloneBackend::dummy(processor, &root.path().join(PREFERENCES_SETTINGS))
            .expect("dummy backend");
        let backend = Arc::new(match opener {
            Some(opener) => backend.with_folder_opener(opener),
            None => backend,
        });
        let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
        chrome.set_standalone(true);
        Self {
            backend,
            driver: UiDriver::on_real_backend(chrome),
            root,
        }
    }

    fn value(&self, id: &'static str) -> f32 {
        self.backend.parameter(ParameterId(id)).expect("known").0
    }

    fn undo_steps(&self) -> usize {
        let mut steps = 0;
        while self.backend.chain_state(0).can_undo {
            self.backend.dispatch(UiAction::Undo).expect("undo");
            steps += 1;
        }
        steps
    }

    fn knob_label(&self, prefix: &str) -> String {
        self.driver
            .label_starting_with(prefix)
            .unwrap_or_else(|| panic!("no {prefix} in {:?}", self.driver.visible_labels()))
    }

    /// Drags a control up by `steps` times ten points, over one frame for each ten points.
    fn drag_up(&mut self, label: &str, steps: usize) {
        let offsets: Vec<egui::Vec2> = (1..=steps)
            .map(|step| egui::vec2(0.0, -10.0 * step as f32))
            .collect();
        let _ = self.driver.drag_control(label, &offsets).expect("drag");
    }
}

#[test]
fn a_faceplate_knob_drag_is_one_undo_step() {
    let mut rig = Rig::new();
    let start = rig.value("inputLevel");
    let label = rig.knob_label("Input:");

    rig.drag_up(&label, 6);

    assert!(rig.value("inputLevel") > start, "the drag moved the value");
    assert_eq!(rig.undo_steps(), 1);
    assert_eq!(rig.value("inputLevel"), start);
}

#[test]
fn each_frame_of_a_drag_reaches_the_processor() {
    let mut rig = Rig::new();
    let start = rig.value("inputLevel");
    let label = rig.knob_label("Input:");
    let mut seen = Vec::new();
    let control = rig
        .driver
        .controls()
        .iter()
        .find(|control| control.label == label)
        .expect("knob")
        .rect
        .center();
    rig.driver.play(vec![egui::Event::PointerMoved(control)]);
    rig.driver.play(vec![egui::Event::PointerButton {
        pos: control,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    }]);
    for step in 1..=4 {
        let pos = control - egui::vec2(0.0, 10.0 * step as f32);
        rig.driver.play(vec![egui::Event::PointerMoved(pos)]);
        seen.push(rig.value("inputLevel"));
    }
    assert!(
        seen.windows(2).all(|pair| pair[1] > pair[0]) && seen[0] > start,
        "{seen:?}"
    );
}

#[test]
fn a_drag_that_ends_where_it_started_at_the_range_end_is_no_undo_step() {
    let mut rig = Rig::new();
    let id = ParameterId("inputLevel");
    rig.backend
        .set_parameter(
            id,
            ronie_core::params::ParameterValue(1.0),
            ronie_ui::backend::GesturePhase::End,
        )
        .expect("top of the range");
    rig.driver.settle();
    assert_eq!(rig.undo_steps(), 1);
    rig.backend
        .dispatch(UiAction::Redo)
        .expect("back to the top");
    rig.driver.settle();
    let label = rig.knob_label("Input:");

    // Up from the top of the range changes nothing.
    rig.drag_up(&label, 4);

    assert_eq!(rig.value("inputLevel"), 1.0);
    assert_eq!(rig.undo_steps(), 1, "only the first edit is a step");
}

#[test]
fn a_press_with_no_movement_is_no_undo_step() {
    let mut rig = Rig::new();
    let label = rig.knob_label("Input:");
    let _ = rig.driver.click(&label).expect("click");
    assert_eq!(rig.undo_steps(), 0);
}

#[test]
fn the_knob_face_follows_an_undo_and_the_next_drag_starts_from_there() {
    let mut rig = Rig::new();
    let start = rig.value("inputLevel");
    let label = rig.knob_label("Input:");
    rig.drag_up(&label, 6);
    let moved = rig.value("inputLevel");
    assert!(moved > start);
    let moved_label = rig.knob_label("Input:");

    rig.backend.dispatch(UiAction::Undo).expect("undo");
    rig.driver.settle();

    assert_eq!(
        rig.knob_label("Input:"),
        label,
        "the face shows the old value"
    );
    assert_ne!(rig.knob_label("Input:"), moved_label);
    rig.drag_up(&label, 1);
    assert!(rig.value("inputLevel") < moved, "no jump to the old drag");
}

#[test]
fn a_pan_knob_drag_with_a_linked_pan_is_one_undo_step() {
    let mut rig = Rig::new();
    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("stereo");
    rig.driver.settle();
    assert_eq!(rig.undo_steps(), 1);
    rig.backend
        .dispatch(UiAction::Redo)
        .expect("back to stereo");
    rig.driver.settle();
    let linked = rig.value("chainPanLinked") > 0.5;
    assert!(linked, "the default links the two pans");
    let (left, right) = (rig.value("chainPanLeft"), rig.value("chainPanRight"));

    let offsets: Vec<egui::Vec2> = (1..=5)
        .map(|step| egui::vec2(0.0, 10.0 * step as f32))
        .collect();
    let label = rig.knob_label("Pan R");
    let _ = rig.driver.drag_control(&label, &offsets).expect("drag");
    assert_ne!(rig.value("chainPanLeft"), left, "the link moved the other");
    assert_ne!(rig.value("chainPanRight"), right);

    rig.backend.dispatch(UiAction::Undo).expect("undo");
    assert_eq!(rig.value("chainPanLeft"), left);
    assert_eq!(rig.value("chainPanRight"), right);
}

#[test]
fn an_alt_click_reset_of_a_linked_pan_is_one_undo_step() {
    let mut rig = Rig::new();
    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("stereo");
    rig.backend
        .set_parameter(
            ParameterId("chainPanLeft"),
            ronie_core::params::ParameterValue(0.3),
            ronie_ui::backend::GesturePhase::End,
        )
        .expect("pan");
    rig.backend
        .set_parameter(
            ParameterId("chainPanRight"),
            ronie_core::params::ParameterValue(0.7),
            ronie_ui::backend::GesturePhase::End,
        )
        .expect("pan");
    rig.driver.settle();
    let label = rig.knob_label("Pan L");
    let pos = rig
        .driver
        .controls()
        .iter()
        .find(|control| control.label == label)
        .expect("pan knob")
        .rect
        .center();
    let before = (rig.value("chainPanLeft"), rig.value("chainPanRight"));
    rig.alt_click(pos);
    assert_eq!(
        (rig.value("chainPanLeft"), rig.value("chainPanRight")),
        (0.0, 1.0),
        "the Alt-click reset both pans"
    );

    rig.backend.dispatch(UiAction::Undo).expect("undo");

    assert_eq!(
        (rig.value("chainPanLeft"), rig.value("chainPanRight")),
        before,
        "one undo takes both back"
    );
}

#[test]
fn a_block_trim_knob_drag_is_one_undo_step() {
    let mut rig = Rig::new();
    let target = rig.backend.chain_state(0).chain.left[0].id.clone();
    rig.backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: target.clone(),
            tone: ronie_core::Tone::from_value(serde_json::json!({
                "id": 6,
                "title": "Cab",
                "format": "ir",
                "models": [{"id": 8, "name": "Cab", "model_url": "https://models.example/8.wav"}],
            }))
            .expect("tone"),
            model_id: ronie_core::ModelId(8),
        })
        .expect("load");
    rig.driver
        .chrome_mut()
        .open_detail(&target.0)
        .expect("detail");
    rig.driver.settle();
    let before = rig
        .backend
        .chain_state(0)
        .chain
        .block(&target)
        .expect("block")
        .params
        .clone();

    let mix = rig.knob_label("Mix: ");
    rig.drag_up(&mix, 3);
    let input = rig.knob_label("In: ");
    rig.drag_up(&input, 3);

    let moved = rig
        .backend
        .chain_state(0)
        .chain
        .block(&target)
        .expect("block")
        .params
        .clone();
    assert_ne!(
        moved.input_gain, before.input_gain,
        "the drag moved the value"
    );
    rig.backend.dispatch(UiAction::Undo).expect("undo");
    let first = rig
        .backend
        .chain_state(0)
        .chain
        .block(&target)
        .expect("block")
        .params
        .clone();
    assert_eq!(
        first.input_gain, before.input_gain,
        "one undo for that drag"
    );
    assert_eq!(first.mix, moved.mix, "the other drag stays");
}

/// An opener that records each folder that the app asks the file manager to open.
fn recording_opener() -> (FolderOpener, Arc<Mutex<Vec<PathBuf>>>) {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&opened);
    let opener: FolderOpener = Arc::new(move |folder: &std::path::Path| {
        record.lock().expect("record").push(folder.to_path_buf());
        Ok(())
    });
    (opener, opened)
}

impl Rig {
    fn open_plugin_settings(&mut self, heading: &str) {
        let chrome = self.driver.chrome_mut();
        chrome.open_settings();
        chrome
            .settings_mut()
            .expect("settings page")
            .scroll_to_heading(heading);
        self.driver.settle();
    }
}

#[test]
fn the_real_app_shows_a_test_tone_and_no_control_panel_or_bluetooth_midi() {
    let mut rig = Rig::new();
    rig.driver.chrome_mut().open_system_settings();
    rig.driver.settle();
    assert!(
        rig.driver.is_visible("Output device"),
        "{:?}",
        rig.driver.visible_labels()
    );
    assert!(
        rig.driver.is_visible("Test"),
        "{:?}",
        rig.driver.visible_labels()
    );
    for label in ["Bluetooth MIDI", "Open Control Panel", "Allow Access"] {
        assert!(
            !rig.driver.is_visible(label),
            "{label} has no real action: {:?}",
            rig.driver.visible_labels()
        );
    }
}

#[test]
fn the_real_device_state_offers_no_banner_button_that_has_no_action() {
    let rig = Rig::new();
    let device = rig.backend.device_settings().expect("device state");
    assert!(!device.banner.mic_denied, "no adapter can open that page");
    assert!(!device.banner.asio_available, "this app has no ASIO");
    assert!(!device.has_control_panel);
    assert!(!device.bt_midi_available);
}

#[test]
fn the_real_app_shows_no_auto_balance() {
    let mut rig = Rig::new();
    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("stereo");
    rig.driver.settle();
    assert!(
        rig.driver.label_starting_with("Bal:").is_some(),
        "the balance knob stays: {:?}",
        rig.driver.visible_labels()
    );
    assert!(!rig.driver.is_visible("Auto balance"));
}

#[test]
fn the_mock_keeps_each_control_for_the_visual_scenarios() {
    let scenario =
        ronie_ui::testbed::load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "chrome-auto-balance")
            .expect("scenario");
    let backend = MockBackend::from_scenario(&scenario);
    assert!(backend.supports(HostControl::TestTone));
    assert!(backend.supports(HostControl::AutoBalance));
    assert_eq!(backend.diagnostic_log(), None);
    let chrome = ChromeController::new(Arc::new(backend.clone()), Vec::new(), false, true);
    assert!(UiDriver::new(chrome, backend).is_visible("Auto balance"));
}

#[test]
fn open_presets_folder_opens_the_folder_of_the_presets() {
    let (opener, opened) = recording_opener();
    let mut rig = Rig::with_opener(Some(opener));
    rig.open_plugin_settings("Presets");

    let _ = rig.driver.click("Open presets folder").expect("click");

    let presets = rig.root.path().join("presets");
    assert_eq!(
        *opened.lock().expect("record"),
        std::slice::from_ref(&presets)
    );
    assert!(
        presets.is_dir(),
        "the file manager needs the folder to exist"
    );
}

#[test]
fn reveal_logs_opens_the_folder_of_a_real_log_file() {
    let (opener, opened) = recording_opener();
    let mut rig = Rig::with_opener(Some(opener));
    rig.open_plugin_settings("Diagnostics");

    let _ = rig.driver.click("Reveal log file on disk").expect("click");

    let folder = rig.root.path().join("logs");
    assert_eq!(
        *opened.lock().expect("record"),
        std::slice::from_ref(&folder)
    );
    let text = std::fs::read_to_string(folder.join("ronnie.log")).expect("log file");
    assert!(text.contains("started"), "{text}");
}

#[test]
fn copy_logs_puts_the_log_text_on_the_clipboard() {
    let mut rig = Rig::new();
    rig.open_plugin_settings("Diagnostics");

    let interaction = rig.driver.click("Copy Logs").expect("click");

    assert_eq!(interaction.copied_text.len(), 1);
    let text = &interaction.copied_text[0];
    assert!(text.contains("started"), "{text}");
    assert!(text.contains("stream opened"), "{text}");
}

#[test]
fn a_file_manager_that_cannot_start_shows_a_message() {
    let failing: FolderOpener =
        Arc::new(|_: &std::path::Path| Err(UiError::Control("no file manager".to_owned())));
    let mut rig = Rig::with_opener(Some(failing));
    rig.open_plugin_settings("Presets");
    assert!(!rig.plugin_reports_a_failure());

    let _ = rig.driver.click("Open presets folder").expect("click");

    assert!(rig.plugin_reports_a_failure());
}

impl Rig {
    fn plugin_reports_a_failure(&mut self) -> bool {
        self.driver
            .chrome_mut()
            .settings_mut()
            .expect("settings page")
            .plugin_mut()
            .host_action_failed()
    }
}

#[test]
fn the_log_records_what_happens_to_the_stream() {
    let root = tempfile::tempdir().expect("temporary root");
    let device = ronie_app::audio::DummyBackend::connected();
    let processor = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 128,
        oversampling: 1,
        input_mode: InputMode::Left,
        stereo_input: false,
        stereo_output: true,
    }));
    let backend = StandaloneBackend::with_dummy_device(
        processor,
        &root.path().join(PREFERENCES_SETTINGS),
        device.clone(),
    )
    .expect("backend");

    device.set_connected(false);
    let _ = backend.device_settings();
    let text = backend.diagnostic_log().expect("log");
    assert!(text.contains("stream lost"), "{text}");

    device.set_connected(true);
    let _ = backend.dispatch(UiAction::Device(DeviceAction::Restart));
    backend.settle_audio().expect("settle");
    let text = backend.diagnostic_log().expect("log");
    assert!(
        text.matches("stream opened").count() >= 2,
        "the first stream and the new stream: {text}"
    );
}

/// Renders 40 ms of a sine, and gives the page one frame and the time that one frame takes.
fn play_and_show(rig: &mut Rig, phase: &mut f32, hertz: f32) {
    let frames = rig
        .backend
        .device_settings()
        .expect("device")
        .banner
        .buffer_size as usize;
    let step = std::f32::consts::TAU * hertz / 48_000.0;
    for _ in 0..8 {
        let input: Vec<f32> = (0..frames)
            .map(|_| {
                *phase = (*phase + step) % std::f32::consts::TAU;
                0.25 * phase.sin()
            })
            .collect();
        rig.backend.render_dummy(&input).expect("render");
    }
    std::thread::sleep(std::time::Duration::from_millis(33));
    rig.driver.frame();
}

#[test]
fn the_tuner_page_shows_the_note_of_the_real_input() {
    let mut rig = Rig::new();
    assert!(!rig.driver.chrome().tuner().state().has_signal);
    rig.driver.chrome_mut().set_tuner(true);
    rig.driver.settle();
    let mut phase = 0.0;
    for _ in 0..12 {
        play_and_show(&mut rig, &mut phase, 110.0);
    }
    let state = rig.driver.chrome().tuner().state();
    assert!(state.has_signal);
    assert_eq!((state.note.as_str(), state.octave), ("A", 2));
    assert!(state.cents.abs() < 5.0, "{} cents", state.cents);
}

#[test]
fn the_eq_page_shows_the_spectrum_of_the_real_block() {
    let mut rig = Rig::new();
    let target = rig.backend.chain_state(0).chain.left[0].id.clone();
    let samples = [16_000_i16, 0, 0, 0];
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36_u32 + 8).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&8_u32.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    rig.backend
        .dispatch(UiAction::LoadToneWithBytes {
            target_insert_id: target.clone(),
            tone: ronie_core::Tone::from_value(serde_json::json!({
                "id": 6,
                "title": "Cab",
                "format": "ir",
                "models": [{"id": 8, "name": "Cab", "model_url": "https://models.example/8.wav"}],
            }))
            .expect("tone"),
            model_id: ronie_core::ModelId(8),
            bytes: bytes.into(),
        })
        .expect("load");
    rig.backend.settle_audio().expect("settle");
    rig.driver
        .chrome_mut()
        .open_detail(&target.0)
        .expect("detail");
    rig.driver
        .chrome_mut()
        .detail_mut()
        .expect("detail")
        .set_view(ronie_ui::default::BlockView::Eq);
    rig.driver.settle();

    let mut phase = 0.0;
    for _ in 0..12 {
        play_and_show(&mut rig, &mut phase, 1_000.0);
    }

    let detail = rig.driver.chrome_mut().detail_mut().expect("detail");
    let bins = detail.eq_mut().spectrum_bins();
    let loudest = bins
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .expect("bins");
    let hertz = 20.0 * (1000.0_f64.ln() * (loudest.0 as f64 + 0.5) / 64.0).exp();
    assert!(*loudest.1 > -40.0, "{bins:?}");
    assert!((700.0..1_400.0).contains(&hertz), "{hertz} Hz: {bins:?}");
}

/// Loads one real impulse response in the first slot and returns the id of the block.
fn load_block(rig: &mut Rig) -> ronie_core::BlockId {
    let target = rig
        .backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|block| block.is_insert)
        .expect("an empty slot")
        .id
        .clone();
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36_u32 + 8).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&8_u32.to_le_bytes());
    for sample in [16_000_i16, 0, 0, 0] {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    rig.backend
        .dispatch(UiAction::LoadToneWithBytes {
            target_insert_id: target.clone(),
            tone: ronie_core::Tone::from_value(serde_json::json!({
                "id": 6,
                "title": "Cab",
                "format": "ir",
                "models": [{"id": 8, "name": "Cab", "model_url": "https://models.example/8.wav"}],
            }))
            .expect("tone"),
            model_id: ronie_core::ModelId(8),
            bytes: bytes.into(),
        })
        .expect("load");
    rig.backend.settle_audio().expect("settle");
    rig.driver.settle();
    target
}

impl Rig {
    fn block_titles(&self) -> Vec<String> {
        self.backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .filter_map(|block| block.tone.as_ref().map(|tone| tone.title.clone()))
            .collect()
    }

    fn center_of(&self, label: &str) -> egui::Pos2 {
        self.driver
            .controls()
            .iter()
            .find(|control| control.label == label)
            .unwrap_or_else(|| panic!("no {label} in {:?}", self.driver.visible_labels()))
            .rect
            .center()
    }

    /// Plays what a mouse sends: a move, a press, and a release, each in its own frame.
    fn pointer_click(&mut self, pos: egui::Pos2, button: egui::PointerButton) {
        self.driver.play(vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.driver.play(vec![egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        self.driver.settle();
    }

    /// Moves the pointer in small steps, because a real pointer crosses each pixel in between.
    fn pointer_travel(&mut self, from: egui::Pos2, to: egui::Pos2, steps: usize) {
        for step in 0..=steps {
            let pos = from + (to - from) * step as f32 / steps as f32;
            self.driver.play(vec![egui::Event::PointerMoved(pos)]);
        }
    }
}

#[test]
fn the_remove_button_of_a_tile_takes_a_real_pointer_and_undo_restores_the_block() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);
    assert_eq!(rig.block_titles(), ["Cab"]);

    // The pointer comes in from outside of the tile, and each frame is a real hit test.
    let tile = rig.center_of("Cab");
    rig.pointer_travel(tile + egui::vec2(0.0, 200.0), tile, 8);
    let remove = rig.center_of("Remove block");
    rig.pointer_travel(tile, remove, 8);
    assert!(
        rig.driver.is_visible("Remove block"),
        "the button stays while the pointer is on it: {:?}",
        rig.driver.visible_labels()
    );
    rig.pointer_click(remove, egui::PointerButton::Primary);

    assert!(rig.block_titles().is_empty(), "{:?}", rig.block_titles());
    rig.backend.dispatch(UiAction::Undo).expect("undo");
    rig.driver.settle();
    assert_eq!(rig.block_titles(), ["Cab"]);
}

#[test]
fn the_context_menu_remove_takes_a_real_pointer_and_undo_restores_the_block() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);

    let tile = rig.center_of("Cab");
    rig.pointer_click(tile, egui::PointerButton::Secondary);
    let remove = rig.center_of("Remove");
    rig.pointer_travel(tile, remove, 8);
    rig.pointer_click(remove, egui::PointerButton::Primary);

    assert!(rig.block_titles().is_empty(), "{:?}", rig.block_titles());
    rig.backend.dispatch(UiAction::Undo).expect("undo");
    rig.driver.settle();
    assert_eq!(rig.block_titles(), ["Cab"]);
}

#[test]
fn the_remove_button_of_the_block_page_takes_a_real_pointer_and_undo_restores_the_block() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);

    let tile = rig.center_of("Cab");
    rig.pointer_click(tile, egui::PointerButton::Primary);
    assert!(rig.driver.chrome().detail().is_some(), "the tile opened");
    let remove = rig.center_of("Remove block");
    rig.pointer_travel(tile, remove, 8);
    rig.pointer_click(remove, egui::PointerButton::Primary);

    assert!(rig.block_titles().is_empty(), "{:?}", rig.block_titles());
    assert!(
        rig.driver.chrome().detail().is_none(),
        "the app went back to the chain"
    );
    rig.backend.dispatch(UiAction::Undo).expect("undo");
    rig.driver.settle();
    assert_eq!(rig.block_titles(), ["Cab"]);
}

#[test]
fn the_power_and_swap_buttons_of_a_tile_take_a_real_pointer() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    let enabled = |rig: &Rig| {
        rig.backend
            .chain_state(0)
            .chain
            .block(&block)
            .expect("block")
            .params
            .enabled
    };
    assert!(enabled(&rig));

    let tile = rig.center_of("Cab");
    rig.pointer_travel(tile + egui::vec2(0.0, 200.0), tile, 8);
    let power = rig.center_of("Block power");
    rig.pointer_click(power, egui::PointerButton::Primary);
    assert!(!enabled(&rig), "the power button took the click");

    let swap = rig.center_of("Swap tone");
    rig.pointer_click(swap, egui::PointerButton::Primary);
    assert!(
        rig.driver.chrome().gallery().selection() != &ronie_ui::default::GallerySelection::None,
        "the swap button asks for a tone"
    );
}

#[test]
fn a_press_and_release_in_one_frame_still_reaches_a_tile_button() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);
    let tile = rig.center_of("Cab");
    rig.pointer_travel(tile + egui::vec2(0.0, 200.0), tile, 8);
    let remove = rig.center_of("Remove block");

    rig.driver.play(vec![egui::Event::PointerMoved(remove)]);
    let button = |pressed| egui::Event::PointerButton {
        pos: remove,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    rig.driver.play(vec![button(true), button(false)]);
    rig.driver.settle();

    assert!(rig.block_titles().is_empty(), "{:?}", rig.block_titles());
}

#[test]
fn a_click_on_a_tile_button_does_not_open_the_block_page() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);
    let tile = rig.center_of("Cab");
    rig.pointer_travel(tile + egui::vec2(0.0, 200.0), tile, 8);
    let power = rig.center_of("Block power");
    rig.pointer_click(power, egui::PointerButton::Primary);
    assert!(rig.driver.chrome().detail().is_none());
}

#[test]
fn the_quick_actions_leave_with_the_pointer() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);
    let tile = rig.center_of("Cab");
    rig.pointer_travel(tile + egui::vec2(0.0, 200.0), tile, 8);
    assert!(rig.driver.is_visible("Remove block"));
    rig.driver.play(vec![egui::Event::PointerMoved(
        tile + egui::vec2(0.0, 200.0),
    )]);
    rig.driver.frame();
    assert!(!rig.driver.is_visible("Remove block"));
}

#[test]
fn a_drag_over_another_tile_shows_no_quick_actions_on_that_tile() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);
    let _ = load_block(&mut rig);
    let tile = rig.center_of("Cab");
    let actions = |rig: &Rig| {
        rig.driver
            .controls()
            .iter()
            .filter(|control| control.label == "Remove block")
            .count()
    };
    rig.driver.play(vec![egui::Event::PointerMoved(tile)]);
    rig.driver.play(vec![egui::Event::PointerButton {
        pos: tile,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    }]);
    // The tile is 224 points wide and the second tile starts 248 points to the right.
    rig.pointer_travel(tile, tile + egui::vec2(248.0, 0.0), 8);

    assert_eq!(actions(&rig), 0, "{:?}", rig.driver.visible_labels());
}

#[test]
fn the_undo_button_takes_a_real_pointer_after_a_remove() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);
    let tile = rig.center_of("Cab");
    rig.pointer_travel(tile + egui::vec2(0.0, 200.0), tile, 8);
    let remove = rig.center_of("Remove block");
    rig.pointer_click(remove, egui::PointerButton::Primary);
    assert!(rig.block_titles().is_empty());

    let undo = rig.center_of("Undo");
    rig.pointer_click(undo, egui::PointerButton::Primary);

    assert_eq!(rig.block_titles(), ["Cab"]);
}

impl Rig {
    fn input_gain(&self, block: &ronie_core::BlockId) -> f64 {
        self.backend
            .chain_state(0)
            .chain
            .block(block)
            .expect("block")
            .params
            .input_gain
    }

    fn press(&mut self, pos: egui::Pos2, pressed: bool) {
        self.driver.play(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
}

#[test]
fn a_drag_on_the_tile_gain_knob_is_one_undo_step_and_moves_no_tile() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    let before = rig.backend.chain_state(0).chain.left.clone();
    let knob = rig.center_of("Cab gain: 0.0 dB");

    // The pointer comes in from outside the tile, presses, and travels up in small steps.
    rig.pointer_travel(knob + egui::vec2(0.0, 300.0), knob, 8);
    assert!(
        rig.driver.is_visible("Remove block"),
        "the quick actions stay"
    );
    rig.press(knob, true);
    let top = knob - egui::vec2(0.0, 40.0);
    rig.pointer_travel(knob, top, 8);
    let during = rig.driver.visible_labels();
    assert!(
        during.iter().any(|label| label.starts_with("Cab gain: +")),
        "{during:?}"
    );
    assert!(
        rig.driver.is_visible("Remove block"),
        "the quick actions stay"
    );
    rig.press(top, false);
    rig.driver.settle();

    assert!(rig.input_gain(&block) > 0.5, "the drag raised the gain");
    assert!(
        rig.driver.chrome().detail().is_none(),
        "no tone page opened"
    );
    let after = rig.backend.chain_state(0).chain.left.clone();
    assert_eq!(
        after.iter().map(|block| &block.id).collect::<Vec<_>>(),
        before.iter().map(|block| &block.id).collect::<Vec<_>>(),
        "no tile moved"
    );
    assert_eq!(
        rig.undo_steps(),
        2,
        "one step for the load and one for the drag"
    );
}

impl Rig {
    /// Sets the input gain of a block outside of the UI, and shows it.
    fn set_input_gain(&mut self, block: &ronie_core::BlockId, gain: f64) {
        let mut params = self
            .backend
            .chain_state(0)
            .chain
            .block(block)
            .expect("block")
            .params
            .clone();
        params.input_gain = gain;
        self.backend
            .dispatch(UiAction::SetBlockParams {
                block_id: block.clone(),
                params,
            })
            .expect("params");
        self.driver.settle();
    }

    /// Plays an Alt-click, with the Alt key down only around the click.
    fn alt_click(&mut self, pos: egui::Pos2) {
        self.driver
            .play(vec![egui::Event::ModifiersChanged(egui::Modifiers::ALT)]);
        self.pointer_click(pos, egui::PointerButton::Primary);
        self.driver
            .play(vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
        self.driver.settle();
    }

    /// Plays a double-click, types the text in the field that opens, and presses Enter.
    fn type_in(&mut self, pos: egui::Pos2, field: &str, text: &str) {
        self.pointer_click(pos, egui::PointerButton::Primary);
        self.pointer_click(pos, egui::PointerButton::Primary);
        assert!(
            self.driver.is_visible(field),
            "the double-click opens the field: {:?}",
            self.driver.visible_labels()
        );
        self.driver.play(vec![egui::Event::Text(text.to_owned())]);
        let _ = self.driver.press_key(egui::Key::Enter);
    }
}

#[test]
fn an_alt_click_on_the_tile_gain_knob_sets_zero_db_in_one_undo_step() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    rig.set_input_gain(&block, 0.75);
    let knob = rig.center_of("Cab gain: +12.0 dB");

    rig.alt_click(knob);

    assert_eq!(rig.input_gain(&block), 0.5);
    assert!(
        rig.driver.chrome().detail().is_none(),
        "no tone page opened"
    );
    assert!(rig.driver.is_visible("Cab gain: 0.0 dB"));
    rig.backend.dispatch(UiAction::Undo).expect("undo");
    assert_eq!(
        rig.input_gain(&block),
        0.75,
        "one undo takes the reset back"
    );
}

#[test]
fn a_double_click_on_the_tile_gain_knob_opens_a_field_and_enter_commits_it() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    let knob = rig.center_of("Cab gain: 0.0 dB");

    rig.type_in(knob, "Cab gain value", "6");

    assert_eq!(rig.input_gain(&block), 0.625);
    assert!(rig.driver.is_visible("Cab gain: +6.0 dB"));
    assert!(!rig.driver.is_visible("Cab gain value"), "the field closed");
    assert!(
        rig.driver.chrome().detail().is_none(),
        "no tone page opened"
    );
    assert_eq!(
        rig.undo_steps(),
        2,
        "one step for the load and one for the typed value"
    );
}

#[test]
fn each_arrow_key_on_a_focused_faceplate_knob_is_one_undo_step() {
    let mut rig = Rig::new();
    let start = rig.value("inputLevel");
    rig.focus("Input: 0.0 dB");

    let _ = rig.driver.press_key(egui::Key::ArrowUp);
    assert!((rig.value("inputLevel") - (start + 0.01)).abs() < 1.0e-6);
    assert_eq!(
        rig.driver.focused(),
        Some("Input: +0.5 dB"),
        "the knob keeps the focus and its name shows the new value"
    );
    let _ = rig.driver.press_key(egui::Key::ArrowRight);

    assert_eq!(rig.undo_steps(), 2, "one step for each key");
    assert_eq!(rig.value("inputLevel"), start);
}

#[test]
fn a_double_click_on_a_faceplate_knob_opens_a_field_and_enter_commits_it() {
    let mut rig = Rig::new();
    let label = rig.knob_label("Input:");
    let knob = rig.center_of(&label);

    rig.type_in(knob, "Input value", "-12");

    assert_eq!(rig.value("inputLevel"), 0.25);
    assert!(rig.driver.is_visible("Input: -12.0 dB"));
    assert_eq!(rig.undo_steps(), 1);
}

#[test]
fn escape_during_a_faceplate_knob_drag_restores_the_value_and_keeps_no_undo_step() {
    let mut rig = Rig::new();
    let start = rig.value("inputLevel");
    let knob = rig.center_of(&rig.knob_label("Input:"));
    rig.driver.play(vec![egui::Event::PointerMoved(knob)]);
    rig.press(knob, true);
    rig.pointer_travel(knob, knob - egui::vec2(0.0, 40.0), 4);
    assert!(rig.value("inputLevel") > start, "the drag moved the value");

    let _ = rig.driver.press_key(egui::Key::Escape);
    rig.press(knob - egui::vec2(0.0, 40.0), false);
    rig.driver.settle();

    assert_eq!(rig.value("inputLevel"), start);
    assert_eq!(rig.undo_steps(), 0);
}

#[test]
fn an_empty_slot_has_no_gain_knob() {
    let mut rig = Rig::new();
    let _ = load_block(&mut rig);
    let knobs = rig
        .driver
        .controls()
        .iter()
        .filter(|control| control.label.contains(" gain: "))
        .count();
    assert_eq!(knobs, 1, "{:?}", rig.driver.visible_labels());
}

/// A tone shows as the real backend holds it.
const SHOWN_REAL: u8 = 0;
/// Each tone shows as loading.
const SHOWN_LOADING: u8 = 1;
/// Each tone shows as failed.
const SHOWN_FAILED: u8 = 2;

/// The real backend, where each tone shows as loading or as failed.
///
/// Only a fixture gives a tone these states. The real backend has the bytes of a tone before it
/// loads the tone, and a session keeps no load state. A test can change the state between two
/// frames.
struct ShownLoad {
    real: Arc<StandaloneBackend>,
    shown: Arc<AtomicU8>,
}

impl UiBackend for ShownLoad {
    fn chain_state(&self, known_revision: u64) -> ronie_ui::backend::ChainViewState {
        let mut state = self.real.chain_state(known_revision);
        let shown = self.shown.load(Ordering::Relaxed);
        if shown == SHOWN_REAL {
            return state;
        }
        let loading = shown == SHOWN_LOADING;
        // The snapshots are shared, so this view changes its own copies of them.
        let session = std::sync::Arc::make_mut(&mut state.session);
        for chain in [
            std::sync::Arc::make_mut(&mut state.chain),
            &mut session.chain,
        ] {
            for block in chain.left.iter_mut().chain(chain.right.iter_mut()) {
                if !block.is_insert {
                    block.model_loading = loading;
                    block.load_failed = !loading;
                }
            }
        }
        state
    }

    fn meter_levels(&self) -> ronie_ui::backend::MeterSnapshot {
        self.real.meter_levels()
    }

    fn supports(&self, control: HostControl) -> bool {
        self.real.supports(control)
    }

    fn diagnostic_log(&self) -> Option<String> {
        self.real.diagnostic_log()
    }

    fn device_settings(&self) -> Option<ronie_ui::default::SystemDeviceState> {
        self.real.device_settings()
    }

    fn preference(&self, key: &str) -> Option<bool> {
        self.real.preference(key)
    }

    fn dispatch(&self, action: UiAction) -> Result<ronie_ui::backend::UiReceipt, UiError> {
        self.real.dispatch(action)
    }

    fn settle_audio(&self) -> Result<(), UiError> {
        self.real.settle_audio()
    }

    fn can_paste_block(&self) -> bool {
        self.real.can_paste_block()
    }

    fn parameter(&self, id: ParameterId) -> Option<ronie_core::params::ParameterValue> {
        self.real.parameter(id)
    }

    fn set_parameter(
        &self,
        id: ParameterId,
        value: ronie_core::params::ParameterValue,
        gesture: ronie_ui::backend::GesturePhase,
    ) -> Result<(), UiError> {
        self.real.set_parameter(id, value, gesture)
    }
}

impl Rig {
    /// Shows the chain of this rig with each tone as loading or as failed.
    fn show_load(&mut self, loading: bool) {
        let _ = self.show_load_switch(if loading { SHOWN_LOADING } else { SHOWN_FAILED });
    }

    /// Shows the chain of this rig through a switch of the load state that a test can change.
    fn show_load_switch(&mut self, shown: u8) -> Arc<AtomicU8> {
        let switch = Arc::new(AtomicU8::new(shown));
        let backend = Arc::new(ShownLoad {
            real: self.backend.clone(),
            shown: switch.clone(),
        });
        let mut chrome = ChromeController::new(backend, Vec::new(), false, true);
        chrome.set_standalone(true);
        self.driver = UiDriver::on_real_backend(chrome);
        switch
    }

    /// Moves the keyboard focus with Tab until the control with this label has it.
    fn focus(&mut self, label: &str) {
        for _ in 0..100 {
            if self.driver.focused() == Some(label) {
                return;
            }
            let _ = self.driver.press_key(egui::Key::Tab);
        }
        panic!("Tab did not reach {label}");
    }
}

#[test]
fn a_loading_or_failed_tile_has_no_gain_knob() {
    for loading in [true, false] {
        let mut rig = Rig::new();
        let _ = load_block(&mut rig);
        rig.show_load(loading);

        assert!(rig.driver.is_visible("Cab"), "the tile shows");
        assert_eq!(
            rig.driver.is_visible("Retry model load"),
            !loading,
            "the tile shows its state"
        );
        assert_eq!(
            rig.driver.label_starting_with("Cab gain:"),
            None,
            "loading {loading}: {:?}",
            rig.driver.visible_labels()
        );
    }
}

#[test]
fn a_bypassed_tile_keeps_its_gain_knob() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    let mut params = rig
        .backend
        .chain_state(0)
        .chain
        .block(&block)
        .expect("block")
        .params
        .clone();
    params.enabled = false;
    rig.backend
        .dispatch(UiAction::SetBlockParams {
            block_id: block,
            params,
        })
        .expect("bypass");
    rig.driver.settle();

    assert!(
        rig.driver.is_visible("Cab gain: 0.0 dB"),
        "{:?}",
        rig.driver.visible_labels()
    );
}

#[test]
fn each_arrow_key_on_the_focused_tile_gain_knob_is_one_decibel_and_one_undo_step() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    rig.focus("Cab gain: 0.0 dB");

    let _ = rig.driver.press_key(egui::Key::ArrowUp);
    assert_eq!(rig.input_gain(&block), 0.5 + 1.0 / 48.0);
    assert_eq!(
        rig.driver.focused(),
        Some("Cab gain: +1.0 dB"),
        "the knob keeps the focus and its name shows the new value"
    );

    let _ = rig.driver.press_key(egui::Key::ArrowLeft);
    assert_eq!(rig.input_gain(&block), 0.5);
    assert!(
        rig.driver.chrome().detail().is_none(),
        "no tone page opened"
    );
    assert_eq!(
        rig.undo_steps(),
        3,
        "one step for the load and one for each key"
    );
}

impl Rig {
    /// Whether the backend has an open gesture. Only an open gesture can take a cancel.
    fn gesture_open(&self) -> bool {
        self.backend
            .set_parameter(
                ParameterId("inputLevel"),
                ronie_core::params::ParameterValue(self.value("inputLevel")),
                ronie_ui::backend::GesturePhase::Cancel,
            )
            .is_ok()
    }

    /// Presses on a control and moves up by ten points in each of `steps` frames. The button
    /// stays down.
    fn press_and_drag_up(&mut self, label: &str, steps: usize) -> egui::Pos2 {
        let start = self.center_of(label);
        self.driver.play(vec![egui::Event::PointerMoved(start)]);
        self.press(start, true);
        let end = start - egui::vec2(0.0, 10.0 * steps as f32);
        self.pointer_travel(start, end, steps);
        end
    }

    fn show_pitch_knob(&mut self, shown: bool) {
        self.backend
            .dispatch(UiAction::SetPreference {
                key: "t3k.showPitchControl".to_owned(),
                value: shown,
            })
            .expect("preference");
    }

    /// Loads one tone and shows its detail page.
    fn open_detail_of_new_block(&mut self) -> ronie_core::BlockId {
        let block = load_block(self);
        self.driver
            .chrome_mut()
            .open_detail(&block.0)
            .expect("detail");
        self.driver.settle();
        block
    }
}

#[test]
fn a_faceplate_knob_that_hides_during_its_drag_closes_its_gesture() {
    let mut rig = Rig::new();
    rig.show_pitch_knob(true);
    rig.driver.settle();
    let pitch = rig.knob_label("Pitch:");
    let end = rig.press_and_drag_up(&pitch, 3);
    let moved = rig.value("pitchSemitones");
    assert!(moved > 0.0, "the drag moved the pitch");

    rig.show_pitch_knob(false);
    rig.pointer_travel(end, end - egui::vec2(0.0, 30.0), 3);
    rig.press(end - egui::vec2(0.0, 30.0), false);
    rig.driver.settle();
    assert!(!rig.gesture_open(), "the hidden knob closed its gesture");
    assert_eq!(rig.value("pitchSemitones"), moved, "no more motion");

    // A later key press and a later drag of another knob are steps of their own.
    let input = rig.knob_label("Input:");
    rig.pointer_click(rig.center_of(&input), egui::PointerButton::Primary);
    let _ = rig.driver.press_key(egui::Key::ArrowRight);
    rig.drag_up(&rig.knob_label("Input:"), 2);
    assert_eq!(rig.undo_steps(), 3, "the pitch drag, the key, and the drag");
    assert_eq!(rig.value("pitchSemitones"), 0.0);
}

#[test]
fn a_tile_gain_knob_that_hides_during_its_drag_closes_its_gesture() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    let switch = rig.show_load_switch(SHOWN_REAL);
    rig.driver.settle();
    let end = rig.press_and_drag_up("Cab gain: 0.0 dB", 4);
    let moved = rig.input_gain(&block);
    assert!(moved > 0.5, "the drag moved the gain");

    switch.store(SHOWN_LOADING, Ordering::Relaxed);
    rig.pointer_travel(end, end - egui::vec2(0.0, 20.0), 2);
    switch.store(SHOWN_REAL, Ordering::Relaxed);
    rig.pointer_travel(end - egui::vec2(0.0, 20.0), end - egui::vec2(0.0, 40.0), 2);
    rig.press(end - egui::vec2(0.0, 40.0), false);
    rig.driver.settle();

    assert!(!rig.gesture_open(), "the hidden knob closed its gesture");
    assert_eq!(
        rig.input_gain(&block),
        moved,
        "the old drag sends nothing more"
    );
    let _ = rig.driver.press_key(egui::Key::ArrowUp);
    assert_eq!(
        rig.input_gain(&block),
        moved,
        "the hidden knob lost the focus"
    );
    assert_eq!(
        rig.undo_steps(),
        2,
        "one step for the load and one for the drag"
    );
}

#[test]
fn a_trim_knob_that_hides_during_its_drag_closes_its_gesture() {
    let mut rig = Rig::new();
    let block = rig.open_detail_of_new_block();
    let input = rig.knob_label("In: ");
    let end = rig.press_and_drag_up(&input, 3);
    let moved = rig.input_gain(&block);
    assert!(moved > 0.5, "the drag moved the gain");

    let detail = rig.driver.chrome_mut().detail_mut().expect("detail");
    detail.set_view(ronie_ui::default::BlockView::Eq);
    rig.pointer_travel(end, end - egui::vec2(0.0, 20.0), 2);
    rig.press(end - egui::vec2(0.0, 20.0), false);
    rig.driver.settle();

    assert!(!rig.gesture_open(), "the hidden knob closed its gesture");
    assert_eq!(rig.input_gain(&block), moved);
    assert_eq!(
        rig.undo_steps(),
        2,
        "one step for the load and one for the drag"
    );
}

#[test]
fn a_closed_detail_closes_the_gesture_of_its_trim_drag() {
    let mut rig = Rig::new();
    let _ = rig.open_detail_of_new_block();
    let input = rig.knob_label("In: ");
    let end = rig.press_and_drag_up(&input, 3);

    rig.driver
        .chrome_mut()
        .detail_mut()
        .expect("detail")
        .close();
    rig.driver.settle();
    rig.press(end, false);
    rig.driver.settle();

    assert!(!rig.gesture_open());
    assert_eq!(rig.undo_steps(), 2);
}

impl Rig {
    /// Sets the output level outside of the UI, as one edit that is not a part of a drag.
    fn edit_output_level(&self) {
        self.backend
            .set_parameter(
                ParameterId("outputLevel"),
                ronie_core::params::ParameterValue(0.25),
                ronie_ui::backend::GesturePhase::End,
            )
            .expect("output level");
    }
}

#[test]
fn a_browser_that_covers_the_faceplate_during_a_knob_drag_closes_its_gesture_at_once() {
    let mut rig = Rig::new();
    let start = rig.value("inputLevel");
    let insert = rig.backend.chain_state(0).chain.left[0].id.clone();
    let input = rig.knob_label("Input:");
    let end = rig.press_and_drag_up(&input, 3);
    let moved = rig.value("inputLevel");
    assert!(moved > start, "the drag moved the value");

    // One pass opens the browser, and the next pass shows no faceplate. The button stays down.
    rig.driver
        .chrome_mut()
        .gallery_mut()
        .select_tone(ronie_core::ChainSide::Left, insert);
    rig.pointer_travel(end, end - egui::vec2(0.0, 20.0), 1);
    assert!(
        rig.driver.chrome().browser().is_some(),
        "the browser is open"
    );
    assert_eq!(
        rig.driver.label_starting_with("Input:"),
        None,
        "the browser covers the faceplate"
    );

    assert!(!rig.gesture_open(), "the covered knob closed its gesture");
    assert_eq!(rig.value("inputLevel"), moved, "no more motion");
    rig.edit_output_level();
    assert_eq!(rig.undo_steps(), 2, "the drag and the next edit");
}

#[test]
fn a_tuner_that_covers_the_chain_during_a_tile_gain_drag_closes_its_gesture_at_once() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    let end = rig.press_and_drag_up("Cab gain: 0.0 dB", 4);
    let moved = rig.input_gain(&block);
    assert!(moved > 0.5, "the drag moved the gain");

    // The first pass with the tuner shows no chain. The button stays down.
    rig.driver.chrome_mut().set_tuner(true);
    rig.driver
        .play(vec![egui::Event::PointerMoved(end - egui::vec2(0.0, 10.0))]);
    assert_eq!(
        rig.driver.label_starting_with("Cab gain:"),
        None,
        "the tuner covers the chain"
    );

    assert!(!rig.gesture_open(), "the covered knob closed its gesture");
    assert_eq!(rig.input_gain(&block), moved, "no more motion");
    rig.edit_output_level();
    assert_eq!(rig.undo_steps(), 3, "the load, the drag, and the next edit");
}

#[test]
fn escape_during_a_linked_pan_drag_restores_both_pans_and_keeps_no_step() {
    let mut rig = Rig::new();
    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("stereo");
    rig.driver.settle();
    assert_eq!(rig.undo_steps(), 1);
    rig.backend
        .dispatch(UiAction::Redo)
        .expect("back to stereo");
    rig.driver.settle();
    assert!(rig.value("chainPanLinked") > 0.5, "the pans are linked");
    let before = (rig.value("chainPanLeft"), rig.value("chainPanRight"));

    let label = rig.knob_label("Pan R");
    let start = rig.center_of(&label);
    rig.driver.play(vec![egui::Event::PointerMoved(start)]);
    rig.press(start, true);
    rig.pointer_travel(start, start + egui::vec2(0.0, 40.0), 4);
    assert_ne!(
        (rig.value("chainPanLeft"), rig.value("chainPanRight")),
        before,
        "the drag moved both pans"
    );

    let _ = rig.driver.press_key(egui::Key::Escape);
    rig.press(start + egui::vec2(0.0, 40.0), false);
    rig.driver.settle();

    assert_eq!(
        (rig.value("chainPanLeft"), rig.value("chainPanRight")),
        before
    );
    assert!(!rig.gesture_open());
    assert_eq!(rig.undo_steps(), 1, "only the stereo mode is a step");
}

#[test]
fn a_click_on_a_faceplate_knob_lets_the_arrow_keys_step_it() {
    let mut rig = Rig::new();
    let start = rig.value("inputLevel");
    let knob = rig.center_of(&rig.knob_label("Input:"));
    rig.pointer_click(knob, egui::PointerButton::Primary);

    let _ = rig.driver.press_key(egui::Key::ArrowRight);

    assert!((rig.value("inputLevel") - (start + 0.01)).abs() < 1.0e-6);
    assert_eq!(rig.undo_steps(), 1);
}

#[test]
fn a_click_elsewhere_takes_the_focus_from_the_knob() {
    let mut rig = Rig::new();
    let start = rig.value("inputLevel");
    let knob = rig.center_of(&rig.knob_label("Input:"));
    rig.pointer_click(knob, egui::PointerButton::Primary);
    let _ = rig.driver.click_empty_space();

    let _ = rig.driver.press_key(egui::Key::ArrowRight);

    assert_eq!(rig.value("inputLevel"), start);
    assert_eq!(rig.undo_steps(), 0);
}

#[test]
fn the_arrow_keys_move_the_focus_from_a_tile_after_a_knob_had_the_focus() {
    let mut rig = Rig::new();
    let block = load_block(&mut rig);
    let knob = rig.center_of(&rig.knob_label("Input:"));
    rig.pointer_click(knob, egui::PointerButton::Primary);
    let _ = rig.driver.click_empty_space();
    rig.focus("Cab");

    let _ = rig.driver.press_key(egui::Key::ArrowRight);

    let focused = rig.driver.focused().map(str::to_owned);
    assert!(
        focused.as_deref().is_some_and(|label| label != "Cab"),
        "the arrow key moved the focus: {focused:?}"
    );
    assert_eq!(rig.value("inputLevel"), 0.5, "no knob took the key");
    assert_eq!(rig.input_gain(&block), 0.5, "no knob took the key");
}
