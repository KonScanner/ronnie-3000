//! The system route against a private sound server with fake nodes.
//!
//! `script/check-system-route.sh` starts that server and runs this test. Run it on its own and it
//! does nothing, because it must never reach the sound server of the desktop.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ronie_app::{
    audio::{AudioBackendKind, DeviceConfig, SYSTEM_DEFAULT_DEVICE},
    audio_settings::StandaloneAudioSettings,
    standalone_backend::{StandaloneBackend, standalone_processor},
};
use ronie_core::PREFERENCES_SETTINGS;
use ronie_ui::backend::{DeviceAction, UiAction, UiBackend};

const WAIT_LIMIT: Duration = Duration::from_secs(20);

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < WAIT_LIMIT, "no result: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The harness and this test talk through files in one directory.
struct Harness(PathBuf);

impl Harness {
    fn mark(&self, name: &str) {
        std::fs::write(self.0.join(name), b"").expect("mark");
    }
}

fn act(backend: &StandaloneBackend, action: DeviceAction) {
    backend
        .dispatch(UiAction::Device(action))
        .expect("device action");
    backend.settle_audio().expect("settle");
}

#[test]
fn a_signal_and_the_test_tone_pass_through_a_private_sound_server() {
    let Some(directory) = std::env::var_os("RONNIE_PRIVATE_SERVER") else {
        eprintln!("skipped: script/check-system-route.sh runs this test");
        return;
    };
    let harness = Harness(PathBuf::from(directory));
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("runtime directory");
    assert!(
        Path::new(&runtime).starts_with(&harness.0),
        "this test runs only on the private server"
    );
    assert!(
        Path::new(&runtime).join("pulse/native").exists(),
        "a system route with no server would run on ALSA"
    );

    let root = tempfile::tempdir().expect("settings directory");
    let path = root.path().join(PREFERENCES_SETTINGS);
    let frames: u32 = std::env::var("RONNIE_BUFFER")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(256);
    StandaloneAudioSettings::load(&path)
        .expect("settings")
        .save_config(&DeviceConfig {
            backend: AudioBackendKind::System,
            buffer_frames: Some(frames),
            ..DeviceConfig::default()
        })
        .expect("save the buffer");
    let processor = standalone_processor(root.path()).expect("processor");
    let backend = StandaloneBackend::real(processor, &path, None).expect("backend");

    // The first open runs on the worker, and this thread asks as a frame does.
    wait_until("the first stream", || {
        backend
            .device_settings()
            .is_some_and(|state| state.status.is_none())
    });
    let state = backend.device_settings().expect("device state");
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(state.banner.device_open);
    assert_eq!(state.banner.current_type, "System");
    assert_eq!(state.banner.sample_rate, 48_000.0, "the rate of the server");
    assert_eq!(state.banner.buffer_size, frames);
    assert_eq!(
        state.banner.input_device, "Fake Interface Input 1 Inst/Line",
        "the instrument input and not the system default"
    );
    assert_eq!(state.selected_output, SYSTEM_DEFAULT_DEVICE);
    assert_eq!(
        state.input_channels.len(),
        1,
        "a mono source has one channel"
    );
    let labels: Vec<&str> = state
        .input_devices
        .iter()
        .chain(&state.output_devices)
        .map(|row| row.label.as_str())
        .collect();
    assert!(
        labels.iter().all(|label| !label.contains("Monitor of")),
        "{labels:?}"
    );
    println!("rows: {labels:?}");
    println!("rates: {:?}", state.sample_rates);
    println!("buffers: {:?}", state.banner.buffer_sizes);

    // The harness links the stream, and then plays a sine of -12 dBFS into the fake input.
    harness.mark("stream-ready");
    wait_until("the signal at the input", || {
        backend.meter_levels().input_db[0] > -20.0
    });
    wait_until("the signal at the output", || {
        backend.meter_levels().output_db[0] > -40.0
    });
    // A meter holds one short window, and a window can fall between two deliveries. So this reads
    // the highest values of two seconds, and the recording of the harness judges the gaps.
    let started = Instant::now();
    let (mut input, mut output, mut load) = (-60.0_f32, [-60.0_f32; 2], 0.0_f32);
    while started.elapsed() < Duration::from_secs(2) {
        let meters = backend.meter_levels();
        input = input.max(meters.input_db[0]);
        output[0] = output[0].max(meters.output_db[0]);
        output[1] = output[1].max(meters.output_db[1]);
        load = load.max(meters.cpu);
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "signal: input {input:.2} dB, output {:.2} dB {:.2} dB, highest callback load {load:.4}",
        output[0], output[1]
    );
    assert!((input + 12.0).abs() < 1.0, "{input}");
    assert!(output[0] > -30.0, "{}", output[0]);
    assert!(output[1] > -30.0, "{}", output[1]);
    assert!(load < 0.5, "{load}");
    let levels = backend.input_channel_levels();
    assert_eq!(levels.len(), 1, "{levels:?}");

    // A two-channel source, with a signal only on its second channel.
    let webcam = backend
        .device_settings()
        .expect("state")
        .input_devices
        .iter()
        .find(|row| row.label == "Fake Webcam Microphone")
        .expect("a row")
        .id
        .clone();
    let mut config = backend.device_settings().expect("state").config();
    config.input = Some(webcam);
    act(&backend, DeviceAction::Configure(config));
    harness.mark("second-stream-ready");
    let state = backend.device_settings().expect("device state");
    assert!(state.banner.device_open, "{:?}", state.error);
    assert_eq!(state.input_channels.len(), 2, "a stereo source has two");
    wait_until("the signal on the second channel", || {
        backend
            .input_channel_levels()
            .get(1)
            .is_some_and(|level| *level > -20.0)
    });
    let levels = backend.input_channel_levels();
    println!("channels of the second source: {levels:?}");
    assert!(levels[0] < -50.0, "{levels:?}");
    assert!(
        backend.meter_levels().input_db[0] < -50.0,
        "the first channel feeds the chain"
    );
    backend
        .dispatch(UiAction::Device(DeviceAction::SetInputChannels(vec![1])))
        .expect("route");
    wait_until("the chosen channel at the chain", || {
        backend.meter_levels().input_db[0] > -20.0
    });
    println!("the route changed with no new stream");
    let log = backend.diagnostic_log().expect("log");
    assert_eq!(
        log.matches("stream opened").count(),
        2,
        "a route is no new stream: {log}"
    );

    // No signal now, and only the test tone reaches the sink.
    harness.mark("feed-can-stop");
    wait_until("silence at the input", || {
        backend.meter_levels().input_db[0] < -50.0
    });
    std::thread::sleep(Duration::from_millis(500));
    harness.mark("tone-starts");
    act(&backend, DeviceAction::PlayTestTone);
    std::thread::sleep(Duration::from_millis(300));
    let mut during = -60.0_f32;
    for _ in 0..40 {
        during = during.max(backend.meter_levels().output_db[0]);
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("output during the test tone: {during:.2} dB");
    assert!((during + 20.0).abs() < 1.0, "{during}");
    wait_until("the end of the test tone", || {
        backend.meter_levels().output_db[0] < -50.0
    });
    let after = backend.meter_levels().output_db[0];
    println!("output after the test tone: {after:.2} dB");
    assert!(after < -50.0, "{after}");
    harness.mark("tone-ended");

    let log = backend.diagnostic_log().expect("log");
    println!("--- log\n{log}");
    let settings_text = std::fs::read_to_string(&path).expect("settings");
    assert!(settings_text.contains("deviceType"));
    harness.mark("done");
}
