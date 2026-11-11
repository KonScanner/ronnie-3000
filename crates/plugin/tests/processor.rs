//! Reference parameter and processor contract tests.

use ronie_core::{
    InputMode,
    params::{Mapping, ParameterCatalog, ParameterId, ParameterValue, Smoothing},
};
use ronie_dsp::PrepareSpec;
use ronie_plugin::{
    backend::{ProcessorBackend, UiBackend},
    params,
    processor::RonieProcessor,
};

#[test]
fn settings_store_seeds_and_persists_machine_defaults_and_preferences() {
    let directory =
        std::env::temp_dir().join(format!("ronie-processor-settings-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create settings directory");
    let settings = ronie_core::SettingsStore::new(directory.join("preferences.settings"));
    settings
        .merge_key("calibrateInput", "1.0")
        .expect("seed calibration");
    settings.merge_key("osFactor", "2.0").expect("seed rate");
    settings
        .merge_key("multiCore", "false")
        .expect("seed multicore");
    settings
        .merge_key("namSlimSizeDefault", "1.0")
        .expect("seed size");
    settings
        .merge_key("t3k.showPitchControl", "true")
        .expect("seed preference");
    let backend = ProcessorBackend::with_settings(RonieProcessor::new(spec()), settings.clone())
        .expect("settings backend");
    assert_eq!(
        backend.parameter(ParameterId("calibrateInput")),
        Some(ParameterValue(1.0))
    );
    assert_eq!(
        backend.parameter(ParameterId("osFactor")),
        Some(ParameterValue(2.0))
    );
    assert!(!backend.chain_state(0).multi_core);
    assert!(backend.chain_state(0).nam_full_size);
    assert_eq!(backend.preference("t3k.showPitchControl"), Some(true));

    backend
        .dispatch(ronie_ui::backend::UiAction::SetParameter {
            id: ParameterId("inputCalibrationLevel"),
            value: ParameterValue(13.5),
            gesture: ronie_ui::backend::GesturePhase::Change,
        })
        .expect("calibration value");
    backend
        .dispatch(ronie_ui::backend::UiAction::PersistMachineDefault(
            ParameterId("inputCalibrationLevel"),
        ))
        .expect("persist calibration");
    backend
        .dispatch(ronie_ui::backend::UiAction::SetMultiCore(true))
        .expect("persist multicore");
    backend
        .dispatch(ronie_ui::backend::UiAction::SetNamFullSize(false))
        .expect("persist size");
    backend
        .dispatch(ronie_ui::backend::UiAction::SetPreference {
            key: "t3k.showPitchControl".to_owned(),
            value: false,
        })
        .expect("persist preference");
    let properties = settings.read().expect("stored properties");
    assert_eq!(properties.get("inputCalibrationLevel"), Some("13.5"));
    assert!(properties.get_bool("multiCore", false));
    assert_eq!(properties.get("namSlimSizeDefault"), Some("0.0"));
    assert!(!properties.get_bool("t3k.showPitchControl", true));
    std::fs::remove_dir_all(&directory).expect("clean settings directory");
}

fn spec() -> PrepareSpec {
    PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 64,
        oversampling: 1,
        input_mode: InputMode::Stereo,
        stereo_input: false,
        stereo_output: true,
    }
}

#[test]
fn complete_parameter_catalog_matches_reference_v1() {
    let catalog = ParameterCatalog::v1();
    assert_eq!(catalog.len(), 43);
    for (index, parameter) in catalog.iter().enumerate() {
        assert_eq!(parameter.version_hint as usize, index + 1);
    }

    let check = |index: usize,
                 id: &'static str,
                 min: f32,
                 max: f32,
                 default: f32,
                 interval: Option<f32>,
                 mapping: Mapping,
                 smoothing: Smoothing,
                 automatable: bool,
                 preset: bool,
                 machine_default: bool| {
        let parameter = &catalog[index - 1];
        assert_eq!(parameter.id, ParameterId(id), "id at {index}");
        assert_eq!(parameter.min, min, "min for {id}");
        assert_eq!(parameter.max, max, "max for {id}");
        assert_eq!(parameter.default, default, "default for {id}");
        assert_eq!(parameter.interval, interval, "interval for {id}");
        assert_eq!(parameter.mapping, mapping, "mapping for {id}");
        assert_eq!(parameter.smoothing, smoothing, "smoothing for {id}");
        assert_eq!(parameter.automatable, automatable, "automation for {id}");
        assert!(parameter.scope.session, "session scope for {id}");
        assert_eq!(parameter.scope.preset, preset, "preset scope for {id}");
        assert_eq!(
            parameter.scope.machine_default, machine_default,
            "machine scope for {id}"
        );
    };

    check(
        1,
        "inputLevel",
        0.0,
        1.0,
        0.5,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        2,
        "toneBass",
        0.0,
        10.0,
        5.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        3,
        "toneMid",
        0.0,
        10.0,
        5.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        4,
        "toneTreble",
        0.0,
        10.0,
        5.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        5,
        "outputLevel",
        0.0,
        1.0,
        0.5,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        true,
        false,
    );
    check(
        6,
        "gateThreshold",
        -100.0,
        0.0,
        -80.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        7,
        "targetLoudness",
        -60.0,
        0.0,
        -18.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        false,
        false,
    );
    check(
        8,
        "calibrateInput",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        false,
        true,
    );
    check(
        9,
        "inputCalibrationLevel",
        -60.0,
        60.0,
        12.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        false,
        true,
    );
    check(
        10,
        "gateEnabled",
        0.0,
        1.0,
        1.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        11,
        "toneEqEnabled",
        0.0,
        1.0,
        1.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        12,
        "outputBalance",
        0.0,
        1.0,
        0.5,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        true,
        false,
    );
    check(
        13,
        "spreadEnabled",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        14,
        "spreadOffset",
        0.0,
        1.0,
        0.8125,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Milliseconds(100),
        true,
        true,
        false,
    );
    check(
        15,
        "spreadWobble",
        0.0,
        1.0,
        0.25,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        16,
        "spreadWobbleEnabled",
        0.0,
        1.0,
        1.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        17,
        "spreadCrossover",
        0.0,
        1.0,
        0.5,
        Some(0.0001),
        Mapping::Logarithmic,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        18,
        "spreadCrossoverEnabled",
        0.0,
        1.0,
        1.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        19,
        "spreadDiffuseEnabled",
        0.0,
        1.0,
        1.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        20,
        "alignEnabled",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        21,
        "alignOffset",
        0.0,
        1.0,
        0.5,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Milliseconds(40),
        true,
        true,
        false,
    );
    check(
        22,
        "alignWobble",
        0.0,
        1.0,
        0.25,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        23,
        "alignWobbleEnabled",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        24,
        "alignCrossover",
        0.0,
        1.0,
        0.5,
        Some(0.0001),
        Mapping::Logarithmic,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        25,
        "alignCrossoverEnabled",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        26,
        "alignDiffuseEnabled",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(25),
        true,
        true,
        false,
    );
    check(
        27,
        "chainPanLeft",
        0.0,
        1.0,
        0.0,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        true,
        false,
    );
    check(
        28,
        "chainPanRight",
        0.0,
        1.0,
        1.0,
        Some(0.0001),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        true,
        false,
    );
    check(
        29,
        "chainPanLinked",
        0.0,
        1.0,
        1.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        true,
        false,
    );
    check(
        30,
        "chainSoloLeft",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        false,
        false,
    );
    check(
        31,
        "chainSoloRight",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        false,
        false,
    );
    check(
        32,
        "chainInvertLeft",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        true,
        false,
    );
    check(
        33,
        "chainInvertRight",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Milliseconds(20),
        true,
        true,
        false,
    );
    check(
        34,
        "osEnabled",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Immediate,
        false,
        false,
        true,
    );
    check(
        35,
        "osFactor",
        0.0,
        2.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Immediate,
        false,
        false,
        true,
    );
    check(
        36,
        "gateRelease",
        5.0,
        500.0,
        50.0,
        None,
        Mapping::Logarithmic,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        37,
        "gateHold",
        0.0,
        200.0,
        20.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        38,
        "gateRange",
        20.0,
        80.0,
        80.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        39,
        "pitchEnabled",
        0.0,
        1.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        40,
        "pitchSemitones",
        -24.0,
        24.0,
        0.0,
        None,
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        41,
        "pitchStep",
        0.0,
        1.0,
        1.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        42,
        "pitchTonality",
        1000.0,
        20000.0,
        20000.0,
        None,
        Mapping::Logarithmic,
        Smoothing::Immediate,
        true,
        true,
        false,
    );
    check(
        43,
        "pitchWindow",
        0.0,
        3.0,
        0.0,
        Some(1.0),
        Mapping::Linear,
        Smoothing::Immediate,
        false,
        true,
        false,
    );

    assert_eq!(catalog[34].choices, &["2x", "4x", "8x"]);
    assert_eq!(catalog[42].choices, &["20 ms", "30 ms", "40 ms", "60 ms"]);
    assert_eq!(params::catalog(), catalog);
}

#[test]
fn processor_tracks_parameters_gestures_and_revision() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    let initial = backend.chain_state(u64::MAX);
    assert!(initial.changed);
    assert_eq!(initial.preparation, spec());
    let revision = initial.revision;

    backend
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.75),
            params::GesturePhase::Begin,
        )
        .expect("known parameter accepts a gesture");
    backend
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.8),
            params::GesturePhase::End,
        )
        .expect("gesture ends");
    let actual = backend
        .parameter(ParameterId("inputLevel"))
        .expect("parameter value");
    assert!((actual.0 - 0.8).abs() < 0.0001);
    assert!(backend.chain_state(revision).revision > revision);
    assert!(
        backend
            .set_parameter(
                ParameterId("not-a-parameter"),
                ParameterValue(1.0),
                params::GesturePhase::Change
            )
            .is_err()
    );
}

#[test]
fn processor_presets_and_chain_actions_have_typed_receipts() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    let saved = backend
        .dispatch(params::UiAction::SavePreset {
            name: "Stadium Lead".to_owned(),
        })
        .expect("preset saves");
    assert!(saved.revision > 0);
    assert_eq!(saved.effect, params::UiEffect::PresetSaved);

    let stereo = backend
        .dispatch(params::UiAction::SetStereoMode(true))
        .expect("stereo changes");
    assert_eq!(stereo.effect, params::UiEffect::ChainChanged);
    assert!(backend.chain_state(0).chain.stereo_enabled);

    let input = backend
        .dispatch(params::UiAction::SetInputMode(InputMode::DualMono))
        .expect("input mode changes");
    assert_eq!(input.effect, params::UiEffect::ChainChanged);
    assert_eq!(
        backend.chain_state(0).session.input_mode,
        InputMode::DualMono
    );

    let undone = backend
        .dispatch(params::UiAction::Undo)
        .expect("undo works");
    assert_eq!(undone.effect, params::UiEffect::ChainChanged);
    let redone = backend
        .dispatch(params::UiAction::Redo)
        .expect("redo works");
    assert_eq!(redone.effect, params::UiEffect::ChainChanged);
}

#[test]
fn an_unknown_preset_is_an_unknown_target_with_or_without_a_store() {
    let directory =
        std::env::temp_dir().join(format!("ronie-processor-presets-{}", std::process::id()));
    let store = std::sync::Arc::new(ronie_core::PresetStore::new(
        directory.join("presets"),
        directory.join("system"),
    ));
    let processors = [
        RonieProcessor::new(spec()),
        RonieProcessor::new(spec()).with_preset_store(store),
    ];
    for mut processor in processors {
        let missing = "user:no-such-preset".to_owned();
        for action in [
            params::UiAction::MovePreset {
                id: missing.clone(),
                delta: 1,
            },
            params::UiAction::RenamePreset {
                id: missing.clone(),
                name: "Other".to_owned(),
            },
            params::UiAction::DeletePreset { id: missing },
        ] {
            let shown = format!("{action:?}");
            assert!(
                matches!(
                    processor.dispatch(action),
                    Err(params::UiError::UnknownTarget(_))
                ),
                "{shown}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}

type ReadLevel = fn(&ronie_dsp::rt_chain::ChainControls) -> f32;
type ReadSwitch = fn(&ronie_dsp::rt_chain::ChainControls) -> bool;

fn audio_state(backend: &ProcessorBackend) -> ronie_plugin::AudioSessionState {
    backend.audio_state().expect("audio state")
}

fn set(backend: &ProcessorBackend, id: &'static str, value: f32) {
    backend
        .set_parameter(
            ParameterId(id),
            ParameterValue(value),
            params::GesturePhase::End,
        )
        .expect("set parameter");
}

#[test]
fn every_audio_parameter_reaches_its_chain_control() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    let defaults = audio_state(&backend).controls;
    assert_eq!(defaults.input.input_gain, 1.0);
    assert_eq!(defaults.output.output_level, 0.5);
    assert!(defaults.input.gate_enabled);
    assert_eq!(defaults.input.pitch.tonality_hz, 0.0);

    let cases: &[(&'static str, f32, ReadLevel)] = &[
        ("inputLevel", 0.75, |c| c.input.input_gain),
        ("gateThreshold", -40.0, |c| c.input.gate.threshold_db),
        ("gateRelease", 100.0, |c| c.input.gate.release_ms),
        ("gateHold", 50.0, |c| c.input.gate.hold_ms),
        ("gateRange", 40.0, |c| c.input.gate.range_db),
        ("pitchSemitones", 7.0, |c| c.input.pitch.semitones),
        ("pitchTonality", 6_000.0, |c| c.input.pitch.tonality_hz),
        ("toneBass", 8.0, |c| c.output.tone.bass),
        ("toneMid", 2.0, |c| c.output.tone.mid),
        ("toneTreble", 9.0, |c| c.output.tone.treble),
        ("outputLevel", 0.25, |c| c.output.output_level),
        ("outputBalance", 0.75, |c| c.output.image.balance),
        ("chainPanLeft", 0.25, |c| c.output.image.pan_left),
        ("chainPanRight", 0.75, |c| c.output.image.pan_right),
        ("targetLoudness", -30.0, |c| {
            c.routing.block.target_loudness_db
        }),
        ("inputCalibrationLevel", 3.0, |c| {
            c.routing.block.input_calibration_dbu
        }),
        ("spreadOffset", 1.0, |c| c.output.image.spread.offset_ms),
        ("alignOffset", 1.0, |c| c.output.image.align.offset_ms),
    ];
    for (id, value, read) in cases {
        let before = read(&audio_state(&backend).controls);
        set(&backend, id, *value);
        assert_ne!(read(&audio_state(&backend).controls), before, "{id}");
    }
    let switches: &[(&'static str, ReadSwitch)] = &[
        ("gateEnabled", |c| c.input.gate_enabled),
        ("pitchEnabled", |c| c.input.pitch_enabled),
        ("pitchStep", |c| c.input.pitch_step),
        ("toneEqEnabled", |c| c.output.tone.enabled),
        ("spreadEnabled", |c| c.output.image.spread_enabled),
        ("alignEnabled", |c| c.output.image.align_enabled),
        ("chainSoloLeft", |c| c.output.image.solo_left),
        ("chainSoloRight", |c| c.output.image.solo_right),
        ("chainInvertLeft", |c| c.output.image.invert_left),
        ("chainInvertRight", |c| c.output.image.invert_right),
        ("calibrateInput", |c| c.routing.block.calibrate_input),
    ];
    for (id, read) in switches {
        let before = read(&audio_state(&backend).controls);
        set(&backend, id, if before { 0.0 } else { 1.0 });
        assert_ne!(read(&audio_state(&backend).controls), before, "{id}");
    }
}

#[test]
fn a_mono_rig_moves_chain_mixing_to_the_routing_stage_only() {
    let mut mono = spec();
    mono.stereo_output = false;
    let backend = ProcessorBackend::new(RonieProcessor::new(mono));
    set(&backend, "chainInvertLeft", 1.0);
    set(&backend, "chainSoloRight", 1.0);
    set(&backend, "outputBalance", 1.0);
    let controls = audio_state(&backend).controls;
    assert!(controls.routing.invert_left);
    assert!(controls.routing.solo_right);
    assert!(controls.routing.balance_left < 1.0);
    assert!(!controls.output.image.invert_left);
    assert!(!controls.output.image.solo_right);
    assert_eq!(controls.output.image.balance, 0.5);
}

#[test]
fn a_block_setting_is_not_a_structure_change_but_a_tone_is() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    let first = audio_state(&backend);
    let tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 42,
        "title": "Remote tone",
        "format": "ir",
        "models": [{"id": 7, "name": "Remote"}]
    }))
    .expect("tone");
    let target = backend.chain_state(0).chain.left[0].id.clone();
    backend
        .dispatch(params::UiAction::LoadTone {
            target_insert_id: target.clone(),
            tone,
            model_id: ronie_core::ModelId(7),
        })
        .expect("load tone");
    let loaded = audio_state(&backend);
    assert_ne!(loaded.fingerprint, first.fingerprint);

    let block = backend.chain_state(0).chain.left[0].clone();
    let mut changed = block.params.clone();
    changed.mix = 0.25;
    backend
        .dispatch(params::UiAction::SetBlockParams {
            block_id: target.clone(),
            params: changed,
        })
        .expect("block settings");
    let edited = audio_state(&backend);
    assert_eq!(edited.fingerprint, loaded.fingerprint);
    assert_ne!(edited.block_params, loaded.block_params);

    let chain = backend
        .prepare_session_chain()
        .expect("a tone without bytes must not stop the chain");
    assert_eq!(chain.unavailable.len(), 1);
    assert_eq!(chain.unavailable[0].0, target);
}

fn catalog_ir_tone() -> ronie_core::Tone {
    ronie_core::Tone::from_value(serde_json::json!({
        "id": 42,
        "title": "Remote tone",
        "format": "ir",
        "future": "kept",
        "models": [
            {"id": 7, "name": "Remote", "model_url": "https://models.example/7.wav"},
        ]
    }))
    .expect("tone")
}

fn wav_bytes() -> ronie_core::ModelBytes {
    let samples = [16_000_i16, 0, 0, 16_000];
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36_u32 + samples.len() as u32 * 2).to_le_bytes());
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
    bytes.into()
}

fn load_with_bytes(
    backend: &ProcessorBackend,
    tone: ronie_core::Tone,
    bytes: ronie_core::ModelBytes,
) {
    let target = backend.chain_state(0).chain.left[0].id.clone();
    backend
        .dispatch(params::UiAction::LoadToneWithBytes {
            target_insert_id: target,
            tone,
            model_id: ronie_core::ModelId(7),
            bytes,
        })
        .expect("load with bytes");
}

#[test]
fn downloaded_bytes_reach_a_prepared_chain_from_the_byte_cache() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    let revision = backend.chain_state(0).revision;

    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());

    assert_eq!(backend.chain_state(0).revision, revision + 1);
    assert!(backend.chain_state(0).can_undo);
    let chain = backend.prepare_session_chain().expect("chain");
    assert!(chain.unavailable.is_empty(), "{:?}", chain.unavailable);
}

#[test]
fn a_block_copy_receives_downloaded_bytes() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());
    let block = backend.chain_state(0).chain.left[0].id.clone();

    backend
        .dispatch(params::UiAction::DuplicateBlock(block))
        .expect("duplicate");

    let chain = backend.prepare_session_chain().expect("chain");
    assert!(chain.unavailable.is_empty(), "{:?}", chain.unavailable);
}

#[test]
fn a_refused_byte_load_changes_nothing() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    let revision = backend.chain_state(0).revision;
    let insert = backend.chain_state(0).chain.left[0].id.clone();
    let mut local = catalog_ir_tone();
    local.local = true;
    let cases = [
        (
            "unknown target",
            ronie_core::BlockId("missing".to_owned()),
            catalog_ir_tone(),
            ronie_core::ModelId(7),
            wav_bytes(),
        ),
        (
            "model is not in the tone",
            insert.clone(),
            catalog_ir_tone(),
            ronie_core::ModelId(8),
            wav_bytes(),
        ),
        (
            "local tone",
            insert.clone(),
            local,
            ronie_core::ModelId(7),
            wav_bytes(),
        ),
        (
            "no bytes",
            insert,
            catalog_ir_tone(),
            ronie_core::ModelId(7),
            Vec::new().into(),
        ),
    ];

    for (name, target_insert_id, tone, model_id, bytes) in cases {
        assert!(
            backend
                .dispatch(params::UiAction::LoadToneWithBytes {
                    target_insert_id,
                    tone,
                    model_id,
                    bytes,
                })
                .is_err(),
            "{name}"
        );
        assert_eq!(backend.chain_state(0).revision, revision, "{name}");
    }
    assert!(!backend.chain_state(0).can_undo);
}

#[test]
fn model_validation_compiles_the_bytes_without_a_change() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    let revision = backend.chain_state(0).revision;
    let tone = catalog_ir_tone();

    backend
        .validate_model(&tone, ronie_core::ModelId(7), &wav_bytes())
        .expect("a real impulse response");
    assert!(
        backend
            .validate_model(
                &tone,
                ronie_core::ModelId(7),
                &b"not a model".to_vec().into()
            )
            .is_err()
    );
    assert!(
        backend
            .validate_model(&tone, ronie_core::ModelId(8), &wav_bytes())
            .is_err(),
        "the model is not in the tone"
    );

    assert_eq!(backend.chain_state(0).revision, revision);
}

fn restored_backend(root: &std::path::Path) -> ProcessorBackend {
    ProcessorBackend::with_settings(
        RonieProcessor::new(spec()),
        ronie_core::SettingsStore::standalone(root, ronie_core::PREFERENCES_SETTINGS),
    )
    .expect("settings backend")
}

fn stored_bytes(root: &std::path::Path) -> Vec<(i64, Vec<u8>)> {
    let bytes = std::fs::read(root.join("session.state")).expect("saved session");
    ronie_core::StateCodec::new()
        .decode_session(&bytes)
        .expect("session")
        .model_cache
        .iter()
        .map(|entry| (entry.model_id.0, entry.bytes.to_vec()))
        .collect()
}

/// Replaces the embedded bytes of the first cached model in a saved session.
fn replace_stored_bytes(root: &std::path::Path, bytes: &[u8]) {
    let path = root.join("session.state");
    let mut document = ronie_core::StateCodec::new()
        .decode_session(&std::fs::read(&path).expect("saved session"))
        .expect("session")
        .raw;
    let data = document
        .children
        .iter_mut()
        .filter(|child| child.identifier == "ChainSnapshot")
        .flat_map(|snapshot| &mut snapshot.children)
        .flat_map(|lane| &mut lane.children)
        .flat_map(|block| &mut block.children)
        .filter(|child| child.identifier == "ModelCache")
        .flat_map(|cache| &mut cache.children)
        .flat_map(|cached| &mut cached.properties)
        .find(|(name, _)| name == "data")
        .expect("embedded model");
    data.1 = ronie_core::JuceValue::Binary(bytes.to_vec());
    let mut encoded = b"T3KB".to_vec();
    encoded.extend_from_slice(
        &ronie_core::JuceCodec::default()
            .write_tree(&document)
            .expect("session bytes"),
    );
    std::fs::write(path, encoded).expect("write session");
}

#[test]
fn a_catalog_model_that_cannot_compile_restores_as_bypass_with_its_bytes() {
    let directory = tempfile::tempdir().expect("root");
    let root = directory.path();
    let backend = restored_backend(root);
    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());
    let saved = backend.chain_state(0).chain.left[0].clone();
    drop(backend);
    replace_stored_bytes(root, b"not a model");

    let restarted = restored_backend(root);
    let block = restarted.chain_state(0).chain.left[0].clone();
    assert_eq!(block.tone, saved.tone);
    assert_eq!(block.active_model_id, saved.active_model_id);
    assert!(
        restarted.take_restore().expect("restore").is_none(),
        "a prepared restore would clear the status of the bypassed block"
    );
    let chain = restarted.prepare_session_chain().expect("chain");
    assert_eq!(chain.unavailable.len(), 1);
    assert_eq!(chain.unavailable[0].0, saved.id);
    drop(restarted);
    assert_eq!(stored_bytes(root), [(7, b"not a model".to_vec())]);
}

#[test]
fn a_new_download_repairs_a_catalog_model_that_could_not_compile() {
    let directory = tempfile::tempdir().expect("root");
    let root = directory.path();
    let backend = restored_backend(root);
    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());
    drop(backend);
    replace_stored_bytes(root, b"not a model");

    let restarted = restored_backend(root);
    let block = restarted.chain_state(0).chain.left[0].id.clone();
    restarted
        .dispatch(params::UiAction::SelectModelWithBytes {
            block_id: block,
            tone: catalog_ir_tone(),
            model_id: ronie_core::ModelId(7),
            replaces: Some(ronie_core::ModelId(7)),
            bytes: wav_bytes(),
        })
        .expect("new download");
    let chain = restarted.prepare_session_chain().expect("chain");
    assert!(chain.unavailable.is_empty(), "{:?}", chain.unavailable);
    drop(restarted);
    assert_eq!(stored_bytes(root), [(7, wav_bytes().to_vec())]);
}

#[test]
fn downloaded_bytes_survive_a_restart_and_a_new_download_replaces_them() {
    let directory = tempfile::tempdir().expect("root");
    let root = directory.path();
    let backend = restored_backend(root);
    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());
    drop(backend);
    assert_eq!(stored_bytes(root), [(7, wav_bytes().to_vec())]);

    let restarted = restored_backend(root);
    let block = restarted.chain_state(0).chain.left[0].clone();
    assert_eq!(block.active_model_id, Some(ronie_core::ModelId(7)));
    assert_eq!(block.tone.as_ref().expect("tone").raw["future"], "kept");
    let chain = restarted.prepare_session_chain().expect("chain");
    assert!(chain.unavailable.is_empty(), "{:?}", chain.unavailable);

    let mut newer = wav_bytes().to_vec();
    let last = newer.len() - 1;
    newer[last] ^= 1;
    load_with_bytes(&restarted, catalog_ir_tone(), newer.clone().into());
    drop(restarted);
    assert_eq!(stored_bytes(root), [(7, newer)]);
}

#[test]
fn a_failed_load_stores_no_bytes_for_the_next_launch() {
    let directory = tempfile::tempdir().expect("root");
    let root = directory.path();
    let backend = restored_backend(root);
    assert!(
        backend
            .dispatch(params::UiAction::LoadToneWithBytes {
                target_insert_id: ronie_core::BlockId("missing".to_owned()),
                tone: catalog_ir_tone(),
                model_id: ronie_core::ModelId(7),
                bytes: wav_bytes(),
            })
            .is_err()
    );
    drop(backend);

    assert!(stored_bytes(root).is_empty());
}

fn catalog_ir_tone_on(model: i64) -> ronie_core::Tone {
    ronie_core::Tone::from_value(serde_json::json!({
        "id": 42,
        "title": "Remote tone",
        "format": "ir",
        "future": "kept",
        "models": [
            {"id": model, "name": "Remote", "model_url": "https://models.example/other.wav"},
        ]
    }))
    .expect("tone")
}

fn other_wav_bytes() -> ronie_core::ModelBytes {
    let mut bytes = wav_bytes().to_vec();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    bytes.into()
}

fn switch_to_eight(
    backend: &ProcessorBackend,
) -> Result<ronie_ui::backend::UiReceipt, params::UiError> {
    backend.dispatch(params::UiAction::SelectModelWithBytes {
        block_id: backend.chain_state(0).chain.left[0].id.clone(),
        tone: catalog_ir_tone_on(8),
        model_id: ronie_core::ModelId(8),
        replaces: Some(ronie_core::ModelId(7)),
        bytes: other_wav_bytes(),
    })
}

#[test]
fn a_switch_with_bytes_reaches_a_prepared_chain_and_keeps_the_block_settings() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());
    let mut block = backend.chain_state(0).chain.left[0].clone();
    block.params.mix = 0.25;
    backend
        .dispatch(params::UiAction::SetBlockParams {
            block_id: block.id.clone(),
            params: block.params.clone(),
        })
        .expect("edit");
    let revision = backend.chain_state(0).revision;

    switch_to_eight(&backend).expect("switch");

    let state = backend.chain_state(0);
    assert_eq!(state.revision, revision + 1);
    let switched = &state.chain.left[0];
    assert_eq!(switched.active_model_id, Some(ronie_core::ModelId(8)));
    assert_eq!(switched.params.mix, 0.25);
    assert_eq!(switched.id, block.id);
    let tone = switched.tone.as_ref().expect("tone");
    assert_eq!(tone.models.len(), 1);
    assert_eq!(tone.models[0].id, ronie_core::ModelId(8));
    let chain = backend.prepare_session_chain().expect("chain");
    assert!(chain.unavailable.is_empty(), "{:?}", chain.unavailable);
}

#[test]
fn an_undo_of_a_switch_returns_to_the_old_model_with_its_bytes() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());
    switch_to_eight(&backend).expect("switch");

    backend.dispatch(params::UiAction::Undo).expect("undo");

    let block = backend.chain_state(0).chain.left[0].clone();
    assert_eq!(block.active_model_id, Some(ronie_core::ModelId(7)));
    assert_eq!(
        block.tone.as_ref().expect("tone").models[0].id,
        ronie_core::ModelId(7)
    );
    let chain = backend.prepare_session_chain().expect("chain");
    assert!(chain.unavailable.is_empty(), "{:?}", chain.unavailable);
}

#[test]
fn a_switch_that_is_out_of_date_or_not_a_catalog_model_changes_nothing() {
    let backend = ProcessorBackend::new(RonieProcessor::new(spec()));
    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());
    let block = backend.chain_state(0).chain.left[0].id.clone();
    let insert = backend.chain_state(0).chain.left[1].id.clone();
    let revision = backend.chain_state(0).revision;
    let eight = ronie_core::ModelId(8);
    let seven = Some(ronie_core::ModelId(7));
    let mut other_tone = catalog_ir_tone_on(8);
    other_tone.id = ronie_core::ToneId(43);
    let mut local = catalog_ir_tone_on(8);
    local.local = true;
    let cases = [
        (
            "no block",
            ronie_core::BlockId("missing".to_owned()),
            catalog_ir_tone_on(8),
            eight,
            seven,
            other_wav_bytes(),
        ),
        (
            "an open insert",
            insert,
            catalog_ir_tone_on(8),
            eight,
            seven,
            other_wav_bytes(),
        ),
        (
            "another tone",
            block.clone(),
            other_tone,
            eight,
            seven,
            other_wav_bytes(),
        ),
        (
            "another active model",
            block.clone(),
            catalog_ir_tone_on(8),
            eight,
            Some(ronie_core::ModelId(6)),
            other_wav_bytes(),
        ),
        (
            "a local tone",
            block.clone(),
            local,
            eight,
            seven,
            other_wav_bytes(),
        ),
        (
            "not in the tone",
            block.clone(),
            catalog_ir_tone_on(8),
            ronie_core::ModelId(9),
            seven,
            other_wav_bytes(),
        ),
        (
            "no bytes",
            block,
            catalog_ir_tone_on(8),
            eight,
            seven,
            Vec::new().into(),
        ),
    ];

    for (name, block_id, tone, model_id, replaces, bytes) in cases {
        assert!(
            backend
                .dispatch(params::UiAction::SelectModelWithBytes {
                    block_id,
                    tone,
                    model_id,
                    replaces,
                    bytes,
                })
                .is_err(),
            "{name}"
        );
        assert_eq!(backend.chain_state(0).revision, revision, "{name}");
    }
    assert_eq!(backend.chain_state(0).chain.left[0].active_model_id, seven);
}

#[test]
fn a_switched_model_survives_a_restart_and_only_its_bytes_are_stored() {
    let directory = tempfile::tempdir().expect("root");
    let root = directory.path();
    let backend = restored_backend(root);
    load_with_bytes(&backend, catalog_ir_tone(), wav_bytes());
    switch_to_eight(&backend).expect("switch");
    drop(backend);

    assert_eq!(stored_bytes(root), [(8, other_wav_bytes().to_vec())]);

    let restarted = restored_backend(root);
    let block = restarted.chain_state(0).chain.left[0].clone();
    assert_eq!(block.active_model_id, Some(ronie_core::ModelId(8)));
    let tone = block.tone.as_ref().expect("tone");
    assert_eq!(tone.models.len(), 1);
    assert_eq!(tone.raw["future"], "kept");
    let chain = restarted.prepare_session_chain().expect("chain");
    assert!(chain.unavailable.is_empty(), "{:?}", chain.unavailable);
}
