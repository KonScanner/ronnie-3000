//! The audio route: device identity, channel selection, the test tone, and a frame that never
//! waits for device work. Each test here uses fake devices with fake names.

use std::{
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use ronie_app::{
    audio::{
        AudioBackend, AudioBackendFactory, AudioBackendKind, AudioError, AudioStream, AudioWorker,
        DeviceCapabilities, DeviceConfig, DeviceDescriptor, DummyBackend, DummyLimits,
        NegotiatedStream, SYSTEM_DEFAULT_DEVICE, TEST_TONE_HERTZ, TEST_TONE_MILLISECONDS,
        TEST_TONE_PEAK,
    },
    audio_settings::StandaloneAudioSettings,
    standalone_backend::StandaloneBackend,
};
use ronie_core::{InputMode, PREFERENCES_SETTINGS};
use ronie_dsp::PrepareSpec;
use ronie_plugin::{ProcessorBackend, RonieProcessor};
use ronie_ui::backend::{AudioDeviceConfig, DeviceAction, HostControl, UiAction, UiBackend};

const RATE: usize = 48_000;
const FRAMES: usize = 256;

fn processor() -> ProcessorBackend {
    ProcessorBackend::new(RonieProcessor::new(PrepareSpec {
        host_sample_rate: 48_000.0,
        max_host_frames: FRAMES,
        oversampling: 1,
        input_mode: InputMode::Left,
        stereo_input: false,
        stereo_output: true,
    }))
}

fn device_row(id: &str, label: &str, input: bool, output: bool) -> DeviceDescriptor {
    DeviceDescriptor {
        id: id.to_owned(),
        label: label.to_owned(),
        input,
        output,
        default_input: false,
        default_output: false,
        listed: true,
    }
}

/// A desk with a headset as the system default and an interface with an instrument input.
fn desk() -> Vec<DeviceDescriptor> {
    let mut headset_in = device_row(
        "alsa_input.usb-Fake_Headset_SERIAL42-00",
        "Fake Headset",
        true,
        false,
    );
    headset_in.default_input = true;
    let mut headset_out = device_row(
        "alsa_output.usb-Fake_Headset_SERIAL42-00",
        "Fake Headset",
        false,
        true,
    );
    headset_out.default_output = true;
    vec![
        headset_in,
        headset_out,
        device_row(
            "alsa_output.usb-Fake_Solo_SERIAL99-00.sink",
            "Fake Solo Headphones",
            false,
            true,
        ),
        device_row(
            "alsa_input.usb-Fake_Solo_SERIAL99-00.mic2",
            "Fake Solo Input 2 Mic",
            true,
            false,
        ),
        device_row(
            "alsa_input.usb-Fake_Solo_SERIAL99-00.inst1",
            "Fake Solo Input 1 Inst/Line",
            true,
            false,
        ),
        device_row(
            "alsa_input.usb-Other_SERIAL11-00.mic",
            "Same Name",
            true,
            false,
        ),
        device_row(
            "alsa_input.usb-Other_SERIAL22-00.mic",
            "Same Name",
            true,
            false,
        ),
    ]
}

struct Rig {
    backend: StandaloneBackend,
    path: std::path::PathBuf,
    _directory: tempfile::TempDir,
}

impl Rig {
    fn new(limits: DummyLimits) -> Self {
        Self::in_directory(tempfile::tempdir().expect("tempdir"), limits)
    }

    /// A backend on the settings file in `directory`, as a new start of the app reads it.
    fn in_directory(directory: tempfile::TempDir, limits: DummyLimits) -> Self {
        let path = directory.path().join(PREFERENCES_SETTINGS);
        let backend = StandaloneBackend::with_dummy_device(
            processor(),
            &path,
            DummyBackend::with_limits(limits),
        )
        .expect("backend");
        Self {
            backend,
            path,
            _directory: directory,
        }
    }

    fn act(&self, action: DeviceAction) {
        self.backend
            .dispatch(UiAction::Device(action))
            .expect("device action");
        self.backend.settle_audio().expect("settle");
    }

    fn state(&self) -> ronie_ui::default::SystemDeviceState {
        self.backend.device_settings().expect("device state")
    }

    fn saved(&self) -> DeviceConfig {
        StandaloneAudioSettings::load(&self.path)
            .expect("reload")
            .config()
            .clone()
    }
}

fn four_channels() -> DummyLimits {
    DummyLimits {
        input_channels: 4,
        output_channels: 4,
        ..DummyLimits::default()
    }
}

// ---- Device identity and lists ----

#[test]
fn each_row_has_its_own_identity_and_the_system_default_comes_first() {
    let rig = Rig::new(DummyLimits {
        devices: desk(),
        ..DummyLimits::default()
    });
    let state = rig.state();

    assert_eq!(state.input_devices[0].id, SYSTEM_DEFAULT_DEVICE);
    assert_eq!(state.input_devices[0].label, "System default: Fake Headset");
    assert_eq!(state.output_devices[0].id, SYSTEM_DEFAULT_DEVICE);
    assert_eq!(
        state.output_devices[0].label,
        "System default: Fake Headset"
    );
    let ids: Vec<&str> = state
        .input_devices
        .iter()
        .filter(|row| row.label == "Same Name")
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "alsa_input.usb-Other_SERIAL11-00.mic",
            "alsa_input.usb-Other_SERIAL22-00.mic"
        ],
        "two devices with one name are two rows"
    );
    assert!(
        state
            .input_devices
            .iter()
            .all(|row| { !row.label.contains("SERIAL") })
    );
}

#[test]
fn with_no_choice_the_input_is_the_instrument_input_and_the_output_is_the_system_default() {
    let rig = Rig::new(DummyLimits {
        devices: desk(),
        ..DummyLimits::default()
    });
    let state = rig.state();

    assert_eq!(
        state.selected_input,
        "alsa_input.usb-Fake_Solo_SERIAL99-00.inst1"
    );
    assert_eq!(state.banner.input_device, "Fake Solo Input 1 Inst/Line");
    assert_eq!(state.selected_output, SYSTEM_DEFAULT_DEVICE);
    assert_eq!(state.banner.output_device, "Fake Headset");
    assert_eq!(rig.saved().input_device, None, "the file holds no choice");
}

#[test]
fn a_change_of_rate_keeps_an_automatic_input_automatic() {
    let rig = Rig::new(DummyLimits {
        devices: desk(),
        ..DummyLimits::default()
    });
    let mut config = rig.state().config();
    config.sample_rate = Some(96_000.0);

    rig.act(DeviceAction::Configure(config));

    let saved = rig.saved();
    assert_eq!(saved.sample_rate, Some(96_000));
    assert_eq!(saved.input_device, None, "the user chose no input");
    assert_eq!(saved.output_device, None, "the user chose no output");
}

#[test]
fn one_click_selects_one_row_of_two_with_one_label_and_the_choice_is_saved_with_its_identity() {
    let rig = Rig::new(DummyLimits {
        devices: desk(),
        ..DummyLimits::default()
    });
    let mut config = rig.state().config();
    config.input = Some("alsa_input.usb-Other_SERIAL22-00.mic".to_owned());

    rig.act(DeviceAction::Configure(config));

    let state = rig.state();
    assert_eq!(state.selected_input, "alsa_input.usb-Other_SERIAL22-00.mic");
    assert_eq!(
        state
            .input_devices
            .iter()
            .filter(|row| row.id == state.selected_input)
            .count(),
        1
    );
    let saved = rig.saved();
    assert_eq!(
        saved.input_device.as_deref(),
        Some("alsa_input.usb-Other_SERIAL22-00.mic")
    );
    assert_eq!(saved.input_label.as_deref(), Some("Same Name"));
}

#[test]
fn a_chosen_system_default_stays_a_choice_and_an_interface_does_not_replace_it() {
    let rig = Rig::new(DummyLimits {
        devices: desk(),
        ..DummyLimits::default()
    });
    let mut config = rig.state().config();
    config.input = Some(SYSTEM_DEFAULT_DEVICE.to_owned());

    rig.act(DeviceAction::Configure(config));

    let state = rig.state();
    assert_eq!(state.selected_input, SYSTEM_DEFAULT_DEVICE);
    assert_eq!(state.banner.input_device, "Fake Headset");
    assert_eq!(
        rig.saved().input_device.as_deref(),
        Some(SYSTEM_DEFAULT_DEVICE)
    );
}

#[test]
fn a_saved_device_that_is_gone_falls_back_and_says_so_and_the_choice_stays_in_the_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join(PREFERENCES_SETTINGS);
    let settings = StandaloneAudioSettings::load(&path).expect("settings");
    settings
        .save_config(&DeviceConfig {
            backend: AudioBackendKind::Dummy,
            input_device: Some("alsa_input.usb-Gone_SERIAL77-00".to_owned()),
            input_label: Some("Gone Interface".to_owned()),
            ..DeviceConfig::default()
        })
        .expect("save");

    let backend = StandaloneBackend::with_dummy_device(
        processor(),
        &path,
        DummyBackend::with_limits(DummyLimits {
            devices: desk(),
            ..DummyLimits::default()
        }),
    )
    .expect("backend");

    let state = backend.device_settings().expect("device state");
    assert!(state.banner.device_open);
    assert_eq!(
        state.selected_input,
        "alsa_input.usb-Fake_Solo_SERIAL99-00.inst1"
    );
    let notice = state.notice.expect("a message");
    assert!(notice.contains("Gone Interface"), "{notice}");
    assert!(!notice.contains("SERIAL"), "{notice}");

    backend
        .dispatch(UiAction::Device(DeviceAction::Configure(
            AudioDeviceConfig {
                sample_rate: Some(96_000.0),
                buffer_frames: Some(256),
                input: Some(state.selected_input),
                output: Some(state.selected_output),
                midi_inputs: Vec::new(),
            },
        )))
        .expect("configure");
    backend.settle_audio().expect("settle");
    assert_eq!(
        StandaloneAudioSettings::load(&path)
            .expect("reload")
            .config()
            .input_device
            .as_deref(),
        Some("alsa_input.usb-Gone_SERIAL77-00"),
        "the device can come back"
    );
}

#[test]
fn the_log_holds_labels_and_no_identity() {
    let rig = Rig::new(DummyLimits {
        devices: desk(),
        ..DummyLimits::default()
    });
    rig.act(DeviceAction::Restart);

    let text = rig.backend.diagnostic_log().expect("log");
    assert!(text.contains("stream opened"), "{text}");
    assert!(text.contains("Fake Solo Input 1 Inst/Line"), "{text}");
    assert!(!text.contains("SERIAL"), "{text}");
    assert!(!text.contains("alsa_input"), "{text}");
}

#[test]
fn a_device_that_is_lost_shows_its_label_and_no_identity() {
    let device = DummyBackend::with_limits(DummyLimits {
        devices: desk(),
        ..DummyLimits::default()
    });
    let directory = tempfile::tempdir().expect("tempdir");
    let backend = StandaloneBackend::with_dummy_device(
        processor(),
        &directory.path().join(PREFERENCES_SETTINGS),
        device.clone(),
    )
    .expect("backend");

    device.set_connected(false);
    let state = backend.device_settings().expect("device state");

    let error = state.error.expect("an error");
    assert!(error.contains("Fake Headset"), "{error}");
    assert!(error.contains("not connected"), "{error}");
    let log = backend.diagnostic_log().expect("log");
    assert!(log.contains("stream lost"), "{log}");
    assert!(!log.contains("SERIAL"), "{log}");
}

#[test]
fn the_rates_and_the_buffers_come_from_the_device() {
    let rig = Rig::new(DummyLimits {
        sample_rates: vec![48_000, 96_000],
        buffer_frames: 64..=512,
        ..DummyLimits::default()
    });
    let state = rig.state();

    assert_eq!(state.sample_rates, [48_000.0, 96_000.0]);
    assert_eq!(state.banner.buffer_sizes, [64, 128, 256, 512]);
    assert_eq!(state.banner.buffer_size, 256, "the default buffer");
}

#[test]
fn a_mono_device_has_one_channel_row_and_a_stereo_request_falls_back_to_it() {
    let rig = Rig::new(DummyLimits {
        input_channels: 1,
        ..DummyLimits::default()
    });
    assert_eq!(rig.state().input_channels.len(), 1);

    rig.act(DeviceAction::SetInputChannels(vec![0, 1]));

    let state = rig.state();
    assert!(state.banner.device_open, "{:?}", state.error);
    assert_eq!(state.input_channels.len(), 1);
    assert!(state.input_channels[0].active);
}

// ---- Channel routing ----

/// One callback of a four-channel device with one sample value on each channel.
fn capture(values: [f32; 4]) -> Vec<f32> {
    (0..FRAMES).flat_map(|_| values).collect()
}

fn peak_of_channel(output: &[f32], channels: usize, channel: usize) -> f32 {
    output
        .iter()
        .skip(channel)
        .step_by(channels)
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

#[test]
fn a_guitar_on_the_chosen_channel_reaches_the_chain_with_no_new_stream() {
    let rig = Rig::new(four_channels());
    let values = [0.0, 0.0, 0.5, 0.0];
    let settle = |rig: &Rig| {
        let mut output = Vec::new();
        for _ in 0..60 {
            output = rig
                .backend
                .render_dummy_capture(&capture(values))
                .expect("render");
        }
        output
    };

    let silent = settle(&rig);
    assert_eq!(
        peak_of_channel(&silent, 4, 0),
        0.0,
        "channel 1 has no signal"
    );

    let generation = rig
        .backend
        .audio_status()
        .expect("status")
        .active_generation;
    rig.backend
        .dispatch(UiAction::Device(DeviceAction::SetInputChannels(vec![2])))
        .expect("route");

    let routed = settle(&rig);
    assert!(
        peak_of_channel(&routed, 4, 0) > 0.1,
        "the left channel plays"
    );
    assert!(
        peak_of_channel(&routed, 4, 1) > 0.1,
        "a mono input fills both"
    );
    assert_eq!(
        rig.backend
            .audio_status()
            .expect("status")
            .active_generation,
        generation,
        "the same chain still runs"
    );
    assert!(
        rig.state().input_channels[2].active,
        "the page shows the route"
    );
    rig.backend.settle_audio().expect("settle");
    assert_eq!(rig.saved().input_route[0], 2, "the route is saved");
}

#[test]
fn a_saved_route_applies_to_the_next_stream() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join(PREFERENCES_SETTINGS);
    {
        let backend = StandaloneBackend::with_dummy_device(
            processor(),
            &path,
            DummyBackend::with_limits(four_channels()),
        )
        .expect("backend");
        backend
            .dispatch(UiAction::Device(DeviceAction::SetInputChannels(vec![3])))
            .expect("route");
        backend.settle_audio().expect("settle");
    }
    let backend = StandaloneBackend::with_dummy_device(
        processor(),
        &path,
        DummyBackend::with_limits(four_channels()),
    )
    .expect("backend");
    let mut output = Vec::new();
    for _ in 0..60 {
        output = backend
            .render_dummy_capture(&capture([0.0, 0.0, 0.0, 0.5]))
            .expect("render");
    }
    assert!(peak_of_channel(&output, 4, 0) > 0.1);
    assert!(
        backend.device_settings().expect("state").input_channels[3].active,
        "the page shows the saved route"
    );
}

#[test]
fn stereo_routes_two_channels_in_their_chosen_order() {
    let rig = Rig::new(four_channels());
    rig.act(DeviceAction::SetInputChannels(vec![3, 1]));

    let mut meters = None;
    for _ in 0..4 {
        rig.backend
            .render_dummy_capture(&capture([0.0, 0.5, 0.0, 0.125]))
            .expect("render");
        meters = Some(rig.backend.meter_levels());
    }

    let meters = meters.expect("meters");
    assert!((meters.input_db[0] - 20.0 * 0.125_f32.log10()).abs() < 0.1);
    assert!((meters.input_db[1] - 20.0 * 0.5_f32.log10()).abs() < 0.1);
}

#[test]
fn each_channel_of_the_device_has_its_own_level() {
    let rig = Rig::new(four_channels());
    for _ in 0..4 {
        rig.backend
            .render_dummy_capture(&capture([0.5, 0.0, 0.25, 0.0]))
            .expect("render");
    }

    let levels = rig.backend.input_channel_levels();

    assert_eq!(levels.len(), 4);
    assert!(
        (levels[0] - 20.0 * 0.5_f32.log10()).abs() < 0.1,
        "{levels:?}"
    );
    assert_eq!(levels[1], -60.0, "{levels:?}");
    assert!(
        (levels[2] - 20.0 * 0.25_f32.log10()).abs() < 0.1,
        "{levels:?}"
    );
    assert_eq!(levels[3], -60.0, "{levels:?}");
}

#[test]
fn the_tuner_reads_the_routed_channel() {
    let rig = Rig::new(four_channels());
    rig.backend
        .dispatch(UiAction::Device(DeviceAction::SetInputChannels(vec![2])))
        .expect("route");
    let _ = rig.backend.tuner_reading();
    let mut reading = rig.backend.tuner_reading();
    for block in 0..80 {
        let samples: Vec<f32> = (0..FRAMES)
            .flat_map(|frame| {
                let time = (block * FRAMES + frame) as f32 / RATE as f32;
                let wave = 0.25 * (std::f32::consts::TAU * 440.0 * time).sin();
                [0.0, 0.0, wave, 0.0]
            })
            .collect();
        rig.backend.render_dummy_capture(&samples).expect("render");
        reading = rig.backend.tuner_reading();
    }
    assert!(reading.in_tune(440.0), "read {} Hz", reading.frequency_hz);
}

/// Renders a 440 Hz guitar on input 1 for some callbacks, and returns the last output.
fn render_a_note(rig: &Rig, blocks: usize) -> Vec<f32> {
    let mut output = Vec::new();
    for block in 0..blocks {
        let samples: Vec<f32> = (0..FRAMES)
            .flat_map(|frame| {
                let time = (block * FRAMES + frame) as f32 / RATE as f32;
                let wave = 0.25 * (std::f32::consts::TAU * 440.0 * time).sin();
                [wave, 0.0, 0.0, 0.0]
            })
            .collect();
        output = rig.backend.render_dummy_capture(&samples).expect("render");
        let _ = rig.backend.tuner_reading();
    }
    output
}

#[test]
fn hear_yourself_off_silences_the_output_but_the_input_still_reads() {
    let rig = Rig::new(four_channels());
    rig.act(DeviceAction::SetInputChannels(vec![0]));
    rig.act(DeviceAction::SetHearYourself(false));

    let output = render_a_note(&rig, 80);
    assert!(
        output.iter().all(|sample| *sample == 0.0),
        "the output plays"
    );
    let meters = rig.backend.meter_levels();
    assert!(meters.input_db[0] > -20.0, "input meter {meters:?}");
    assert!(meters.output_db[0] < -50.0, "output meter {meters:?}");
    let reading = rig.backend.tuner_reading();
    assert!(reading.in_tune(440.0), "read {} Hz", reading.frequency_hz);

    // A new stream keeps the choice.
    rig.act(DeviceAction::Restart);
    let output = render_a_note(&rig, 20);
    assert!(
        output.iter().all(|sample| *sample == 0.0),
        "a new stream plays"
    );

    rig.act(DeviceAction::SetHearYourself(true));
    let output = render_a_note(&rig, 20);
    assert!(
        peak_of_channel(&output, 4, 0) > 0.01,
        "the output stays silent"
    );
}

#[test]
fn hear_yourself_off_stays_off_after_a_new_start_from_its_first_callback() {
    let rig = Rig::new(four_channels());
    rig.act(DeviceAction::SetInputChannels(vec![0]));
    assert!(
        peak_of_channel(&render_a_note(&rig, 20), 4, 0) > 0.01,
        "the input does not play"
    );
    rig.act(DeviceAction::SetHearYourself(false));
    let Rig {
        backend,
        _directory: directory,
        ..
    } = rig;
    drop(backend);

    let rig = Rig::in_directory(directory, four_channels());
    assert!(
        !rig.state().banner.hear_yourself,
        "the new start turned Hear Yourself on"
    );
    for callback in 0..20 {
        let output = render_a_note(&rig, 1);
        assert!(
            output.iter().all(|sample| *sample == 0.0),
            "callback {callback} of the new start plays the input"
        );
    }

    // The choice of on also stays.
    rig.act(DeviceAction::SetHearYourself(true));
    let Rig {
        backend,
        _directory: directory,
        ..
    } = rig;
    drop(backend);
    let rig = Rig::in_directory(directory, four_channels());
    assert!(rig.state().banner.hear_yourself);
    assert!(peak_of_channel(&render_a_note(&rig, 20), 4, 0) > 0.01);
}

#[test]
fn hear_yourself_is_on_for_a_settings_file_with_no_choice() {
    let directory = tempfile::tempdir().expect("tempdir");
    // A file of an earlier release has a device setup and no Hear Yourself value.
    StandaloneAudioSettings::load(directory.path().join(PREFERENCES_SETTINGS))
        .expect("settings")
        .save_config(&DeviceConfig {
            backend: AudioBackendKind::Dummy,
            ..DeviceConfig::default()
        })
        .expect("an older setup");

    let rig = Rig::in_directory(directory, four_channels());
    assert!(rig.state().banner.hear_yourself, "Hear Yourself is off");
    assert!(
        peak_of_channel(&render_a_note(&rig, 20), 4, 0) > 0.01,
        "the input does not play"
    );
}

#[test]
fn the_output_pair_selects_where_the_chain_plays() {
    let rig = Rig::new(four_channels());
    assert_eq!(rig.state().output_pairs, ["Output 1-2", "Output 3-4"]);
    rig.act(DeviceAction::SetOutputPair(1));

    let mut output = Vec::new();
    for _ in 0..60 {
        output = rig
            .backend
            .render_dummy_capture(&capture([0.5, 0.0, 0.0, 0.0]))
            .expect("render");
    }

    assert_eq!(peak_of_channel(&output, 4, 0), 0.0);
    assert_eq!(peak_of_channel(&output, 4, 1), 0.0);
    assert!(peak_of_channel(&output, 4, 2) > 0.1);
    assert!(peak_of_channel(&output, 4, 3) > 0.1);
    assert_eq!(rig.state().active_output_pair, 1);
    assert_eq!(rig.saved().output_first, 2);
}

#[test]
fn a_device_with_two_outputs_offers_no_output_pair() {
    let rig = Rig::new(DummyLimits::default());
    assert!(rig.state().output_pairs.is_empty());
}

// ---- Test tone ----

/// Plays the callbacks after a test tone request, and returns the left and the right channel.
fn tone_output(rig: &Rig, callbacks: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for _ in 0..callbacks {
        let output = rig
            .backend
            .render_dummy_capture(&vec![0.0; FRAMES * 2])
            .expect("render");
        left.extend(output.iter().step_by(2));
        right.extend(output.iter().skip(1).step_by(2));
    }
    (left, right)
}

#[test]
fn the_real_backend_offers_the_test_tone_and_still_no_auto_balance() {
    let rig = Rig::new(DummyLimits::default());
    assert!(rig.backend.supports(HostControl::TestTone));
    assert!(!rig.backend.supports(HostControl::AutoBalance));
}

#[test]
fn the_test_tone_reaches_the_output_at_its_level_and_frequency_and_stops() {
    let rig = Rig::new(DummyLimits::default());
    let tone_frames = RATE * TEST_TONE_MILLISECONDS as usize / 1000;
    rig.backend
        .dispatch(UiAction::Device(DeviceAction::PlayTestTone))
        .expect("tone");

    let (left, right) = tone_output(&rig, tone_frames / FRAMES + 8);

    assert_eq!(left, right, "both channels of the pair play the tone");
    let peak = left
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let peak_db = 20.0 * peak.log10();
    assert!((peak_db + 20.0).abs() < 0.05, "peak {peak_db} dBFS");
    assert!((peak - TEST_TONE_PEAK).abs() < 0.001, "peak {peak}");

    let body = &left[tone_frames / 4..tone_frames * 3 / 4];
    let rms = (body.iter().map(|sample| sample * sample).sum::<f32>() / body.len() as f32).sqrt();
    assert!(
        (rms - TEST_TONE_PEAK * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.001,
        "rms {rms}"
    );
    let rising: Vec<usize> = body
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] <= 0.0 && pair[1] > 0.0)
        .map(|(index, _)| index)
        .collect();
    let periods = (rising.len() - 1) as f64;
    let span = (rising[rising.len() - 1] - rising[0]) as f64;
    let hertz = periods * RATE as f64 / span;
    assert!((hertz - TEST_TONE_HERTZ).abs() < 2.0, "{hertz} Hz");

    assert!(left[0].abs() < 0.001, "the fade in starts from silence");
    assert!(
        left[..40]
            .iter()
            .all(|sample| sample.abs() < TEST_TONE_PEAK * 0.2),
        "a short fade in"
    );
    assert!(
        left[tone_frames - 2..tone_frames]
            .iter()
            .all(|s| s.abs() < 0.001),
        "a fade out"
    );
    assert!(
        left[tone_frames..].iter().all(|sample| *sample == 0.0),
        "the tone stops at its end"
    );
    assert!(
        left[tone_frames - RATE / 100..tone_frames]
            .iter()
            .any(|s| s.abs() > 0.01),
        "the tone plays until its fade"
    );
}

#[test]
fn a_second_request_plays_the_tone_again() {
    let rig = Rig::new(DummyLimits::default());
    let request = || {
        rig.backend
            .dispatch(UiAction::Device(DeviceAction::PlayTestTone))
            .expect("tone");
    };
    request();
    let tone_callbacks = RATE / FRAMES + 2;
    let (first, _) = tone_output(&rig, tone_callbacks);
    assert!(first.iter().any(|sample| sample.abs() > 0.05));

    let (silent, _) = tone_output(&rig, 4);
    assert!(silent.iter().all(|sample| *sample == 0.0), "no third tone");

    request();
    let (second, _) = tone_output(&rig, 20);
    assert!(second.iter().any(|sample| sample.abs() > 0.05));
}

#[test]
fn the_test_tone_plays_on_the_chosen_output_pair() {
    let rig = Rig::new(four_channels());
    rig.act(DeviceAction::SetOutputPair(1));
    rig.backend
        .dispatch(UiAction::Device(DeviceAction::PlayTestTone))
        .expect("tone");

    let mut output = Vec::new();
    for _ in 0..40 {
        output.extend(
            rig.backend
                .render_dummy_capture(&vec![0.0; FRAMES * 4])
                .expect("render"),
        );
    }

    assert_eq!(peak_of_channel(&output, 4, 0), 0.0);
    assert_eq!(peak_of_channel(&output, 4, 1), 0.0);
    assert!(peak_of_channel(&output, 4, 2) > 0.05);
    assert!(peak_of_channel(&output, 4, 3) > 0.05);
}

#[test]
fn the_test_tone_plays_while_hear_yourself_is_off() {
    let rig = Rig::new(four_channels());
    rig.act(DeviceAction::SetInputChannels(vec![0]));
    rig.act(DeviceAction::SetHearYourself(false));
    rig.backend
        .dispatch(UiAction::Device(DeviceAction::PlayTestTone))
        .expect("tone");

    // A loud guitar plays during the tone, and only the tone reaches the output.
    let mut left: Vec<f32> = Vec::new();
    for block in 0..20 {
        let samples: Vec<f32> = (0..FRAMES)
            .flat_map(|frame| {
                let time = (block * FRAMES + frame) as f32 / RATE as f32;
                [
                    0.5 * (std::f32::consts::TAU * 220.0 * time).sin(),
                    0.0,
                    0.0,
                    0.0,
                ]
            })
            .collect();
        let output = rig.backend.render_dummy_capture(&samples).expect("render");
        left.extend(output.iter().step_by(4));
    }

    let peak = left
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(peak > 0.05, "the tone plays, peak {peak}");
    assert!(
        peak <= TEST_TONE_PEAK + 0.001,
        "the guitar plays, peak {peak}"
    );
}

#[test]
fn a_test_tone_with_no_stream_does_nothing_and_is_no_error() {
    let device = DummyBackend::connected();
    let directory = tempfile::tempdir().expect("tempdir");
    let backend = StandaloneBackend::with_dummy_device(
        processor(),
        &directory.path().join(PREFERENCES_SETTINGS),
        device.clone(),
    )
    .expect("backend");
    device.set_connected(false);
    let _ = backend.device_settings();

    backend
        .dispatch(UiAction::Device(DeviceAction::PlayTestTone))
        .expect("a tone with no stream is no error");
}

// ---- A frame never waits for device work ----

/// A gate that a test holds closed, so that the device work stays inside one call.
#[derive(Default)]
struct Gate {
    closed: Mutex<bool>,
    changed: Condvar,
    calls: AtomicUsize,
}

impl Gate {
    fn close(&self) {
        *self.closed.lock().expect("gate") = true;
    }

    fn open(&self) {
        *self.closed.lock().expect("gate") = false;
        self.changed.notify_all();
    }

    fn pass(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut closed = self.closed.lock().expect("gate");
        while *closed {
            closed = self.changed.wait(closed).expect("gate");
        }
    }

    fn wait_for_a_call(&self) {
        let start = Instant::now();
        while self.calls.load(Ordering::SeqCst) == 0 {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "no device work ran"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// A backend with a slow server: each call waits for the gate. It wraps the dummy backend.
struct SlowBackend {
    inner: DummyBackend,
    gate: Arc<Gate>,
}

impl AudioBackend for SlowBackend {
    fn enumerate(&self) -> Result<Vec<DeviceDescriptor>, AudioError> {
        self.gate.pass();
        self.inner.enumerate()
    }

    fn capabilities(
        &self,
        device: Option<&str>,
        input: bool,
    ) -> Result<DeviceCapabilities, AudioError> {
        self.gate.pass();
        self.inner.capabilities(device, input)
    }

    fn negotiate(&self, requested: &DeviceConfig) -> Result<NegotiatedStream, AudioError> {
        self.gate.pass();
        self.inner.negotiate(requested)
    }

    fn open(
        &mut self,
        config: &DeviceConfig,
        worker: AudioWorker,
    ) -> Result<AudioStream, AudioError> {
        self.gate.pass();
        AudioBackend::open(&mut self.inner, config, worker)
    }
}

fn slow_factory(gate: &Arc<Gate>, device: &DummyBackend) -> AudioBackendFactory {
    let gate = Arc::clone(gate);
    let device = device.clone();
    Arc::new(move |_| {
        Box::new(SlowBackend {
            inner: device.clone(),
            gate: Arc::clone(&gate),
        })
    })
}

fn slow_backend(path: &Path, gate: &Arc<Gate>, device: &DummyBackend) -> StandaloneBackend {
    StandaloneBackend::with_audio_backend(
        processor(),
        path,
        slow_factory(gate, device),
        AudioBackendKind::Dummy,
    )
    .expect("backend")
}

/// All that one frame of the page asks from the backend. Returns the time of the slowest frame.
fn slowest_frame(backend: &StandaloneBackend, frames: usize) -> Duration {
    let mut slowest = Duration::ZERO;
    for _ in 0..frames {
        let started = Instant::now();
        let _ = backend.device_settings();
        let _ = backend.meter_levels();
        let _ = backend.chain_faults();
        let _ = backend.input_channel_levels();
        backend.refresh_devices();
        slowest = slowest.max(started.elapsed());
        std::thread::sleep(Duration::from_millis(2));
    }
    slowest
}

/// A frame at 60 Hz has about 16 ms. A busy test machine can stop a thread, so that the limit
/// leaves room for that and it still fails on a wait for the device work, which never ends here.
const FRAME_LIMIT: Duration = Duration::from_millis(100);

#[test]
fn a_frame_returns_while_the_first_open_waits_for_the_server() {
    let gate = Arc::new(Gate::default());
    gate.close();
    let directory = tempfile::tempdir().expect("tempdir");
    let device = DummyBackend::connected();

    let started = Instant::now();
    let backend = slow_backend(&directory.path().join(PREFERENCES_SETTINGS), &gate, &device);
    assert!(
        started.elapsed() < FRAME_LIMIT,
        "the window must not wait for the server"
    );
    gate.wait_for_a_call();

    let slowest = slowest_frame(&backend, 100);
    assert!(slowest < FRAME_LIMIT, "a frame took {slowest:?}");
    let state = backend.device_settings().expect("device state");
    assert_eq!(state.status.as_deref(), Some("Opening…"));
    assert!(!state.banner.device_open);
    eprintln!("slowest frame during the first open: {slowest:?}");

    gate.open();
    backend.settle_audio().expect("settle");
    let state = backend.device_settings().expect("device state");
    assert_eq!(state.status, None);
    assert!(state.banner.device_open);
}

#[test]
fn a_frame_and_a_click_return_while_a_reopen_waits_for_the_device() {
    let gate = Arc::new(Gate::default());
    let directory = tempfile::tempdir().expect("tempdir");
    let device = DummyBackend::connected();
    let backend = slow_backend(&directory.path().join(PREFERENCES_SETTINGS), &gate, &device);
    backend.settle_audio().expect("first open");
    assert!(backend.device_settings().expect("state").banner.device_open);

    gate.close();
    gate.calls.store(0, Ordering::SeqCst);
    let started = Instant::now();
    backend
        .dispatch(UiAction::Device(DeviceAction::Restart))
        .expect("restart");
    assert!(started.elapsed() < FRAME_LIMIT, "the click must not wait");
    gate.wait_for_a_call();

    let slowest = slowest_frame(&backend, 100);
    assert!(slowest < FRAME_LIMIT, "a frame took {slowest:?}");
    let state = backend.device_settings().expect("device state");
    assert_eq!(state.status.as_deref(), Some("Opening…"));
    assert!(
        state.banner.device_open,
        "the banner stays still during a short reopen"
    );

    // A change of a knob and a second device action also do not wait.
    let started = Instant::now();
    backend
        .dispatch(UiAction::SetStereoMode(true))
        .expect("edit");
    backend
        .dispatch(UiAction::Device(DeviceAction::SetHearYourself(false)))
        .expect("toggle");
    assert!(started.elapsed() < FRAME_LIMIT, "an edit must not wait");

    gate.open();
    backend.settle_audio().expect("settle");
    let state = backend.device_settings().expect("device state");
    assert!(state.banner.device_open);
    assert_eq!(state.status, None);
    let status = backend.audio_status().expect("status");
    assert_eq!(status.chain_error, None);
    assert!(
        backend.chain_state(0).chain.stereo_enabled,
        "the edit during the job reached the session"
    );
}

#[test]
fn a_frame_returns_while_the_device_list_waits_after_a_lost_device() {
    let gate = Arc::new(Gate::default());
    let directory = tempfile::tempdir().expect("tempdir");
    let device = DummyBackend::connected();
    let backend = slow_backend(&directory.path().join(PREFERENCES_SETTINGS), &gate, &device);
    backend.settle_audio().expect("first open");

    gate.close();
    gate.calls.store(0, Ordering::SeqCst);
    device.set_connected(false);
    let started = Instant::now();
    let state = backend.device_settings().expect("device state");
    assert!(started.elapsed() < FRAME_LIMIT);
    assert!(!state.banner.device_open, "the loss shows at once");
    assert!(state.error.is_some());
    gate.wait_for_a_call();

    let slowest = slowest_frame(&backend, 50);
    assert!(slowest < FRAME_LIMIT, "a frame took {slowest:?}");

    gate.open();
    backend.settle_audio().expect("settle");
}

#[test]
fn a_job_that_never_returns_ends_with_a_message_and_a_later_choice_works() {
    let gate = Arc::new(Gate::default());
    let directory = tempfile::tempdir().expect("tempdir");
    let device = DummyBackend::connected();
    let gate_for_first = Arc::clone(&gate);
    let device_for_factory = device.clone();
    let made = Arc::new(AtomicUsize::new(0));
    let made_in_factory = Arc::clone(&made);
    // Only the first backend hangs. The next one is healthy, as a new connection to a server is.
    let factory: AudioBackendFactory = Arc::new(move |_| {
        if made_in_factory.fetch_add(1, Ordering::SeqCst) == 0 {
            Box::new(SlowBackend {
                inner: device_for_factory.clone(),
                gate: Arc::clone(&gate_for_first),
            })
        } else {
            Box::new(device_for_factory.clone())
        }
    });
    gate.close();
    let backend = StandaloneBackend::with_audio_backend(
        processor(),
        &directory.path().join(PREFERENCES_SETTINGS),
        factory,
        AudioBackendKind::Dummy,
    )
    .expect("backend");
    backend
        .set_device_work_limit(Duration::from_millis(50))
        .expect("limit");
    gate.wait_for_a_call();
    std::thread::sleep(Duration::from_millis(60));

    let state = backend.device_settings().expect("device state");
    assert_eq!(state.status, None);
    assert!(
        state.error.expect("an error").contains("does not answer"),
        "the user knows why"
    );

    backend
        .dispatch(UiAction::Device(DeviceAction::Restart))
        .expect("retry");
    backend.settle_audio().expect("settle");
    let state = backend.device_settings().expect("device state");
    assert!(state.banner.device_open, "{:?}", state.error);
    assert!(state.error.is_none());
    let generation = backend.audio_status().expect("status").active_generation;

    // The old job ends late, and its stream must not replace the new stream.
    gate.open();
    std::thread::sleep(Duration::from_millis(50));
    backend.settle_audio().expect("settle");
    assert_eq!(
        backend.audio_status().expect("status").active_generation,
        generation
    );
    assert!(backend.device_settings().expect("state").banner.device_open);
}

#[test]
fn an_exit_does_not_wait_for_a_job_that_the_engine_gave_up_on() {
    let gate = Arc::new(Gate::default());
    gate.close();
    let directory = tempfile::tempdir().expect("tempdir");
    let device = DummyBackend::connected();
    let backend = slow_backend(&directory.path().join(PREFERENCES_SETTINGS), &gate, &device);
    backend
        .set_device_work_limit(Duration::from_millis(50))
        .expect("limit");
    gate.wait_for_a_call();
    std::thread::sleep(Duration::from_millis(60));
    let state = backend.device_settings().expect("device state");
    assert!(
        state.error.expect("an error").contains("does not answer"),
        "the engine gave up on the job"
    );

    // No later job starts a thread, so the thread of the stuck job is the newest device thread.
    let (dropped, done) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(backend);
        let _ = dropped.send(());
    });
    let result = done.recv_timeout(Duration::from_secs(20));
    // The stuck thread ends now, also when the drop failed.
    gate.open();
    result.expect("the exit waited for the sound server");
}

#[test]
fn a_rejected_request_appears_in_the_state_with_no_wait() {
    let rig = Rig::new(DummyLimits {
        sample_rates: vec![48_000],
        ..DummyLimits::default()
    });
    let mut config = rig.state().config();
    config.sample_rate = Some(96_000.0);

    rig.backend
        .dispatch(UiAction::Device(DeviceAction::Configure(config)))
        .expect("the click only records the request");
    rig.backend.settle_audio().expect("settle");

    let state = rig.state();
    assert!(state.error.is_some());
    assert!(!state.banner.device_open);
}
