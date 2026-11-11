use std::{collections::BTreeMap, path::PathBuf};

use quick_xml::{Reader, events::Event};
use ronie_core::{PropertiesFile, SettingsStore};

use crate::audio::{AudioBackendKind, AudioError, DeviceConfig};

const AUDIO_SETUP: &str = "audioSetup";
const OPEN_SENTINEL: &str = "audioOpenSentinel";
/// Records that the one-time move of a saved buffer has run.
const BUFFER_MOVE_MARKER: &str = "audioBuffer256Migrated";
/// The target of that move. It stays fixed when the default buffer changes.
const BUFFER_MOVE_FRAMES: u32 = 256;
/// Records that the one-time move to the system route has run.
const ROUTE_MOVE_MARKER: &str = "audioRouteMigrated";
/// Whether the output plays the processed input. A file with no value plays it, as before.
const HEAR_YOURSELF: &str = "audioHearYourself";
/// The buffer that OPTIMIZE found for each device pair, as a JSON object of frames.
const OPTIMIZED_BUFFERS: &str = "audioOptimizedBuffers";

/// JUCE-compatible standalone audio settings with eager atomic persistence.
#[derive(Debug)]
pub struct StandaloneAudioSettings {
    store: SettingsStore,
    config: DeviceConfig,
    hear_yourself: bool,
    optimized_buffers: BTreeMap<String, u32>,
}

impl StandaloneAudioSettings {
    /// Loads one settings file, using system-default audio when no setup exists.
    ///
    /// The first load moves a saved buffer above 256 frames to 256 and records a marker, so a
    /// later user choice stays. A file with no choice also records the marker, because the user
    /// can make that choice before the next load.
    ///
    /// The first load also moves an older setup to the new defaults, one time. An older setup
    /// holds ALSA and a display name that many devices share, because that was all the older page
    /// could save, so that those values are not a choice of the user. A JACK setup stays on JACK.
    ///
    /// # Errors
    /// Returns I/O or malformed-properties errors.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, AudioError> {
        let store = SettingsStore::new(path.into());
        let properties = store.read()?;
        let mut config = properties
            .get(AUDIO_SETUP)
            .map(parse_audio_setup)
            .transpose()?
            .unwrap_or_default();
        let buffer_pending = !properties.get_bool(BUFFER_MOVE_MARKER, false);
        let route_pending = !properties.get_bool(ROUTE_MOVE_MARKER, false);
        let mut moved = false;
        if buffer_pending {
            moved = config
                .buffer_frames
                .is_some_and(|frames| frames > BUFFER_MOVE_FRAMES);
            if moved {
                config.buffer_frames = Some(BUFFER_MOVE_FRAMES);
            }
        }
        if route_pending && properties.get(AUDIO_SETUP).is_some() {
            moved |= move_route(&mut config);
        }
        if buffer_pending || route_pending {
            // A failed write must not stop the launch. The next load does the move again, and the
            // engine shows the same write failure when it saves the setup.
            let markers = [
                (buffer_pending, BUFFER_MOVE_MARKER),
                (route_pending, ROUTE_MOVE_MARKER),
            ];
            let _ = record_moves(&store, &properties, &config, moved, markers);
        }
        let hear_yourself = properties.get_bool(HEAR_YOURSELF, true);
        // A damaged value only costs a new probe, so that it must not stop the launch.
        let optimized_buffers = properties
            .get(OPTIMIZED_BUFFERS)
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        Ok(Self {
            store,
            config,
            hear_yourself,
            optimized_buffers,
        })
    }

    /// Returns the configuration captured by the latest load.
    #[must_use]
    pub const fn config(&self) -> &DeviceConfig {
        &self.config
    }

    /// Returns the Hear Yourself choice captured by the latest load. A file with no choice
    /// returns true.
    #[must_use]
    pub const fn hear_yourself(&self) -> bool {
        self.hear_yourself
    }

    /// The buffer that OPTIMIZE found for each device pair, as the latest load read it.
    #[must_use]
    pub const fn optimized_buffers(&self) -> &BTreeMap<String, u32> {
        &self.optimized_buffers
    }

    /// Eagerly persists the buffer that OPTIMIZE found for each device pair.
    ///
    /// # Errors
    /// Returns lock or atomic-write errors.
    pub fn save_optimized_buffers(
        &self,
        buffers: &BTreeMap<String, u32>,
    ) -> Result<(), AudioError> {
        let json = serde_json::to_string(buffers)
            .map_err(|error| AudioError::InvalidConfig(error.to_string()))?;
        self.store.merge_key(OPTIMIZED_BUFFERS, &json)?;
        Ok(())
    }

    /// Eagerly persists one Hear Yourself choice.
    ///
    /// # Errors
    /// Returns lock or atomic-write errors.
    pub fn set_hear_yourself(&self, enabled: bool) -> Result<(), AudioError> {
        self.store
            .merge_key(HEAR_YOURSELF, if enabled { "true" } else { "false" })?;
        Ok(())
    }

    /// Persists a complete device setup while preserving enabled MIDI inputs.
    ///
    /// # Errors
    /// Returns lock, parse, or atomic-write errors.
    pub fn save_config(&self, config: &DeviceConfig) -> Result<(), AudioError> {
        let properties = self.store.read()?;
        let midi = properties
            .get(AUDIO_SETUP)
            .map(parse_midi_inputs)
            .transpose()?
            .unwrap_or_default();
        self.store
            .merge_key(AUDIO_SETUP, &audio_setup_xml(config, &midi))?;
        Ok(())
    }

    /// Eagerly persists one MIDI input enablement change.
    ///
    /// # Errors
    /// Returns lock, parse, or atomic-write errors.
    pub fn set_midi_enabled(&self, source: &str, enabled: bool) -> Result<(), AudioError> {
        let properties = self.store.read()?;
        let config = properties
            .get(AUDIO_SETUP)
            .map(parse_audio_setup)
            .transpose()?
            .unwrap_or_default();
        let mut midi = properties
            .get(AUDIO_SETUP)
            .map(parse_midi_inputs)
            .transpose()?
            .unwrap_or_default();
        midi.insert(source.to_owned(), enabled);
        self.store
            .merge_key(AUDIO_SETUP, &audio_setup_xml(&config, &midi))?;
        Ok(())
    }

    /// Reads the latest persisted enablement for one MIDI source.
    #[must_use]
    pub fn midi_enabled(&self, source: &str) -> bool {
        self.store
            .read()
            .ok()
            .and_then(|properties| {
                properties
                    .get(AUDIO_SETUP)
                    .and_then(|xml| parse_midi_inputs(xml).ok())
            })
            .and_then(|midi| midi.get(source).copied())
            .unwrap_or(false)
    }

    /// Sets the crash-detection sentinel before attempting an open.
    ///
    /// # Errors
    /// Returns lock or atomic-write errors.
    pub fn mark_open_attempt(&self) -> Result<(), AudioError> {
        self.store.merge_key(OPEN_SENTINEL, "true")?;
        Ok(())
    }

    /// Clears the crash-detection sentinel only after a clean open.
    ///
    /// # Errors
    /// Returns lock or atomic-write errors.
    pub fn mark_open_clean(&self) -> Result<(), AudioError> {
        self.store.remove_key(OPEN_SENTINEL)?;
        Ok(())
    }

    /// Reads whether the previous audio-open attempt did not complete cleanly.
    #[must_use]
    pub fn audio_open_sentinel(&self) -> bool {
        self.store
            .read()
            .map(|properties| properties.get_bool(OPEN_SENTINEL, false))
            .unwrap_or(false)
    }
}

/// Moves an older setup to the new defaults. Returns whether the setup changed.
fn move_route(config: &mut DeviceConfig) -> bool {
    match config.backend {
        AudioBackendKind::Dummy => false,
        AudioBackendKind::Jack => {
            // The device names of the older page are no identities, and JACK has its own ports.
            config.input_device = None;
            config.output_device = None;
            config.input_label = None;
            config.output_label = None;
            true
        }
        AudioBackendKind::Alsa | AudioBackendKind::System => {
            *config = DeviceConfig {
                buffer_frames: config.buffer_frames,
                ..DeviceConfig::default()
            };
            true
        }
    }
}

/// Writes the moved setup before the markers. A stop between the writes leaves the moved values
/// with no marker, and the next load only records the markers.
fn record_moves(
    store: &SettingsStore,
    properties: &PropertiesFile,
    config: &DeviceConfig,
    moved: bool,
    markers: [(bool, &str); 2],
) -> Result<(), AudioError> {
    if moved {
        let midi = properties
            .get(AUDIO_SETUP)
            .map(parse_midi_inputs)
            .transpose()?
            .unwrap_or_default();
        store.merge_key(AUDIO_SETUP, &audio_setup_xml(config, &midi))?;
    }
    for (pending, marker) in markers {
        if pending {
            store.merge_key(marker, "true")?;
        }
    }
    Ok(())
}

fn parse_audio_setup(xml: &str) -> Result<DeviceConfig, AudioError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.name().as_ref() == "AUDIOSETUP" =>
            {
                let attributes = attributes(&element)?;
                let (input_device, input_label) =
                    identity_and_label(&attributes, "inputDeviceId", "inputDeviceName");
                let (output_device, output_label) =
                    identity_and_label(&attributes, "outputDeviceId", "outputDeviceName");
                return Ok(DeviceConfig {
                    backend: attributes
                        .get("deviceType")
                        .map(|name| AudioBackendKind::from_name(name))
                        .unwrap_or_default(),
                    input_device,
                    output_device,
                    input_label,
                    output_label,
                    sample_rate: parse_optional(&attributes, "sampleRate")?,
                    buffer_frames: parse_optional(&attributes, "bufferSize")?,
                    input_channels: parse_optional(&attributes, "inputChannels")?.unwrap_or(1),
                    output_channels: parse_optional(&attributes, "outputChannels")?.unwrap_or(2),
                    input_route: [
                        parse_channel(&attributes, "inputFirstChannel", 0),
                        parse_channel(&attributes, "inputSecondChannel", 1),
                    ],
                    output_first: parse_channel(&attributes, "outputFirstChannel", 0),
                });
            }
            Ok(Event::Eof) => {
                return Err(AudioError::InvalidConfig(
                    "audioSetup has no AUDIOSETUP element".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(AudioError::InvalidConfig(format!(
                    "invalid audioSetup XML: {error}"
                )));
            }
        }
    }
}

fn parse_midi_inputs(xml: &str) -> Result<BTreeMap<String, bool>, AudioError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut midi = BTreeMap::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.name().as_ref() == "MIDIINPUT" =>
            {
                let attributes = attributes(&element)?;
                let name = attributes
                    .get("name")
                    .cloned()
                    .ok_or_else(|| AudioError::InvalidConfig("MIDIINPUT has no name".to_owned()))?;
                let enabled = matches!(
                    attributes.get("enabled").map(String::as_str),
                    Some("1" | "true")
                );
                midi.insert(name, enabled);
            }
            Ok(Event::Eof) => return Ok(midi),
            Ok(_) => {}
            Err(error) => {
                return Err(AudioError::InvalidConfig(format!(
                    "invalid MIDI settings XML: {error}"
                )));
            }
        }
    }
}

fn attributes(
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<BTreeMap<String, String>, AudioError> {
    let mut result = BTreeMap::new();
    for attribute in element.attributes().with_checks(false) {
        let attribute = attribute.map_err(|error| {
            AudioError::InvalidConfig(format!("invalid setting attribute: {error}"))
        })?;
        let key = attribute.key.as_ref().to_owned();
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
            .map_err(|error| AudioError::InvalidConfig(format!("invalid setting value: {error}")))?
            .into_owned();
        result.insert(key, value);
    }
    Ok(result)
}

fn parse_optional<T: std::str::FromStr>(
    attributes: &BTreeMap<String, String>,
    name: &str,
) -> Result<Option<T>, AudioError> {
    attributes
        .get(name)
        .filter(|value| !value.is_empty() && *value != "0")
        .map(|value| {
            value.parse().map_err(|_| {
                AudioError::InvalidConfig(format!("invalid numeric {name} audio setting"))
            })
        })
        .transpose()
}

/// A channel number from zero. Zero is a real value here, so that it does not mean "no value".
fn parse_channel(attributes: &BTreeMap<String, String>, name: &str, default: usize) -> usize {
    attributes
        .get(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// Reads one device. A file from before identities holds only a name, which then is the identity.
fn identity_and_label(
    attributes: &BTreeMap<String, String>,
    identity: &str,
    label: &str,
) -> (Option<String>, Option<String>) {
    let named = optional_string(attributes.get(label));
    if attributes.contains_key(identity) {
        (optional_string(attributes.get(identity)), named)
    } else {
        (named, None)
    }
}

fn optional_string(value: Option<&String>) -> Option<String> {
    value.filter(|value| !value.is_empty()).cloned()
}

fn audio_setup_xml(config: &DeviceConfig, midi: &BTreeMap<String, bool>) -> String {
    let mut xml = format!(
        "<AUDIOSETUP deviceType=\"{}\" inputDeviceId=\"{}\" outputDeviceId=\"{}\" inputDeviceName=\"{}\" outputDeviceName=\"{}\" sampleRate=\"{}\" bufferSize=\"{}\" inputChannels=\"{}\" outputChannels=\"{}\" inputFirstChannel=\"{}\" inputSecondChannel=\"{}\" outputFirstChannel=\"{}\">",
        config.backend.name(),
        escape(config.input_device.as_deref().unwrap_or("")),
        escape(config.output_device.as_deref().unwrap_or("")),
        escape(config.input_label.as_deref().unwrap_or("")),
        escape(config.output_label.as_deref().unwrap_or("")),
        config.sample_rate.unwrap_or(0),
        config.buffer_frames.unwrap_or(0),
        config.input_channels,
        config.output_channels,
        config.input_route[0],
        config.input_route[1],
        config.output_first,
    );
    for (name, enabled) in midi {
        xml.push_str(&format!(
            "<MIDIINPUT name=\"{}\" enabled=\"{}\"/>",
            escape(name),
            u8::from(*enabled)
        ));
    }
    xml.push_str("</AUDIOSETUP>");
    xml
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
