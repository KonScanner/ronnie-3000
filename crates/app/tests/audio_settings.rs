//! Standalone settings persistence and sentinel tests.

use ronie_app::{
    audio::{AudioBackendKind, DeviceConfig},
    audio_settings::StandaloneAudioSettings,
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
fn audio_setup_and_enabled_midi_round_trip() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");
    let settings = StandaloneAudioSettings::load(path.clone()).expect("load defaults");
    settings.save_config(&config()).expect("save config");
    settings
        .set_midi_enabled("keyboard", true)
        .expect("enable MIDI");
    settings
        .set_midi_enabled("pads", false)
        .expect("disable MIDI");

    let xml = ronie_core::PropertiesFile::read(&path)
        .expect("properties")
        .get("audioSetup")
        .expect("audioSetup key")
        .to_owned();
    assert!(xml.contains("<AUDIOSETUP"));
    assert!(xml.contains("guitar"));

    let restored = StandaloneAudioSettings::load(path).expect("reload");
    assert_eq!(restored.config(), &config());
    assert!(restored.midi_enabled("keyboard"));
    assert!(!restored.midi_enabled("pads"));
}

const BUFFER_MARKER: &str = "audioBuffer256Migrated";
const ROUTE_MARKER: &str = "audioRouteMigrated";

/// Writes the setup of an earlier release, which saved a display name that many devices share.
fn older_setup(kind: &str, stereo: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");
    let channels = if stereo { 2 } else { 1 };
    ronie_core::SettingsStore::new(path.clone())
        .merge_key(
            "audioSetup",
            &format!(
                "<AUDIOSETUP deviceType=\"{kind}\" inputDeviceName=\"Fake Card, USB Audio\" outputDeviceName=\"Fake Card, USB Audio\" sampleRate=\"44100\" bufferSize=\"128\" inputChannels=\"{channels}\" outputChannels=\"2\"><MIDIINPUT name=\"keyboard\" enabled=\"1\"/></AUDIOSETUP>"
            ),
        )
        .expect("seed audio setup");
    (directory, path)
}

#[test]
fn an_older_alsa_setup_is_no_choice_and_moves_to_the_new_defaults_one_time() {
    let (_directory, path) = older_setup("ALSA", true);

    let settings = load(&path);

    let config = settings.config();
    assert_eq!(config.backend, AudioBackendKind::System);
    assert_eq!(config.input_device, None);
    assert_eq!(config.output_device, None);
    assert_eq!(config.sample_rate, None, "the rate follows the system");
    assert_eq!(config.input_channels, 1, "one mono channel for a guitar");
    assert_eq!(
        config.buffer_frames,
        Some(128),
        "the buffer was a real choice"
    );
    assert!(settings.midi_enabled("keyboard"), "the move keeps MIDI");
    assert!(properties(&path).get_bool(ROUTE_MARKER, false));

    // The new file holds the new values, so that the next load reads them with no move.
    let reloaded = load(&path);
    assert_eq!(reloaded.config().backend, AudioBackendKind::System);
    assert_eq!(reloaded.config().input_device, None);
}

#[cfg(target_os = "linux")]
#[test]
fn a_choice_of_alsa_on_the_new_page_stays_after_the_move() {
    let (_directory, path) = older_setup("ALSA", false);
    let settings = load(&path);
    let mut config = settings.config().clone();
    config.backend = AudioBackendKind::Alsa;
    config.input_device = Some("plughw:CARD=Fake,DEV=0".to_owned());
    config.input_label = Some("Fake Card, USB Audio".to_owned());
    settings.save_config(&config).expect("save");

    let reloaded = load(&path);

    assert_eq!(reloaded.config(), &config);
}

#[cfg(target_os = "linux")]
#[test]
fn an_older_jack_setup_stays_on_jack_and_drops_its_names() {
    let (_directory, path) = older_setup("JACK", true);

    let settings = load(&path);

    let config = settings.config();
    assert_eq!(config.backend, AudioBackendKind::Jack);
    assert_eq!(config.input_device, None);
    assert_eq!(config.output_device, None);
    assert_eq!(config.input_channels, 2, "a JACK user chose this");
    assert_eq!(config.sample_rate, Some(44_100));
}

/// A setup from Linux names a driver that this operating system does not have.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_saved_jack_setup_moves_to_the_system_route_on_another_system() {
    let (_directory, path) = older_setup("JACK", true);

    let settings = load(&path);

    let config = settings.config();
    assert_eq!(config.backend, AudioBackendKind::System);
    assert_eq!(config.input_device, None);
    assert_eq!(config.output_device, None);
}

#[test]
fn a_new_file_has_no_setup_and_starts_on_the_system_route() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");

    let settings = load(&path);

    assert_eq!(settings.config().backend, AudioBackendKind::System);
    assert!(
        properties(&path).get("audioSetup").is_none(),
        "the move writes no setup that the user did not make"
    );
    assert!(properties(&path).get_bool(ROUTE_MARKER, false));
}

#[test]
fn identities_labels_and_routes_stay_through_a_save_and_a_load() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");
    let settings = load(&path);
    let config = DeviceConfig {
        backend: AudioBackendKind::System,
        input_device: Some("alsa_input.fake & <odd> \"name\"".to_owned()),
        output_device: Some("alsa_output.fake".to_owned()),
        input_label: Some("Fake Input".to_owned()),
        output_label: Some("Fake Output".to_owned()),
        sample_rate: Some(96_000),
        buffer_frames: Some(512),
        input_channels: 2,
        output_channels: 2,
        input_route: [3, 0],
        output_first: 2,
    };
    settings.save_config(&config).expect("save");

    assert_eq!(load(&path).config(), &config);
}

fn setup_xml(buffer: u32) -> String {
    format!(
        "<AUDIOSETUP deviceType=\"Dummy\" inputDeviceName=\"guitar\" outputDeviceName=\"monitors\" sampleRate=\"48000\" bufferSize=\"{buffer}\" inputChannels=\"1\" outputChannels=\"2\"><MIDIINPUT name=\"keyboard\" enabled=\"1\"/></AUDIOSETUP>"
    )
}

/// Writes a file as an earlier release wrote it, with no marker.
fn saved_settings(buffer: u32) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");
    ronie_core::SettingsStore::new(path.clone())
        .merge_key("audioSetup", &setup_xml(buffer))
        .expect("seed audio setup");
    (directory, path)
}

fn load(path: &std::path::Path) -> StandaloneAudioSettings {
    StandaloneAudioSettings::load(path).expect("load settings")
}

fn properties(path: &std::path::Path) -> ronie_core::PropertiesFile {
    ronie_core::PropertiesFile::read(path).expect("properties")
}

fn choose_buffer(path: &std::path::Path, frames: u32) {
    let settings = load(path);
    let mut config = settings.config().clone();
    config.buffer_frames = Some(frames);
    settings.save_config(&config).expect("save choice");
}

#[test]
fn a_saved_buffer_above_256_moves_to_256_and_records_the_marker() {
    for saved in [257, 512, 1024, 8192] {
        let (_directory, path) = saved_settings(saved);

        let settings = load(&path);

        assert_eq!(settings.config().buffer_frames, Some(256), "{saved}");
        let stored = properties(&path);
        let stored_setup = stored.get("audioSetup").expect("setup");
        assert!(
            stored_setup.contains("bufferSize=\"256\""),
            "{stored_setup}"
        );
        assert!(stored_setup.contains("name=\"keyboard\""), "{stored_setup}");
        assert!(stored.get_bool(BUFFER_MARKER, false));
        assert_eq!(load(&path).config().buffer_frames, Some(256), "{saved}");
    }
}

#[test]
fn the_buffer_move_keeps_the_other_audio_settings() {
    let (_directory, path) = saved_settings(1024);

    let settings = load(&path);

    assert_eq!(settings.config(), &{
        let mut expected = config();
        expected.buffer_frames = Some(256);
        expected
    });
    assert!(settings.midi_enabled("keyboard"));
}

#[test]
fn the_buffer_move_runs_one_time() {
    let (_directory, path) = saved_settings(1024);
    load(&path);
    let after_move = std::fs::read(&path).expect("settings after the move");

    load(&path);

    assert_eq!(std::fs::read(&path).expect("settings"), after_move);
    ronie_core::SettingsStore::new(path.clone())
        .merge_key("audioSetup", &setup_xml(2048))
        .expect("value after the move");
    assert_eq!(load(&path).config().buffer_frames, Some(2048));
}

#[test]
fn a_later_choice_of_1024_stays_at_each_launch() {
    let (_directory, path) = saved_settings(512);
    load(&path);

    choose_buffer(&path, 1024);

    assert_eq!(load(&path).config().buffer_frames, Some(1024));
    assert_eq!(load(&path).config().buffer_frames, Some(1024));
}

#[test]
fn a_saved_buffer_at_or_under_256_stays_and_records_the_marker() {
    for saved in [32, 128, 255, 256] {
        let (_directory, path) = saved_settings(saved);

        assert_eq!(load(&path).config().buffer_frames, Some(saved));

        let stored = properties(&path);
        assert_eq!(stored.get("audioSetup"), Some(setup_xml(saved).as_str()));
        assert!(stored.get_bool(BUFFER_MARKER, false), "{saved}");

        choose_buffer(&path, 1024);
        assert_eq!(load(&path).config().buffer_frames, Some(1024), "{saved}");
    }
}

#[test]
fn a_file_with_no_audio_setup_records_only_the_marker() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");

    assert_eq!(load(&path).config().buffer_frames, None);

    let stored = properties(&path);
    assert_eq!(stored.get("audioSetup"), None);
    assert!(stored.get_bool(BUFFER_MARKER, false));

    choose_buffer(&path, 1024);
    assert_eq!(load(&path).config().buffer_frames, Some(1024));
    assert_eq!(load(&path).config().buffer_frames, Some(1024));
}

#[test]
fn a_saved_zero_stays_no_choice_and_records_the_marker() {
    let (_directory, path) = saved_settings(0);

    assert_eq!(load(&path).config().buffer_frames, None);

    let stored = properties(&path);
    assert_eq!(stored.get("audioSetup"), Some(setup_xml(0).as_str()));
    assert!(stored.get_bool(BUFFER_MARKER, false));

    choose_buffer(&path, 1024);
    assert_eq!(load(&path).config().buffer_frames, Some(1024));
}

#[test]
fn the_buffer_move_keeps_the_other_keys_in_the_file() {
    for saved in [1024, 128] {
        let (_directory, path) = saved_settings(saved);
        let store = ronie_core::SettingsStore::new(path.clone());
        let others = [
            ("calibrateInput", "1.0"),
            ("multiCore", "false"),
            ("t3k.showPitchControl", "true"),
            ("audioOpenSentinel", "true"),
            ("future.key", "kept <&> value"),
        ];
        for (name, value) in others {
            store.merge_key(name, value).expect("seed other key");
        }

        load(&path);

        let stored = properties(&path);
        for (name, value) in others {
            assert_eq!(stored.get(name), Some(value), "{name}");
        }
        assert!(load(&path).audio_open_sentinel());
    }
}

#[test]
fn a_failed_buffer_move_write_does_not_stop_the_load() {
    let (_directory, path) = saved_settings(1024);
    let before = std::fs::read(&path).expect("settings");
    std::fs::remove_file(path.with_extension("lock")).expect("remove the settings lock");
    std::fs::create_dir(path.with_extension("lock")).expect("block the settings lock");

    let settings = load(&path);

    assert_eq!(settings.config().buffer_frames, Some(256));
    assert_eq!(std::fs::read(&path).expect("settings"), before);

    std::fs::remove_dir(path.with_extension("lock")).expect("free the settings lock");
    assert_eq!(load(&path).config().buffer_frames, Some(256));
    assert!(properties(&path).get_bool(BUFFER_MARKER, false));
}

#[test]
fn sentinel_is_cleared_only_after_a_clean_open() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");
    let settings = StandaloneAudioSettings::load(path.clone()).expect("defaults");
    assert!(!settings.audio_open_sentinel());
    settings.mark_open_attempt().expect("attempt");
    assert!(
        StandaloneAudioSettings::load(path.clone())
            .expect("reload")
            .audio_open_sentinel()
    );
    settings.mark_open_clean().expect("clean open");
    assert!(
        !StandaloneAudioSettings::load(path)
            .expect("reload")
            .audio_open_sentinel()
    );
}

#[test]
fn settings_io_uses_atomic_replacement() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");
    let settings = StandaloneAudioSettings::load(path.clone()).expect("defaults");
    settings.save_config(&config()).expect("first save");
    let first = std::fs::read(&path).expect("first settings");
    settings
        .set_midi_enabled("keyboard", true)
        .expect("second save");
    let second = std::fs::read(&path).expect("second settings");
    assert_ne!(first, second);
    assert!(!path.with_extension("tmp").exists());
}

#[test]
fn hear_yourself_is_on_with_no_saved_choice_and_keeps_each_saved_choice() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("audio.settings");
    let settings = StandaloneAudioSettings::load(path.clone()).expect("defaults");
    settings
        .save_config(&config())
        .expect("a file with no choice");
    assert!(load(&path).hear_yourself(), "a file with no choice is off");
    assert_eq!(properties(&path).get("audioHearYourself"), None);

    settings.set_hear_yourself(false).expect("save off");
    assert!(!load(&path).hear_yourself(), "the choice of off is lost");
    assert_eq!(load(&path).config(), &config(), "the setup changed");

    settings.set_hear_yourself(true).expect("save on");
    assert!(load(&path).hear_yourself(), "the choice of on is lost");
    assert!(!path.with_extension("tmp").exists());
}
