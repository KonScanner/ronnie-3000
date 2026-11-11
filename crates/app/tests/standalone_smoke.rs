//! Dummy and hardware-gated standalone smoke tests.

use ronie_app::audio::{AudioBackendKind, DeviceConfig, DummyBackend};

fn config(rate: u32, frames: u32) -> DeviceConfig {
    DeviceConfig {
        backend: AudioBackendKind::Dummy,
        input_device: Some("guitar".to_owned()),
        output_device: Some("monitors".to_owned()),
        sample_rate: Some(rate),
        buffer_frames: Some(frames),
        input_channels: 1,
        output_channels: 2,
        ..DeviceConfig::default()
    }
}

#[test]
fn dummy_end_to_end_render_works_at_48k_and_44100() {
    for (rate, frames) in [(48_000, 128), (44_100, 64), (96_000, 128)] {
        let config = config(rate, frames);
        let mut backend = DummyBackend::connected();
        let mut stream = backend.open_dummy(&config).expect("dummy open");
        stream.start().expect("dummy start");
        for _ in 0..4 {
            let rendered = stream.render().expect("dummy render");
            assert_eq!(rendered.len(), frames as usize * 2);
            assert!(rendered.iter().all(|sample| sample.abs() <= f32::EPSILON));
        }
        stream.close().expect("dummy close");
    }
}

#[test]
fn dummy_large_buffer_close_uses_bounded_timeout() {
    let config = config(8_000, 4096);
    let mut backend = DummyBackend::connected();
    let stream = backend.open_dummy(&config).expect("dummy open");
    let started = std::time::Instant::now();
    stream.close().expect("large-buffer close");
    assert!(started.elapsed() < std::time::Duration::from_millis(950));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a real ALSA device and nonexclusive access"]
fn real_alsa_open_start_close() {
    let mut backend = ronie_app::audio::CpalBackend::new(AudioBackendKind::Alsa);
    let config = DeviceConfig {
        backend: AudioBackendKind::Alsa,
        input_device: None,
        output_device: None,
        sample_rate: Some(48_000),
        buffer_frames: Some(128),
        input_channels: 1,
        output_channels: 2,
        ..DeviceConfig::default()
    };
    let worker = ronie_app::audio::AudioWorker::new(&config).expect("worker");
    let mut stream = backend.open(&config, worker).expect("ALSA open");
    stream.start().expect("ALSA start");
    stream.close().expect("ALSA close");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a running JACK server"]
fn real_jack_open_start_close() {
    let mut backend = ronie_app::audio::CpalBackend::new(AudioBackendKind::Jack);
    let config = DeviceConfig {
        backend: AudioBackendKind::Jack,
        input_device: None,
        output_device: None,
        sample_rate: Some(48_000),
        buffer_frames: Some(128),
        input_channels: 1,
        output_channels: 2,
        ..DeviceConfig::default()
    };
    let worker = ronie_app::audio::AudioWorker::new(&config).expect("worker");
    let mut stream = backend.open(&config, worker).expect("JACK open");
    stream.start().expect("JACK start");
    stream.close().expect("JACK close");
}
