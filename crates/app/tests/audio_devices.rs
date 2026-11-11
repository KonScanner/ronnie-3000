//! Device selection, channel layout, recovery, and real-time callback tests.

use std::time::Duration;

use ronie_app::{
    audio::{
        AudioBackendKind, AudioDeviceManager, AudioError, AudioWorker, DeviceConfig, DummyBackend,
        choose_sample_rate, close_timeout,
    },
    midi::{MidiEvent, MidiRouter},
};

fn config() -> DeviceConfig {
    DeviceConfig {
        backend: AudioBackendKind::Dummy,
        input_device: Some("guitar".to_owned()),
        output_device: Some("monitors".to_owned()),
        sample_rate: Some(48_000),
        buffer_frames: Some(128),
        input_channels: 1,
        output_channels: 2,
        ..DeviceConfig::default()
    }
}

#[test]
fn backend_enumeration_covers_the_drivers_of_this_system_and_dummy() {
    let manager = AudioDeviceManager::new(DummyBackend::connected());
    let backends = manager.backends();
    assert!(backends.contains(&AudioBackendKind::System));
    assert!(backends.contains(&AudioBackendKind::Dummy));
    let linux = cfg!(target_os = "linux");
    assert_eq!(backends.contains(&AudioBackendKind::Alsa), linux);
    assert_eq!(backends.contains(&AudioBackendKind::Jack), linux);
    assert!(!manager.enumerate().expect("dummy enumeration").is_empty());
}

#[test]
fn default_rate_prefers_lowest_at_least_44100_or_nearest_48000() {
    assert_eq!(
        choose_sample_rate(&[96_000, 48_000, 44_100], None),
        Some(44_100)
    );
    assert_eq!(
        choose_sample_rate(&[96_000, 32_000, 192_000], None),
        Some(48_000)
    );
    assert_eq!(choose_sample_rate(&[8_000, 16_000], None), Some(16_000));
    assert_eq!(
        choose_sample_rate(&[44_100, 96_000], Some(96_000)),
        Some(96_000)
    );
}

#[test]
fn invalid_config_and_disconnected_devices_fail_without_opening() {
    let mut backend = DummyBackend::connected();
    let mut invalid = config();
    invalid.buffer_frames = Some(0);
    assert!(matches!(
        backend.open(&invalid, AudioWorker::new(&invalid).expect("worker")),
        Err(AudioError::InvalidConfig(_))
    ));

    let mut disconnected = DummyBackend::disconnected();
    assert!(matches!(
        disconnected.open(&config(), AudioWorker::new(&config()).expect("worker")),
        Err(AudioError::Disconnected)
    ));
}

#[test]
fn close_timeout_scales_with_io_period() {
    assert_eq!(close_timeout(48_000, 128), Duration::from_millis(400));
    assert_eq!(close_timeout(8_000, 4096), Duration::from_millis(912));
    assert_eq!(close_timeout(1_000, 10_000), Duration::from_millis(2400));
}

#[test]
fn null_and_short_channel_arrays_are_tolerated() {
    let mut worker = AudioWorker::new(&config()).expect("worker");
    let input = [0.25_f32; 64];
    let inputs = [Some(input.as_slice()), None];
    let mut left = [0.0_f32; 32];
    let mut right = [0.0_f32; 64];
    {
        let mut outputs = [Some(left.as_mut_slice()), Some(right.as_mut_slice())];
        worker
            .process_channels(&inputs, &mut outputs)
            .expect("short output is safe");
    }
    assert!(left.iter().all(|sample| sample.abs() <= f32::EPSILON));
    assert!(right.iter().all(|sample| sample.abs() <= f32::EPSILON));
}

#[test]
fn mono_input_fans_out_and_output_folds() {
    let mut worker = AudioWorker::new(&config()).expect("worker");
    let input = [0.25_f32; 128];
    let mut stereo = [0.0_f32; 256];
    worker
        .process_interleaved(&input, 1, &mut stereo, 2)
        .expect("mono to stereo");
    for frame in stereo.as_chunks::<2>().0 {
        assert_eq!(frame[0], frame[1]);
    }

    let mut mono_config = config();
    mono_config.output_channels = 1;
    let mut worker = AudioWorker::new(&mono_config).expect("mono worker");
    let stereo_input = [0.25_f32; 256];
    let mut mono = [0.0_f32; 128];
    worker
        .process_interleaved(&stereo_input, 2, &mut mono, 1)
        .expect("stereo to mono");
    assert!(mono.iter().all(|sample| sample.is_finite()));
}

#[test]
fn reconnection_restores_the_previous_configuration() {
    let mut manager = AudioDeviceManager::new(DummyBackend::connected());
    let stream = manager.open(&config()).expect("initial open");
    stream.close().expect("initial close");
    manager.set_connected(false);
    assert!(manager.open(&config()).is_err());
    manager.set_connected(true);
    let stream = manager.reconnect().expect("reconnect");
    assert_eq!(stream.config(), &config());
    stream.close().expect("reconnected close");
}

#[test]
fn enabled_midi_inputs_merge_while_disabled_inputs_stay_silent() {
    let mut router = MidiRouter::new();
    router.set_enabled("keyboard", true);
    router.set_enabled("pads", true);
    router.set_enabled("disabled", false);
    router.push(MidiEvent::new("keyboard", [0x90, 60, 100]));
    router.push(MidiEvent::new("disabled", [0x90, 61, 100]));
    router.push(MidiEvent::new("pads", [0x90, 62, 100]));
    let events = router.drain();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data, [0x90, 60, 100]);
    assert_eq!(events[1].data, [0x90, 62, 100]);
}

#[test]
fn audio_open_sentinel_falls_back_to_the_default_device() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");
    let settings =
        ronie_app::audio_settings::StandaloneAudioSettings::load(path).expect("settings");
    settings.mark_open_attempt().expect("sentinel write");
    let mut manager = AudioDeviceManager::new(DummyBackend::connected());
    let mut stale = config();
    stale.input_device = Some("crashed-device".to_owned());
    let stream = manager
        .open_with_recovery(&stale, &settings)
        .expect("fallback open");
    assert!(manager.used_fallback());
    assert_eq!(stream.config().input_device, None);
    assert!(!settings.audio_open_sentinel());
    stream.close().expect("fallback close");
}

#[test]
fn worker_publishes_atomic_capabilities_meters_and_banner_state() {
    let mut worker = AudioWorker::new(&config()).expect("worker");
    let telemetry = worker.telemetry();
    assert_eq!(telemetry.sample_rate(), 48_000);
    assert_eq!(telemetry.input_channels(), 1);
    assert_eq!(telemetry.output_channels(), 2);
    assert_eq!(telemetry.last_event(), None);

    let input = [0.25_f32; 128];
    let mut output = [0.0_f32; 256];
    worker
        .process_interleaved(&input, 1, &mut output, 2)
        .expect("metered callback");
    assert_eq!(telemetry.input_peak(), 0.25);
    assert!(telemetry.output_peak().is_finite());
    assert_eq!(telemetry.last_event(), None);
}

#[test]
fn a_host_buffer_above_the_prepared_size_still_renders_every_frame() {
    let mut worker = AudioWorker::new(&config()).expect("worker");
    let input: Vec<f32> = (0..1_000)
        .map(|frame| 0.25 * (frame as f32 * 0.13).sin())
        .collect();
    let mut output = vec![0.0_f32; 2_000];
    worker
        .process_interleaved(&input, 1, &mut output, 2)
        .expect("oversized callback");
    assert!(
        output[1_800..].iter().any(|sample| sample.abs() > 0.05),
        "the end of an oversized callback must carry audio"
    );
}

#[test]
fn worker_reports_a_non_finite_sample_in_its_input_peak_and_never_on_its_output() {
    let mut worker = AudioWorker::new(&config()).expect("worker");
    let telemetry = worker.telemetry();
    let mut input = [0.25_f32; 128];
    input[9] = f32::NAN;
    let mut output = [0.0_f32; 256];
    worker
        .process_interleaved(&input, 1, &mut output, 2)
        .expect("callback");
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_eq!(telemetry.input_peak(), f32::INFINITY);
    assert!(telemetry.output_peak().is_finite());
}

#[test]
fn the_tuner_tap_copies_samples_only_while_a_reader_renews_it() {
    let mut worker = AudioWorker::new(&config()).expect("worker");
    let telemetry = worker.telemetry();
    let input = [0.25_f32; 128];
    let mut output = [0.0_f32; 256];
    worker
        .process_interleaved(&input, 1, &mut output, 2)
        .expect("callback");
    assert_eq!(telemetry.tuner_samples_written(), 0, "no page, no work");

    telemetry.open_tuner_tap();
    let mut callbacks = 0;
    while telemetry.tuner_tap_open() {
        worker
            .process_interleaved(&input, 1, &mut output, 2)
            .expect("callback");
        callbacks += 1;
        assert!(callbacks < 1_000, "the tap must close by itself");
    }
    let written = telemetry.tuner_samples_written();
    assert_eq!(written, callbacks * 128);

    worker
        .process_interleaved(&input, 1, &mut output, 2)
        .expect("callback");
    assert_eq!(
        telemetry.tuner_samples_written(),
        written,
        "a closed page costs no copy"
    );
}

#[test]
fn the_tuner_tap_mixes_two_channels_and_refuses_a_non_finite_sample() {
    let mut worker = AudioWorker::new(&DeviceConfig {
        input_channels: 2,
        ..config()
    })
    .expect("worker");
    let telemetry = worker.telemetry();
    telemetry.open_tuner_tap();
    let mut input = [0.0_f32; 256];
    for frame in input.as_chunks_mut::<2>().0 {
        frame[0] = 0.5;
    }
    input[2] = f32::NAN;
    let mut output = [0.0_f32; 256];
    worker
        .process_interleaved(&input, 2, &mut output, 2)
        .expect("callback");

    let mut samples = Vec::new();
    assert_eq!(telemetry.read_tuner_samples(0, &mut samples), 128);
    assert_eq!(samples.len(), 128);
    assert_eq!(samples[0], 0.25, "half of a guitar in one channel");
    assert_eq!(samples[1], 0.0, "a non-finite sample reads as silence");
    assert_eq!(samples[2], 0.25);
}
