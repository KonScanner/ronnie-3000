//! End-to-end standalone UI, audio-manager, and settings-store integration.

use ronie_app::{audio_settings::StandaloneAudioSettings, standalone_backend::StandaloneBackend};
use ronie_core::InputMode;
use ronie_dsp::PrepareSpec;
use ronie_plugin::{ProcessorBackend, RonieProcessor};
use ronie_ui::backend::{AudioDeviceConfig, DeviceAction, UiAction, UiBackend, UiError};

#[test]
fn dummy_device_and_midi_changes_reopen_and_persist_through_the_real_adapter() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("standalone.settings");
    let processor = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 128,
        oversampling: 1,
        input_mode: InputMode::Left,
        stereo_input: false,
        stereo_output: true,
    }));
    let backend = StandaloneBackend::dummy(processor, &path).expect("dummy backend");
    backend
        .dispatch(UiAction::Device(DeviceAction::Configure(
            AudioDeviceConfig {
                sample_rate: Some(96_000.0),
                buffer_frames: Some(256),
                input: Some("Dummy guitar and monitors".to_owned()),
                output: Some("Dummy guitar and monitors".to_owned()),
                midi_inputs: Vec::new(),
            },
        )))
        .expect("configure");
    backend.settle_audio().expect("settle");
    let device = backend.device_settings().expect("device state");
    assert_eq!(device.banner.sample_rate, 96_000.0);
    assert_eq!(device.banner.buffer_size, 256);
    assert!(device.banner.device_open);
    assert_eq!(
        StandaloneAudioSettings::load(&path)
            .expect("reload")
            .config()
            .sample_rate,
        Some(96_000)
    );

    backend
        .dispatch(UiAction::Device(DeviceAction::SetMidiInputEnabled {
            id: "keyboard".to_owned(),
            enabled: true,
        }))
        .expect("enable MIDI");
    assert!(
        backend
            .device_settings()
            .expect("device state")
            .midi_inputs
            .iter()
            .any(|input| input.id == "keyboard" && input.enabled)
    );
    let settings = StandaloneAudioSettings::load(&path).expect("reload MIDI");
    assert!(settings.midi_enabled("keyboard"));

    backend
        .dispatch(UiAction::Device(DeviceAction::SetInputChannels(vec![1])))
        .expect("stereo channels");
    assert_eq!(
        backend
            .device_settings()
            .expect("device state")
            .input_channels
            .iter()
            .filter(|channel| channel.active)
            .map(|channel| channel.index)
            .collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn an_invalid_device_configuration_makes_no_device_request_and_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("standalone.settings");
    let processor = ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: 128,
        oversampling: 1,
        input_mode: InputMode::Left,
        stereo_input: false,
        stereo_output: true,
    }));
    let backend = StandaloneBackend::dummy(processor, &path).expect("dummy backend");
    backend.settle_audio().expect("settle");
    let device = backend.device_settings().expect("device state");
    let audio = backend.audio_status().expect("status");
    let view = backend.chain_state(0);
    let saved = std::fs::read(&path).ok();

    for (sample_rate, buffer_frames) in [
        (Some(f64::NAN), Some(256)),
        (Some(f64::INFINITY), None),
        (Some(0.0), None),
        (Some(-48_000.0), Some(256)),
        (Some(96_000.0), Some(0)),
        (None, Some(0)),
    ] {
        let refused = backend.dispatch(UiAction::Device(DeviceAction::Configure(
            AudioDeviceConfig {
                sample_rate,
                buffer_frames,
                input: None,
                output: None,
                midi_inputs: Vec::new(),
            },
        )));
        assert!(
            matches!(refused, Err(UiError::InvalidAction(_))),
            "{sample_rate:?} {buffer_frames:?}: {refused:?}"
        );
        // A request would show the open at once, and the open would end in a new stream.
        assert_eq!(
            backend.device_settings().expect("device state"),
            device,
            "{sample_rate:?} {buffer_frames:?}"
        );
        backend.settle_audio().expect("settle");
        assert_eq!(backend.device_settings().expect("device state"), device);
        assert_eq!(backend.audio_status().expect("status"), audio, "no reopen");
        let after = backend.chain_state(0);
        assert_eq!(after.revision, view.revision);
        assert_eq!(after.preparation, view.preparation);
        assert_eq!(std::fs::read(&path).ok(), saved, "no settings write");
    }
}

#[test]
fn preset_restores_stay_accepted_and_pending_when_the_command_queue_fills() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let processor =
        ronie_app::standalone_backend::standalone_processor(root).expect("standalone processor");
    let backend = StandaloneBackend::dummy(processor, &root.join(ronie_core::PREFERENCES_SETTINGS))
        .expect("dummy backend");
    backend
        .dispatch(UiAction::SavePreset {
            name: "Queued".to_owned(),
        })
        .expect("save preset");
    let id = backend
        .chain_state(0)
        .session
        .active_preset
        .clone()
        .expect("active preset")
        .id;

    for _ in 0..12 {
        backend
            .dispatch(UiAction::LoadPreset { id: id.clone() })
            .expect("a full queue must leave the restore pending, not fail the applied preset");
    }
    let output = backend
        .render_dummy(&vec![0.0; 256])
        .expect("render one queued restore");
    assert_eq!(output.len(), 512);
    backend
        .dispatch(UiAction::LoadPreset { id })
        .expect("publication continues after queue capacity is released");
}
