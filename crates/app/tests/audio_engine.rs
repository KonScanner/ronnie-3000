//! Acceptance tests for the audio engine through the dummy adapter.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use ronie_app::{
    audio::{AudioBackendKind, DummyBackend, DummyLimits},
    audio_settings::StandaloneAudioSettings,
    standalone_backend::{StandaloneBackend, standalone_processor},
};
use ronie_core::{
    BlockId, ChainBranch, ChainSide, LocalStash, PREFERENCES_SETTINGS,
    params::{ParameterId, ParameterValue},
};
use ronie_ui::{
    backend::{AudioDeviceConfig, ChainFault, DeviceAction, GesturePhase, UiAction, UiBackend},
    default::{AudioDeviceState, BannerViewAction, ChromeController, WindowStep},
    shell::{initial_banner_visible, sync_chain_faults, sync_device_banner},
};

const PLUS_12_DB: f32 = 3.981;

struct Rig {
    _directory: tempfile::TempDir,
    root: PathBuf,
    device: DummyBackend,
    backend: StandaloneBackend,
    offset: usize,
}

impl Rig {
    fn new() -> Self {
        Self::with_device(DummyBackend::connected())
    }

    fn with_device(device: DummyBackend) -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path().to_path_buf();
        let backend = start(&root, device.clone());
        Self {
            _directory: directory,
            root,
            device,
            backend,
            offset: 0,
        }
    }

    fn knob(&self, id: &'static str, value: f32) {
        self.backend
            .set_parameter(ParameterId(id), ParameterValue(value), GesturePhase::End)
            .expect("set parameter");
    }

    fn device_action(&self, action: DeviceAction) {
        self.backend
            .dispatch(UiAction::Device(action))
            .expect("device action");
        self.backend.settle_audio().expect("settle");
    }

    fn frames(&self) -> usize {
        self.backend
            .device_settings()
            .expect("device state")
            .banner
            .buffer_size as usize
    }

    /// Renders until gain and block fades settle, then returns the two channel levels.
    fn settled_levels(&mut self) -> (f32, f32) {
        let frames = self.frames();
        let mut output = Vec::new();
        let settle = 48_000 / frames / 4 + 1;
        for _ in 0..settle {
            output = self.render(frames);
        }
        let left: Vec<f32> = output.iter().step_by(2).copied().collect();
        let right: Vec<f32> = output.iter().skip(1).step_by(2).copied().collect();
        (rms(&left), rms(&right))
    }

    fn level(&mut self) -> f32 {
        self.settled_levels().0
    }

    fn render(&mut self, frames: usize) -> Vec<f32> {
        let input: Vec<f32> = (0..frames)
            .map(|frame| sine_sample(self.offset + frame))
            .collect();
        self.offset += frames;
        self.backend.render_dummy(&input).expect("render")
    }

    fn write_ir(&self) -> PathBuf {
        let path = self.root.join("cab.wav");
        std::fs::write(&path, wav()).expect("IR fixture");
        path
    }

    fn insert_slot(&self) -> BlockId {
        self.backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .find(|block| block.is_insert)
            .expect("insert slot")
            .id
            .clone()
    }

    fn drop_ir(&self) -> BlockId {
        let target = self.insert_slot();
        self.backend
            .dispatch(UiAction::DropFile {
                target_insert_id: target.clone(),
                path: self.write_ir(),
            })
            .expect("drop model file");
        target
    }

    fn generation(&self) -> u64 {
        self.backend
            .audio_status()
            .expect("status")
            .active_generation
    }
}

fn start(root: &Path, device: DummyBackend) -> StandaloneBackend {
    StandaloneBackend::with_dummy_device(
        standalone_processor(root).expect("processor"),
        &root.join(PREFERENCES_SETTINGS),
        device,
    )
    .expect("dummy backend")
}

fn sine_sample(frame: usize) -> f32 {
    0.25 * (frame as f32 * 1_000.0 / 48_000.0 * std::f32::consts::TAU).sin()
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn assert_close(actual: f32, expected: f32, context: &str) {
    assert!(
        (actual / expected - 1.0).abs() < 0.05,
        "{context}: {actual} against {expected}"
    );
}

fn assert_differs(actual: f32, other: f32, context: &str) {
    assert!(
        (actual / other - 1.0).abs() > 0.2,
        "{context}: {actual} against {other}"
    );
}

/// A two-tap comb, which an amplitude measurement at 1 kHz can tell from the dry signal.
fn wav() -> Vec<u8> {
    let samples = [16_000_i16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16_000];
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
    bytes
}

#[test]
fn moving_a_knob_changes_rendered_audio() {
    let mut rig = Rig::new();
    let unity = rig.level();
    assert!(unity > 0.05, "the default session must pass audio: {unity}");

    rig.knob("outputLevel", 0.75);
    assert_close(rig.level(), unity * PLUS_12_DB, "output level");

    rig.knob("outputLevel", 0.5);
    rig.knob("inputLevel", 0.25);
    assert_close(rig.level(), unity / PLUS_12_DB, "input level");

    rig.knob("inputLevel", 0.5);
    rig.knob("toneEqEnabled", 0.0);
    rig.knob("toneMid", 10.0);
    assert_close(rig.level(), unity, "a disabled tone stack");
    rig.knob("toneEqEnabled", 1.0);
    assert_differs(rig.level(), unity, "the tone stack");
}

#[test]
fn the_noise_gate_controls_reach_the_chain() {
    let mut rig = Rig::new();
    let open = rig.level();
    rig.knob("gateThreshold", 0.0);
    rig.knob("gateRelease", 5.0);
    rig.knob("gateHold", 0.0);
    let closed = rig.level();
    assert!(
        closed < open * 0.1,
        "gate must close: {closed} against {open}"
    );
}

#[test]
fn every_chain_edit_reaches_the_running_chain() {
    let mut rig = Rig::new();
    let dry = rig.level();

    let start_generation = rig.generation();
    let block = rig.drop_ir();
    let wet = rig.level();
    assert_differs(wet, dry, "a dropped model");
    assert!(rig.generation() > start_generation);

    // A block setting keeps the running engines, so the generation must not move.
    let generation = rig.generation();
    let mut params = rig
        .backend
        .chain_state(0)
        .chain
        .block(&block)
        .expect("block")
        .params
        .clone();
    params.mix = 0.0;
    rig.backend
        .dispatch(UiAction::SetBlockParams {
            block_id: block.clone(),
            params,
        })
        .expect("block settings");
    assert_close(rig.level(), dry, "a dry mix");
    assert_eq!(rig.generation(), generation);

    rig.backend.dispatch(UiAction::Undo).expect("undo mix");
    assert_close(rig.level(), wet, "an undone mix");
    assert_eq!(rig.generation(), generation);

    rig.backend.dispatch(UiAction::Undo).expect("undo load");
    assert_close(rig.level(), dry, "an undone load");
    assert!(rig.generation() > generation);

    rig.backend.dispatch(UiAction::Redo).expect("redo load");
    assert_close(rig.level(), wet, "a redone load");

    rig.backend
        .dispatch(UiAction::RemoveBlock(block))
        .expect("remove block");
    assert_close(rig.level(), dry, "a removed block");

    let stash = LocalStash::new(rig.root.join("stash")).expect("stash");
    let loaded =
        ronie_core::load_local_path(&rig.write_ir(), &stash, &ronie_dsp::validate_a2_document)
            .expect("local tone");
    let model_id = loaded.tone.models[0].id;
    let target = rig.insert_slot();
    rig.backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: target,
            tone: loaded.tone,
            model_id,
        })
        .expect("load tone");
    assert_close(rig.level(), wet, "a loaded tone");
}

#[test]
fn lane_structure_edits_reach_the_running_chain() {
    let mut rig = Rig::new();
    let dry = rig.level();
    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("stereo chains");
    rig.drop_ir();
    let (left, right) = rig.settled_levels();
    assert_differs(left, dry, "the left lane has the model");
    assert_close(right, dry, "the right lane stays dry");

    rig.backend.dispatch(UiAction::SwapChains).expect("swap");
    let (swapped_left, swapped_right) = rig.settled_levels();
    assert_close(swapped_left, right, "the swapped left lane");
    assert_close(swapped_right, left, "the swapped right lane");
}

#[test]
fn a_branch_feeds_the_other_lane_from_the_tapped_block() {
    let mut rig = Rig::new();
    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("stereo chains");
    let block = rig.drop_ir();
    let (left, right) = rig.settled_levels();
    assert_differs(right, left, "independent lanes");

    rig.backend
        .dispatch(UiAction::SetBranch(Some(ChainBranch {
            side: ChainSide::Left,
            after_block_id: block,
        })))
        .expect("branch");
    let (trunk, branch) = rig.settled_levels();
    assert_close(trunk, left, "the trunk keeps the model");
    assert_close(branch, trunk, "the branch starts after the tapped model");
}

#[test]
fn one_chain_after_a_lane_move_still_reaches_audio_and_survives_a_reopen() {
    let mut rig = Rig::new();
    let dry = rig.level();
    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("stereo chains");
    let block = rig.drop_ir();
    rig.backend
        .dispatch(UiAction::MoveBlockToLane {
            block_id: block,
            target_side: ChainSide::Right,
            target_index: 0,
        })
        .expect("move block");
    let (left, right) = rig.settled_levels();
    assert_close(left, dry, "the left lane is dry after the move");
    assert_differs(right, left, "the right lane has the model");

    // The left lane grows again here, and its new slot must not repeat an identity.
    rig.backend
        .dispatch(UiAction::SetStereoMode(false))
        .expect("one chain");
    let status = rig.backend.audio_status().expect("status");
    assert_eq!(status.chain_error, None);
    let (left, right) = rig.settled_levels();
    assert_close(left, dry, "one dry chain on the left");
    assert_close(right, dry, "one dry chain on the right");

    rig.device_action(DeviceAction::Restart);
    assert!(
        rig.backend
            .device_settings()
            .expect("device state")
            .banner
            .device_open
    );
}

#[test]
fn a_copy_of_a_block_with_only_embedded_bytes_keeps_its_model() {
    let mut rig = Rig::new();
    let block = rig.drop_ir();
    let wet = rig.level();
    rig.backend
        .dispatch(UiAction::SavePreset {
            name: "Embedded".to_owned(),
        })
        .expect("save preset");
    let id = rig
        .backend
        .chain_state(0)
        .session
        .active_preset
        .clone()
        .expect("active preset")
        .id;
    // Only the bytes that the preset embeds remain, as on a machine that received this preset.
    let stash = rig.root.join("stash");
    std::fs::remove_dir_all(&stash).expect("remove stash");
    std::fs::create_dir_all(&stash).expect("empty stash");
    rig.backend
        .dispatch(UiAction::LoadPreset { id })
        .expect("load preset");
    assert_close(rig.level(), wet, "the embedded model");

    for copy in [
        UiAction::DuplicateBlock(block.clone()),
        UiAction::DuplicateBlockToLane {
            block_id: block.clone(),
            target_side: ChainSide::Left,
            target_index: 0,
        },
    ] {
        rig.backend.dispatch(copy).expect("copy block");
        let status = rig.backend.audio_status().expect("status");
        assert_eq!(status.unavailable_blocks, Vec::<BlockId>::new());
        rig.backend.dispatch(UiAction::Undo).expect("undo copy");
    }

    rig.backend
        .dispatch(UiAction::CopyBlock(block))
        .expect("copy block");
    rig.backend
        .dispatch(UiAction::PasteBlock {
            side: ChainSide::Left,
            index: 0,
        })
        .expect("paste block");
    let status = rig.backend.audio_status().expect("status");
    assert_eq!(status.unavailable_blocks, Vec::<BlockId>::new());
    assert_differs(rig.level(), wet, "two models in series");
}

#[test]
fn a_session_with_a_tone_without_bytes_starts_again_and_runs_it_as_bypass() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let target = {
        let backend = start(root, DummyBackend::connected());
        let target = backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .find(|block| block.is_insert)
            .expect("insert slot")
            .id
            .clone();
        let tone = ronie_core::Tone::from_value(serde_json::json!({
            "id": 42,
            "title": "Catalog tone",
            "format": "ir",
            "models": [{"id": 7, "name": "Not downloaded"}]
        }))
        .expect("tone");
        backend
            .dispatch(UiAction::LoadTone {
                target_insert_id: target.clone(),
                tone,
                model_id: ronie_core::ModelId(7),
            })
            .expect("load tone");
        target
    };

    let backend = start(root, DummyBackend::connected());
    assert!(
        backend.chain_state(0).chain.block(&target).is_some(),
        "the tone stays in the session"
    );
    let status = backend.audio_status().expect("status");
    assert_eq!(status.unavailable_blocks, vec![target]);
    let input: Vec<f32> = (0..256).map(sine_sample).collect();
    assert_eq!(backend.render_dummy(&input).expect("render").len(), 512);
}

#[test]
fn a_reopen_keeps_the_chain_and_the_controls() {
    let mut rig = Rig::new();
    rig.knob("outputLevel", 0.75);
    rig.drop_ir();
    let before = rig.level();

    rig.device_action(DeviceAction::Restart);
    assert_close(rig.level(), before, "after a restart");

    let mut config = rig
        .backend
        .device_settings()
        .expect("device state")
        .config();
    config.buffer_frames = Some(256);
    rig.device_action(DeviceAction::Configure(config));
    assert_eq!(rig.frames(), 256);
    assert_close(rig.level(), before, "after a buffer change");

    rig.knob("outputLevel", 0.5);
    assert_close(rig.level(), before / PLUS_12_DB, "a knob after the reopen");
}

#[test]
fn a_flood_of_changes_never_loses_the_last_one_and_the_control_thread_drains_retirements() {
    let mut rig = Rig::new();
    let dry = rig.level();
    rig.backend
        .dispatch(UiAction::SavePreset {
            name: "Dry".to_owned(),
        })
        .expect("save dry preset");
    let dry_id = rig
        .backend
        .chain_state(0)
        .session
        .active_preset
        .clone()
        .expect("dry preset")
        .id;
    rig.drop_ir();
    let wet = rig.level();
    rig.backend
        .dispatch(UiAction::SavePreset {
            name: "Wet".to_owned(),
        })
        .expect("save wet preset");
    let wet_id = rig
        .backend
        .chain_state(0)
        .session
        .active_preset
        .clone()
        .expect("wet preset")
        .id;
    assert_differs(wet, dry, "the two presets");

    // No callback runs here, so that the bounded queue must fill.
    for _ in 0..16 {
        for id in [&wet_id, &dry_id, &wet_id] {
            rig.backend
                .dispatch(UiAction::LoadPreset { id: id.clone() })
                .expect("load preset");
        }
    }
    let waiting = rig.backend.audio_status().expect("status");
    assert!(waiting.publish_pending, "the last change must wait");

    assert_close(rig.level(), wet, "the last preset");
    let settled = rig.backend.audio_status().expect("status");
    assert!(!settled.publish_pending);
    assert_eq!(settled.active_generation, settled.published_generation);
    assert!(settled.retired_chains >= 8, "{}", settled.retired_chains);

    // One more change retires the chain it replaces as soon as the next callback swaps it out.
    rig.backend
        .dispatch(UiAction::LoadPreset { id: dry_id })
        .expect("load preset");
    let _ = rig.render(256);
    let after = rig.backend.audio_status().expect("status").retired_chains;
    assert!(
        after > settled.retired_chains,
        "the replaced chain must retire"
    );

    for value in 0..64 {
        rig.knob("outputLevel", if value == 63 { 0.75 } else { 0.0 });
    }
    assert_close(rig.level(), dry * PLUS_12_DB, "the last knob value");
}

#[test]
fn the_prepared_spec_comes_from_the_final_stream_configuration() {
    let mut rig = Rig::with_device(DummyBackend::with_limits(DummyLimits {
        sample_rates: vec![44_100],
        default_sample_rate: 44_100,
        buffer_frames: 64..=512,
        ..DummyLimits::default()
    }));
    let preparation = rig.backend.chain_state(0).preparation;
    assert_eq!(preparation.host_sample_rate, 44_100.0);
    assert_eq!(preparation.max_host_frames, 256);

    rig.device_action(DeviceAction::Configure(AudioDeviceConfig {
        sample_rate: None,
        buffer_frames: Some(4_096),
        input: None,
        output: None,
        midi_inputs: Vec::new(),
    }));
    let device = rig.backend.device_settings().expect("device state");
    assert_eq!(
        device.banner.buffer_size, 512,
        "the device clamps the buffer"
    );
    assert_eq!(device.banner.sample_rate, 44_100.0);
    assert!(device.banner.device_open);
    let preparation = rig.backend.chain_state(0).preparation;
    assert_eq!(preparation.host_sample_rate, 44_100.0);
    assert_eq!(preparation.max_host_frames, 512);

    let rendered = rig.render(512);
    assert_eq!(rendered.len(), 1_024);
    assert!(rig.level() > 0.05, "a clamped stream must carry audio");
    rig.drop_ir();
    assert!(rig.level() > 0.01, "a later edit must fit the real stream");

    let saved = StandaloneAudioSettings::load(rig.root.join(PREFERENCES_SETTINGS))
        .expect("reload settings");
    assert_eq!(
        saved.config().buffer_frames,
        Some(4_096),
        "the saved request stays the user's choice"
    );
    assert_eq!(saved.config().backend, AudioBackendKind::Dummy);
}

#[test]
fn a_device_without_256_clamps_the_moved_buffer_and_the_file_keeps_the_request() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    ronie_core::SettingsStore::new(root.join(PREFERENCES_SETTINGS))
        .merge_key(
            "audioSetup",
            "<AUDIOSETUP deviceType=\"Dummy\" inputDeviceName=\"\" outputDeviceName=\"\" sampleRate=\"0\" bufferSize=\"1024\" inputChannels=\"1\" outputChannels=\"2\"></AUDIOSETUP>",
        )
        .expect("seed audio setup");

    let backend = start(
        root,
        DummyBackend::with_limits(DummyLimits {
            sample_rates: vec![48_000],
            default_sample_rate: 48_000,
            buffer_frames: 512..=2_048,
            ..DummyLimits::default()
        }),
    );

    let device = backend.device_settings().expect("device state");
    assert!(device.banner.device_open);
    assert_eq!(device.banner.buffer_size, 512, "the device clamps the move");
    assert_eq!(backend.chain_state(0).preparation.max_host_frames, 512);
    drop(backend);
    let saved =
        StandaloneAudioSettings::load(root.join(PREFERENCES_SETTINGS)).expect("reload settings");
    assert_eq!(saved.config().buffer_frames, Some(256));
}

#[test]
fn a_saved_buffer_larger_than_the_old_restore_spec_renders_on_the_next_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    {
        let backend = start(root, DummyBackend::connected());
        backend
            .dispatch(UiAction::Device(DeviceAction::Configure(
                AudioDeviceConfig {
                    sample_rate: Some(96_000.0),
                    buffer_frames: Some(256),
                    input: None,
                    output: None,
                    midi_inputs: Vec::new(),
                },
            )))
            .expect("configure");
        backend.settle_audio().expect("settle");
    }
    let backend = start(root, DummyBackend::connected());
    let preparation = backend.chain_state(0).preparation;
    assert_eq!(preparation.host_sample_rate, 96_000.0);
    assert_eq!(preparation.max_host_frames, 256);
    let input: Vec<f32> = (0..256).map(sine_sample).collect();
    assert_eq!(backend.render_dummy(&input).expect("render").len(), 512);
}

#[test]
fn a_rejected_stream_request_closes_the_stream_visibly() {
    let rig = Rig::with_device(DummyBackend::with_limits(DummyLimits {
        sample_rates: vec![48_000],
        ..DummyLimits::default()
    }));
    rig.device_action(DeviceAction::Configure(AudioDeviceConfig {
        sample_rate: Some(96_000.0),
        buffer_frames: Some(128),
        input: None,
        output: None,
        midi_inputs: Vec::new(),
    }));
    let device = rig.backend.device_settings().expect("device state");
    assert!(!device.banner.device_open);
    assert!(device.error.is_some());
}

#[test]
fn a_lost_device_is_visible_and_a_later_open_restores_the_session() {
    let mut rig = Rig::new();
    rig.knob("outputLevel", 0.75);
    rig.drop_ir();
    let before = rig.level();
    let healthy = rig.backend.device_settings().expect("device state");
    assert!(healthy.banner.device_open);
    assert!(!initial_banner_visible(&healthy.banner));

    rig.device.set_connected(false);
    let lost = rig.backend.device_settings().expect("device state");
    assert!(
        !lost.banner.device_open,
        "device_open must follow the fault"
    );
    assert!(lost.error.is_some());
    assert!(
        initial_banner_visible(&lost.banner),
        "the existing banner rule must fire for a lost default device"
    );
    assert!(rig.backend.render_dummy(&[0.0; 256]).is_err());

    // Audio edits while the device is away must still reach the next stream.
    rig.knob("outputLevel", 0.5);

    // No automatic reopen: a settings choice retries, here with the values that the UI sends.
    rig.device_action(DeviceAction::Configure(lost.config()));
    let still_lost = rig.backend.device_settings().expect("device state");
    assert!(still_lost.error.is_some(), "the device is still away");
    rig.device.set_connected(true);
    let lost = rig.backend.device_settings().expect("device state");
    assert!(!lost.banner.device_open, "there is no automatic reopen");
    rig.device_action(DeviceAction::Configure(lost.config()));
    let reopened = rig.backend.device_settings().expect("device state");
    assert!(reopened.banner.device_open);
    assert!(reopened.error.is_none());
    assert!(!initial_banner_visible(&reopened.banner));
    assert_close(rig.level(), before / PLUS_12_DB, "the reopened session");

    let saved = StandaloneAudioSettings::load(rig.root.join(PREFERENCES_SETTINGS))
        .expect("reload settings");
    assert_eq!(
        saved.config().output_device,
        None,
        "the default stays default"
    );
    assert_eq!(
        saved.config().input_device,
        None,
        "the default stays default"
    );
}

#[test]
fn a_retry_after_a_lost_device_asks_for_the_values_that_the_device_took() {
    let mut rig = Rig::with_device(DummyBackend::with_limits(DummyLimits {
        sample_rates: vec![44_100],
        default_sample_rate: 44_100,
        ..DummyLimits::default()
    }));
    rig.device.set_connected(false);
    let lost = rig.backend.device_settings().expect("device state");
    assert!(!lost.banner.device_open);
    assert_eq!(lost.banner.sample_rate, 44_100.0);

    rig.device.set_connected(true);
    rig.device_action(DeviceAction::Configure(lost.config()));
    assert!(
        rig.backend
            .device_settings()
            .expect("device state")
            .banner
            .device_open
    );
    assert!(rig.level() > 0.05);
}

#[test]
fn a_start_failure_during_a_reopen_is_visible_and_recoverable() {
    let mut rig = Rig::new();
    let before = rig.level();
    rig.device.set_start_fails(true);
    rig.device_action(DeviceAction::Restart);
    let failed = rig.backend.device_settings().expect("device state");
    assert!(!failed.banner.device_open);
    assert!(failed.error.is_some());
    assert!(initial_banner_visible(&failed.banner));

    rig.device.set_start_fails(false);
    rig.device_action(DeviceAction::Restart);
    assert!(
        rig.backend
            .device_settings()
            .expect("device")
            .banner
            .device_open
    );
    assert_close(rig.level(), before, "after recovery");
}

#[test]
fn a_non_finite_sample_does_not_stay_on_the_output() {
    let mut rig = Rig::new();
    rig.drop_ir();
    let before = rig.level();
    let mut input: Vec<f32> = (0..256).map(sine_sample).collect();
    input[17] = f32::NAN;
    input[40] = f32::INFINITY;
    let output = rig.backend.render_dummy(&input).expect("render");
    assert!(output.iter().all(|sample| sample.is_finite()));
    let after = rig.level();
    assert_close(after, before, "audio after a non-finite input");
}

#[test]
fn a_model_without_bytes_stays_listed_and_every_other_edit_still_reaches_audio() {
    let mut rig = Rig::new();
    let unity = rig.level();
    let target = rig.insert_slot();
    let tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 42,
        "title": "Catalog tone",
        "format": "ir",
        "models": [{"id": 7, "name": "Not downloaded"}]
    }))
    .expect("tone");
    rig.backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: target.clone(),
            tone,
            model_id: ronie_core::ModelId(7),
        })
        .expect("load tone");
    let status = rig.backend.audio_status().expect("status");
    assert_eq!(status.unavailable_blocks, vec![target]);
    assert!(status.chain_error.is_none());
    assert_close(rig.level(), unity, "a bypassed block");
    rig.knob("outputLevel", 0.75);
    assert_close(rig.level(), unity * PLUS_12_DB, "a knob beside it");
}

/// A real backend on a dummy device, with the real controller that the shell keeps in sync.
struct BannerRig {
    _directory: tempfile::TempDir,
    backend: Arc<StandaloneBackend>,
    chrome: ChromeController,
    shown_device: Option<AudioDeviceState>,
    shown_faults: Vec<ChainFault>,
}

impl BannerRig {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let backend = Arc::new(start(directory.path(), DummyBackend::connected()));
        let chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
        let mut rig = Self {
            _directory: directory,
            backend,
            chrome,
            shown_device: None,
            shown_faults: Vec::new(),
        };
        rig.sync();
        rig
    }

    /// One pass of the shell logic.
    fn sync(&mut self) {
        sync_device_banner(
            self.backend.as_ref(),
            &mut self.chrome,
            &mut self.shown_device,
            1_000,
        );
        sync_chain_faults(
            self.backend.as_ref(),
            &mut self.chrome,
            &mut self.shown_faults,
            1_000,
        );
    }

    fn banner(&mut self) -> Option<String> {
        self.sync();
        self.chrome
            .banners()
            .active()
            .map(|banner| format!("{}{}", banner.headline, banner.body))
    }

    fn slots(&self) -> Vec<BlockId> {
        self.backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .filter(|block| block.is_insert)
            .map(|block| block.id.clone())
            .collect()
    }

    fn load_bytes(&self, target: &BlockId, tone: ronie_core::Tone, bytes: &[u8]) {
        self.backend
            .dispatch(UiAction::LoadToneWithBytes {
                target_insert_id: target.clone(),
                model_id: tone.models[0].id,
                tone,
                bytes: bytes.to_vec().into(),
            })
            .expect("load tone");
        self.backend.settle_audio().expect("settle");
    }

    /// A tone that no compile can accept, as a corrupt download gives.
    fn load_bad(&self, target: &BlockId, title: &str) {
        self.load_bytes(target, catalog_tone(title, "nam"), BAD_MODEL_BYTES);
    }

    fn load_good(&self, target: &BlockId, title: &str) {
        self.load_bytes(target, catalog_tone(title, "ir"), &wav());
    }

    fn load_without_bytes(&self, target: &BlockId, title: &str) {
        let tone = catalog_tone(title, "ir");
        self.backend
            .dispatch(UiAction::LoadTone {
                target_insert_id: target.clone(),
                model_id: tone.models[0].id,
                tone,
            })
            .expect("load tone");
    }

    /// Makes the model of a block output non-finite samples in the next callback.
    fn poison(&self, target: &BlockId) {
        let block = self.backend.chain_state(0).chain.block(target).cloned();
        let mut params = block.expect("block").params;
        params.mix = f64::NAN;
        self.backend
            .dispatch(UiAction::SetBlockParams {
                block_id: target.clone(),
                params,
            })
            .expect("set block params");
    }

    fn render(&self) -> Vec<f32> {
        let frames = self
            .backend
            .device_settings()
            .expect("device")
            .banner
            .buffer_size as usize;
        let input: Vec<f32> = (0..frames).map(sine_sample).collect();
        self.backend.render_dummy(&input).expect("render")
    }

    fn dismiss_banner(&mut self) {
        let id = self.chrome.banners().active().expect("banner").id.clone();
        self.chrome
            .handle_banner_view_action(BannerViewAction::Dismiss(id), 1_000)
            .expect("dismiss");
    }
}

const BAD_MODEL_BYTES: &[u8] = b"this is not a model";

fn catalog_tone(title: &str, format: &str) -> ronie_core::Tone {
    // A title with new bytes must also be a new tone, as in the catalog.
    let id = title.bytes().map(i64::from).sum::<i64>();
    ronie_core::Tone::from_value(serde_json::json!({
        "id": id,
        "title": title,
        "format": format,
        "models": [{
            "id": id + 1,
            "name": "Model",
            "model_url": "https://models.example/model",
        }],
    }))
    .expect("tone")
}

#[test]
fn a_healthy_session_has_no_fault_and_no_banner() {
    let mut rig = BannerRig::new();
    assert_eq!(rig.banner(), None);
    assert!(rig.backend.chain_faults().is_empty());
    assert!(!rig.chrome.window_chrome().banner);

    let slot = rig.slots()[0].clone();
    rig.load_good(&slot, "Green Muff");
    let output = rig.render();

    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_eq!(rig.banner(), None);
    assert!(rig.backend.chain_faults().is_empty());
}

#[test]
fn a_live_tone_with_no_bytes_shows_a_banner_that_names_it() {
    let mut rig = BannerRig::new();
    let slot = rig.slots()[0].clone();

    rig.load_without_bytes(&slot, "Green Muff");

    assert_eq!(
        rig.banner().as_deref(),
        Some("Green Muff has no model data. Ronnie bypasses this block. Load the tone again.")
    );
    assert!(rig.chrome.window_chrome().banner);
}

#[test]
fn a_live_model_that_cannot_compile_shows_fixed_text_with_no_parse_error() {
    let mut rig = BannerRig::new();
    let slot = rig.slots()[0].clone();

    rig.load_bad(&slot, "Green Muff");

    assert_eq!(
        rig.banner().as_deref(),
        Some("Green Muff cannot run. Ronnie bypasses this block.")
    );
    assert_eq!(
        rig.backend
            .audio_status()
            .expect("status")
            .unavailable_blocks,
        [slot]
    );
}

#[test]
fn a_new_model_clears_the_fault_and_gives_back_the_window_height() {
    let mut rig = BannerRig::new();
    let slot = rig.slots()[0].clone();
    rig.load_bad(&slot, "Green Muff");
    assert!(rig.banner().is_some());
    let _ = rig.chrome.take_window_steps(1.0);

    rig.load_good(&slot, "Red Fuzz");

    assert_eq!(rig.banner(), None);
    assert!(rig.backend.chain_faults().is_empty());
    assert_eq!(
        rig.chrome.take_window_steps(1.0),
        vec![WindowStep::HideBanner, WindowStep::Resize(1024, 614)]
    );
}

#[test]
fn removing_the_block_clears_its_fault() {
    let mut rig = BannerRig::new();
    let slot = rig.slots()[0].clone();
    rig.load_bad(&slot, "Green Muff");
    assert!(rig.banner().is_some());

    rig.backend
        .dispatch(UiAction::RemoveBlock(slot))
        .expect("remove block");

    assert_eq!(rig.banner(), None);
}

#[test]
fn a_dismissed_fault_stays_hidden_after_a_new_chain_and_a_new_fault_shows() {
    let mut rig = BannerRig::new();
    let slots = rig.slots();
    rig.load_bad(&slots[0], "Green Muff");
    assert!(rig.banner().is_some());

    rig.dismiss_banner();
    assert_eq!(rig.banner(), None);

    // A later edit prepares a new chain with the same fault.
    rig.backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("structure edit");
    assert_eq!(rig.banner(), None);

    rig.load_bad(&slots[1], "Red Fuzz");
    assert_eq!(
        rig.banner().as_deref(),
        Some("Red Fuzz cannot run. Ronnie bypasses this block."),
    );
}

#[test]
fn another_bad_tone_in_the_same_slot_is_a_new_fault() {
    let mut rig = BannerRig::new();
    let slot = rig.slots()[0].clone();
    rig.load_bad(&slot, "Green Muff");
    assert!(rig.banner().is_some());
    rig.dismiss_banner();
    assert_eq!(rig.banner(), None);

    rig.load_bad(&slot, "Red Fuzz");

    assert_eq!(
        rig.banner().as_deref(),
        Some("Red Fuzz cannot run. Ronnie bypasses this block."),
    );
}

#[test]
fn a_model_that_outputs_nan_shows_a_banner_that_names_its_block() {
    let mut rig = BannerRig::new();
    let slots = rig.slots();
    rig.load_good(&slots[0], "Clean Boost");
    rig.load_good(&slots[1], "Green Muff");
    assert!(rig.render().iter().all(|sample| sample.is_finite()));
    assert_eq!(rig.banner(), None);

    rig.poison(&slots[1]);
    let output = rig.render();

    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        rig.banner().as_deref(),
        Some("Green Muff made invalid audio. Ronnie replaces it with silence."),
    );

    // The repair prepares a chain from the same session, and the fault is still there.
    rig.render();
    rig.render();
    assert_eq!(
        rig.banner().as_deref(),
        Some("Green Muff made invalid audio. Ronnie replaces it with silence."),
    );
}

#[test]
fn a_new_model_clears_a_nan_fault_and_no_late_event_brings_it_back() {
    let mut rig = BannerRig::new();
    let slot = rig.slots()[0].clone();
    rig.load_good(&slot, "Green Muff");
    rig.poison(&slot);
    rig.render();
    assert!(rig.banner().is_some());
    let _ = rig.chrome.take_window_steps(1.0);

    rig.load_good(&slot, "Red Fuzz");

    assert_eq!(rig.banner(), None);
    for _ in 0..4 {
        assert!(rig.render().iter().all(|sample| sample.is_finite()));
        assert_eq!(rig.banner(), None);
    }
    assert_eq!(
        rig.chrome.take_window_steps(1.0),
        vec![WindowStep::HideBanner, WindowStep::Resize(1024, 614)],
        "one hide and one resize, and no flicker"
    );
}

#[test]
fn a_dismissed_nan_fault_stays_hidden_through_its_repairs() {
    let mut rig = BannerRig::new();
    let slot = rig.slots()[0].clone();
    rig.load_good(&slot, "Green Muff");
    rig.poison(&slot);
    rig.render();
    assert!(rig.banner().is_some());

    rig.dismiss_banner();

    for _ in 0..4 {
        rig.render();
        assert_eq!(rig.banner(), None);
    }
}

#[test]
fn a_lost_stream_reports_no_chain_fault_and_the_reopen_reports_it_again() {
    let device = DummyBackend::connected();
    let directory = tempfile::tempdir().expect("tempdir");
    let backend = start(directory.path(), device.clone());
    let target = backend.chain_state(0).chain.left[0].id.clone();
    let tone = catalog_tone("Green Muff", "ir");
    backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: target,
            model_id: tone.models[0].id,
            tone,
        })
        .expect("load tone");
    assert_eq!(backend.chain_faults().len(), 1);

    device.set_connected(false);
    let _ = backend.device_settings();
    assert!(backend.chain_faults().is_empty(), "the device banner shows");

    device.set_connected(true);
    backend
        .dispatch(UiAction::Device(DeviceAction::Restart))
        .expect("restart");
    backend.settle_audio().expect("settle");
    assert_eq!(backend.chain_faults().len(), 1);
}

#[test]
fn a_user_with_no_saved_choice_gets_a_buffer_of_256() {
    let rig = Rig::new();
    assert_eq!(rig.frames(), 256);
    assert_eq!(
        rig.backend.chain_state(0).preparation.max_host_frames,
        256,
        "the chain must be prepared for the real size"
    );
}

#[test]
fn a_saved_buffer_choice_stays_above_the_default() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    {
        let backend = start(root, DummyBackend::connected());
        backend
            .dispatch(UiAction::Device(DeviceAction::Configure(
                AudioDeviceConfig {
                    sample_rate: None,
                    buffer_frames: Some(1_024),
                    input: None,
                    output: None,
                    midi_inputs: Vec::new(),
                },
            )))
            .expect("configure");
        backend.settle_audio().expect("settle");
    }
    let backend = start(root, DummyBackend::connected());
    assert_eq!(
        backend
            .device_settings()
            .expect("device")
            .banner
            .buffer_size,
        1_024
    );
}
