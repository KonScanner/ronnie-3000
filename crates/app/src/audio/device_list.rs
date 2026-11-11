//! Pure rules that turn raw host devices into a clean list and one chosen default.
//!
//! A host can show one card many times. These rules keep one row for each real endpoint, and they
//! need no device, so that a test can prove them with fake names.

use super::{DeviceConfig, DeviceDescriptor};

/// The row and the saved value that mean "follow the system default device".
pub const SYSTEM_DEFAULT_DEVICE: &str = "system-default";

/// The sample rates that the settings can offer, before a device filters them.
pub const COMMON_SAMPLE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// The most channels that a stream opens on a real device. A larger device keeps these channels.
pub const MAX_DEVICE_CHANNELS: usize = 32;

/// The channels of a device that only forwards audio to a server. It reports what a server could
/// take, and that is not a count of real channels.
const VIRTUAL_DEVICE_CHANNELS: usize = 2;

/// Names of common audio interfaces. A row with one of these names is a better guitar input than
/// a webcam microphone.
const KNOWN_INTERFACES: [&str; 24] = [
    "scarlett",
    "focusrite",
    "clarett",
    "umc",
    "behringer",
    "motu",
    "presonus",
    "audient",
    "m-audio",
    "m-track",
    "universal audio",
    "apollo",
    "rme",
    "babyface",
    "steinberg",
    "tascam",
    "line 6",
    "helix",
    "irig",
    "komplete audio",
    "apogee",
    "rubix",
    "ssl 2",
    "evo 4",
];

/// ALSA plugin and device names that repeat another row or are not an endpoint.
const ALSA_NOISE: [&str; 25] = [
    "a52",
    "center_lfe",
    "dca",
    "dmix",
    "dsnoop",
    "front",
    "hdmi",
    "iec958",
    "jack",
    "lavrate",
    "modem",
    "null",
    "oss",
    "phoneline",
    "pipewire",
    "pulse",
    "rear",
    "samplerate",
    "side",
    "speex",
    "speexrate",
    "sysdefault",
    "upmix",
    "usbstream",
    "vdownmix",
];

/// One device as a host reports it, before the rules of a driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDevice {
    /// Stable identity in the host.
    pub id: String,
    /// The first line of the description. This is often the same for many devices of one card.
    pub name: String,
    /// The other lines of the description.
    pub detail: Vec<String>,
    /// Whether capture is possible.
    pub input: bool,
    /// Whether playback is possible.
    pub output: bool,
    /// Whether the host names this device as its default input.
    pub default_input: bool,
    /// Whether the host names this device as its default output.
    pub default_output: bool,
}

impl RawDevice {
    fn into_descriptor(self, listed: bool) -> DeviceDescriptor {
        DeviceDescriptor {
            id: self.id,
            label: self.name,
            input: self.input,
            output: self.output,
            default_input: self.default_input,
            default_output: self.default_output,
            listed,
        }
    }
}

/// The card and device that a `hw:` or `plughw:` name points at.
fn alsa_endpoint(id: &str) -> Option<(&str, &str)> {
    let (_, arguments) = id.split_once(':')?;
    let mut card = None;
    let mut device = "0";
    for argument in arguments.split(',') {
        match argument.split_once('=') {
            Some(("CARD", value)) => card = Some(value),
            Some(("DEV", value)) => device = value,
            _ => {}
        }
    }
    Some((card?, device))
}

fn alsa_plugin(id: &str) -> &str {
    id.split(':').next().unwrap_or(id)
}

/// Keeps `default`, one row for each card endpoint, and any device that the user defined.
///
/// A row with a card name wins over the same device with a card number. The numbered row comes
/// from the kernel, so that its direction is the true one, and the kept row takes it. No device
/// opens here.
#[must_use]
pub fn clean_alsa_devices(raw: Vec<RawDevice>) -> Vec<DeviceDescriptor> {
    let numbered: Vec<RawDevice> = raw
        .iter()
        .filter(|device| {
            matches!(alsa_plugin(&device.id), "hw" | "plughw")
                && alsa_endpoint(&device.id)
                    .is_some_and(|(card, _)| card.chars().all(|c| c.is_ascii_digit()))
        })
        .cloned()
        .collect();
    let named_endpoint = |device: &RawDevice, plugin: &str| {
        raw.iter().any(|other| {
            alsa_plugin(&other.id) == plugin
                && alsa_endpoint(&other.id)
                    .is_some_and(|(card, _)| !card.chars().all(|c| c.is_ascii_digit()))
                && other.name == device.name
        })
    };
    let mut kept = Vec::new();
    for mut device in raw.iter().cloned() {
        let plugin = alsa_plugin(&device.id);
        if plugin == "default" || device.id == "default" {
            kept.push(device.into_descriptor(false));
            continue;
        }
        if ALSA_NOISE.contains(&plugin) || plugin.starts_with("surround") {
            continue;
        }
        match plugin {
            "hw" => {
                // A direct name is only for a device with no conversion row.
                let endpoint = alsa_endpoint(&device.id);
                let has_plug = raw.iter().any(|other| {
                    alsa_plugin(&other.id) == "plughw"
                        && (alsa_endpoint(&other.id) == endpoint || other.name == device.name)
                });
                if has_plug {
                    continue;
                }
            }
            "plughw" => {
                let is_number = alsa_endpoint(&device.id)
                    .is_some_and(|(card, _)| card.chars().all(|c| c.is_ascii_digit()));
                if is_number && named_endpoint(&device, "plughw") {
                    continue;
                }
                if !is_number
                    && let Some(kernel) = numbered.iter().find(|other| other.name == device.name)
                {
                    device.input = kernel.input;
                    device.output = kernel.output;
                }
            }
            _ => {}
        }
        kept.push(device.into_descriptor(true));
    }
    separate_equal_labels(kept, &raw)
}

/// Keeps each sink and source of a sound server, except the monitor of a sink.
///
/// A monitor carries what the computer plays, so that a guitar is not on it. A source can have
/// "Monitor" in its description, and that stays, because only the node name tells them apart.
#[must_use]
pub fn clean_server_devices(raw: Vec<RawDevice>) -> Vec<DeviceDescriptor> {
    let kept = raw
        .iter()
        .cloned()
        .map(|device| {
            let listed = !device.id.ends_with(".monitor");
            device.into_descriptor(listed)
        })
        .collect();
    separate_equal_labels(kept, &raw)
}

/// Gives each listed row of one direction a label of its own, first with the next line of its
/// description, and then with a number.
fn separate_equal_labels(
    mut devices: Vec<DeviceDescriptor>,
    raw: &[RawDevice],
) -> Vec<DeviceDescriptor> {
    let same = |a: &DeviceDescriptor, b: &DeviceDescriptor| {
        a.label == b.label && a.listed && b.listed && (a.input && b.input || a.output && b.output)
    };
    let snapshot = devices.clone();
    for device in &mut devices {
        let repeated = snapshot
            .iter()
            .filter(|other| same(other, device) && other.id != device.id)
            .count()
            > 0;
        if !repeated {
            continue;
        }
        let detail = raw
            .iter()
            .find(|row| row.id == device.id)
            .and_then(|row| row.detail.iter().find(|line| **line != row.name));
        if let Some(detail) = detail {
            device.label = format!("{}, {detail}", device.label);
        }
    }
    let snapshot = devices.clone();
    for (index, device) in devices.iter_mut().enumerate() {
        let earlier = snapshot[..index]
            .iter()
            .filter(|other| same(other, device))
            .count();
        if earlier > 0 {
            device.label = format!("{} ({})", device.label, earlier + 1);
        }
    }
    devices
}

/// Whether an ALSA device forwards to a server and has no real channel count.
#[must_use]
pub(super) fn alsa_device_is_virtual(id: &str) -> bool {
    !matches!(alsa_plugin(id), "hw" | "plughw")
}

/// The channel count for a device that reports `reported` channels.
#[must_use]
pub(super) fn sane_channel_count(reported: usize, is_virtual: bool) -> usize {
    let limit = if is_virtual {
        VIRTUAL_DEVICE_CHANNELS
    } else {
        MAX_DEVICE_CHANNELS
    };
    reported.clamp(1, limit)
}

/// The label of the system default row, when the host has a default device.
#[must_use]
pub fn system_default_label(devices: &[DeviceDescriptor], input: bool) -> Option<&str> {
    devices
        .iter()
        .find(|device| {
            if input {
                device.default_input
            } else {
                device.default_output
            }
        })
        .map(|device| device.label.as_str())
}

/// The input for a user with no saved choice: a row for an instrument, then an audio interface,
/// then the system default.
///
/// The result is `None` for the system default. A monitor or a loopback row of an interface is
/// no guitar input, so that it comes after the other rows of that interface.
#[must_use]
pub fn automatic_input(devices: &[DeviceDescriptor]) -> Option<&DeviceDescriptor> {
    let inputs = || {
        devices
            .iter()
            .filter(|device| device.input && device.listed)
    };
    if let Some(instrument) = inputs().find(|device| device.label.contains("Inst")) {
        return Some(instrument);
    }
    let known = |device: &&DeviceDescriptor| {
        let label = device.label.to_lowercase();
        KNOWN_INTERFACES.iter().any(|name| label.contains(name))
    };
    let is_return = |device: &&DeviceDescriptor| {
        let label = device.label.to_lowercase();
        label.contains("monitor") || label.contains("loopback")
    };
    inputs()
        .filter(known)
        .find(|device| !is_return(device))
        .or_else(|| inputs().find(known))
}

/// One device that a stream will use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDevice {
    /// The row to show as selected.
    pub row: String,
    /// The identity to open. `None` asks the host for its default device.
    pub open: Option<String>,
    /// The label of the device, or an empty text when no device exists.
    pub label: String,
    /// A message for a saved device that is gone.
    pub notice: Option<String>,
}

/// The real devices that one saved request means now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    /// The capture device.
    pub input: ResolvedDevice,
    /// The playback device.
    pub output: ResolvedDevice,
}

fn system_default(devices: &[DeviceDescriptor], input: bool) -> ResolvedDevice {
    let label = system_default_label(devices, input).unwrap_or_default();
    ResolvedDevice {
        row: if label.is_empty() {
            String::new()
        } else {
            SYSTEM_DEFAULT_DEVICE.to_owned()
        },
        open: None,
        label: label.to_owned(),
        notice: None,
    }
}

fn automatic(devices: &[DeviceDescriptor], input: bool) -> ResolvedDevice {
    if input && let Some(device) = automatic_input(devices) {
        return ResolvedDevice {
            row: device.id.clone(),
            open: Some(device.id.clone()),
            label: device.label.clone(),
            notice: None,
        };
    }
    system_default(devices, input)
}

fn resolve_device(
    devices: &[DeviceDescriptor],
    saved: Option<&str>,
    saved_label: Option<&str>,
    input: bool,
) -> ResolvedDevice {
    let Some(saved) = saved else {
        return automatic(devices, input);
    };
    if saved == SYSTEM_DEFAULT_DEVICE {
        return system_default(devices, input);
    }
    let found = devices
        .iter()
        .find(|device| device.id == saved && if input { device.input } else { device.output });
    if let Some(device) = found {
        return ResolvedDevice {
            row: device.id.clone(),
            open: Some(device.id.clone()),
            label: device.label.clone(),
            notice: None,
        };
    }
    let mut fallback = automatic(devices, input);
    // The identity can hold a serial number, so that a message names only the saved label.
    let gone = saved_label.unwrap_or("The saved device");
    fallback.notice = Some(if fallback.label.is_empty() {
        format!("{gone} is not connected.")
    } else {
        format!("{gone} is not connected. Using {} now.", fallback.label)
    });
    fallback
}

/// Maps a saved request to the devices that exist now. A saved choice always wins while its
/// device exists. Without that device, the same order as a first start applies.
#[must_use]
pub fn resolve_route(config: &DeviceConfig, devices: &[DeviceDescriptor]) -> ResolvedRoute {
    ResolvedRoute {
        input: resolve_device(
            devices,
            config.input_device.as_deref(),
            config.input_label.as_deref(),
            true,
        ),
        output: resolve_device(
            devices,
            config.output_device.as_deref(),
            config.output_label.as_deref(),
            false,
        ),
    }
}

/// The common rates that a device accepts, in ascending order.
#[must_use]
pub(super) fn common_rates(accepts: impl Fn(u32) -> bool) -> Vec<u32> {
    COMMON_SAMPLE_RATES
        .iter()
        .copied()
        .filter(|rate| accepts(*rate))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(id: &str, name: &str, detail: &str, input: bool, output: bool) -> RawDevice {
        RawDevice {
            id: id.to_owned(),
            name: name.to_owned(),
            detail: if detail.is_empty() {
                Vec::new()
            } else {
                vec![name.to_owned(), detail.to_owned()]
            },
            input,
            output,
            default_input: false,
            default_output: false,
        }
    }

    fn descriptor(id: &str, label: &str, input: bool, output: bool) -> DeviceDescriptor {
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

    /// The names that ALSA lists for one fake interface, with its card number and its card name.
    fn alsa_card() -> Vec<RawDevice> {
        let name = "Fake Scarlett, USB Audio";
        vec![
            raw("default", "Default Audio Device", "", true, true),
            raw(
                "sysdefault:CARD=Fake",
                name,
                "Default Audio Device",
                true,
                true,
            ),
            raw(
                "front:CARD=Fake,DEV=0",
                name,
                "Front output / input",
                true,
                true,
            ),
            raw(
                "surround51:CARD=Fake,DEV=0",
                name,
                "5.1 Surround",
                true,
                true,
            ),
            raw("iec958:CARD=Fake,DEV=0", name, "IEC958", true, true),
            raw(
                "dmix:CARD=Fake,DEV=0",
                name,
                "Direct sample mixing",
                true,
                true,
            ),
            raw(
                "dsnoop:CARD=Fake,DEV=0",
                name,
                "Direct sample snooping",
                true,
                true,
            ),
            raw("usbstream:CARD=Fake", name, "USB Stream Output", true, true),
            raw(
                "hw:CARD=Fake,DEV=0",
                name,
                "Direct hardware device",
                true,
                true,
            ),
            raw(
                "plughw:CARD=Fake,DEV=0",
                name,
                "All software conversions",
                true,
                true,
            ),
            raw(
                "hw:CARD=0,DEV=0",
                name,
                "Direct hardware device",
                true,
                false,
            ),
            raw(
                "plughw:CARD=0,DEV=0",
                name,
                "All software conversions",
                true,
                false,
            ),
            raw("null", "Discard all samples", "", true, true),
            raw("pipewire", "PipeWire Sound Server", "", true, true),
            raw("my_rack", "A rack of the user", "", true, true),
        ]
    }

    #[test]
    fn one_card_has_one_row_and_default_stays_for_the_first_row_of_the_page() {
        let clean = clean_alsa_devices(alsa_card());

        let listed: Vec<&str> = clean
            .iter()
            .filter(|device| device.listed)
            .map(|device| device.id.as_str())
            .collect();
        assert_eq!(listed, ["plughw:CARD=Fake,DEV=0", "my_rack"]);
        assert!(
            clean
                .iter()
                .any(|device| device.id == "default" && !device.listed),
            "the default device names the first row, and it has no second row"
        );
    }

    #[test]
    fn the_kept_row_takes_the_true_direction_from_the_kernel_row() {
        let clean = clean_alsa_devices(alsa_card());

        let card = clean
            .iter()
            .find(|device| device.id == "plughw:CARD=Fake,DEV=0")
            .expect("card");
        assert!(card.input);
        assert!(!card.output, "the hint said both, and the kernel said no");
    }

    #[test]
    fn a_direct_name_stays_only_with_no_conversion_row() {
        let clean = clean_alsa_devices(vec![raw(
            "hw:CARD=Only,DEV=0",
            "Only Card, USB",
            "",
            true,
            true,
        )]);
        assert_eq!(clean[0].id, "hw:CARD=Only,DEV=0");
    }

    #[test]
    fn two_rows_of_one_card_take_the_next_line_of_the_description() {
        let clean = clean_alsa_devices(vec![
            raw("plughw:CARD=Pc,DEV=0", "Pc, Audio", "Rear jack", true, true),
            raw(
                "plughw:CARD=Pc,DEV=1",
                "Pc, Audio",
                "Front jack",
                true,
                true,
            ),
        ]);

        assert_eq!(clean[0].label, "Pc, Audio, Rear jack");
        assert_eq!(clean[1].label, "Pc, Audio, Front jack");
    }

    #[test]
    fn two_rows_with_no_other_text_take_a_number() {
        let clean = clean_server_devices(vec![
            raw("node.a", "Same Name", "", true, false),
            raw("node.b", "Same Name", "", true, false),
        ]);

        assert_eq!(clean[0].label, "Same Name");
        assert_eq!(clean[1].label, "Same Name (2)");
    }

    #[test]
    fn a_sink_monitor_is_hidden_and_a_source_named_monitor_stays() {
        let clean = clean_server_devices(vec![
            raw(
                "alsa_output.fake.monitor",
                "Monitor of Fake",
                "",
                true,
                false,
            ),
            raw(
                "alsa_input.fake.source",
                "Fake Monitor Input 3+4",
                "",
                true,
                false,
            ),
        ]);

        assert!(!clean[0].listed);
        assert!(clean[1].listed);
    }

    #[test]
    fn a_first_start_takes_the_instrument_input() {
        let devices = [
            descriptor("a", "Webcam Microphone", true, false),
            descriptor("b", "Fake Scarlett Input 2 Mic", true, false),
            descriptor("c", "Fake Scarlett Input 1 Inst/Line", true, false),
        ];
        assert_eq!(automatic_input(&devices).expect("input").id, "c");
    }

    #[test]
    fn with_no_instrument_input_a_first_start_takes_an_interface_and_not_its_monitor() {
        let devices = [
            descriptor("a", "Webcam Microphone", true, false),
            descriptor("b", "Fake Scarlett Monitor Input 3+4", true, false),
            descriptor("c", "Fake Scarlett USB", true, false),
        ];
        assert_eq!(automatic_input(&devices).expect("input").id, "c");
    }

    #[test]
    fn a_first_start_with_no_interface_follows_the_system() {
        let mut webcam = descriptor("a", "Webcam Microphone", true, false);
        webcam.default_input = true;
        let config = DeviceConfig::default();
        let route = resolve_route(&config, &[webcam]);

        assert_eq!(route.input.row, SYSTEM_DEFAULT_DEVICE);
        assert_eq!(route.input.open, None);
        assert_eq!(route.input.label, "Webcam Microphone");
    }

    #[test]
    fn a_saved_choice_wins_over_an_instrument_input() {
        let devices = [
            descriptor("a", "Webcam Microphone", true, false),
            descriptor("c", "Fake Scarlett Input 1 Inst/Line", true, false),
        ];
        let config = DeviceConfig {
            input_device: Some("a".to_owned()),
            ..DeviceConfig::default()
        };

        let route = resolve_route(&config, &devices);

        assert_eq!(route.input.open.as_deref(), Some("a"));
        assert_eq!(route.input.notice, None);
    }

    #[test]
    fn a_saved_device_that_is_gone_falls_back_and_names_only_its_label() {
        let devices = [descriptor(
            "c",
            "Fake Scarlett Input 1 Inst/Line",
            true,
            false,
        )];
        let config = DeviceConfig {
            input_device: Some("alsa_input.usb-Fake_SERIAL123-00".to_owned()),
            input_label: Some("Old Interface".to_owned()),
            ..DeviceConfig::default()
        };

        let route = resolve_route(&config, &devices);

        assert_eq!(route.input.open.as_deref(), Some("c"));
        let notice = route.input.notice.expect("notice");
        assert!(notice.contains("Old Interface"), "{notice}");
        assert!(notice.contains("Inst/Line"), "{notice}");
        assert!(!notice.contains("SERIAL"), "{notice}");
    }

    #[test]
    fn a_saved_system_default_stays_the_system_default_with_an_interface_present() {
        let mut webcam = descriptor("a", "Webcam Microphone", true, false);
        webcam.default_input = true;
        let devices = [
            webcam,
            descriptor("c", "Fake Scarlett Input 1 Inst/Line", true, false),
        ];
        let config = DeviceConfig {
            input_device: Some(SYSTEM_DEFAULT_DEVICE.to_owned()),
            ..DeviceConfig::default()
        };

        let route = resolve_route(&config, &devices);

        assert_eq!(route.input.open, None);
        assert_eq!(route.input.row, SYSTEM_DEFAULT_DEVICE);
    }

    #[test]
    fn the_default_output_is_the_system_default() {
        let mut headset = descriptor("h", "Fake Headset", false, true);
        headset.default_output = true;
        let devices = [
            headset,
            descriptor("s", "Fake Scarlett Headphones", false, true),
        ];

        let route = resolve_route(&DeviceConfig::default(), &devices);

        assert_eq!(route.output.row, SYSTEM_DEFAULT_DEVICE);
        assert_eq!(route.output.open, None);
        assert_eq!(route.output.label, "Fake Headset");
    }

    #[test]
    fn a_virtual_device_has_at_most_two_channels_and_a_real_device_keeps_its_own() {
        assert_eq!(sane_channel_count(64, true), 2);
        assert_eq!(sane_channel_count(4, false), 4);
        assert_eq!(sane_channel_count(64, false), MAX_DEVICE_CHANNELS);
        assert_eq!(sane_channel_count(0, false), 1);
        assert!(alsa_device_is_virtual("default"));
        assert!(!alsa_device_is_virtual("plughw:CARD=Fake,DEV=0"));
    }

    #[test]
    fn the_rates_are_the_common_rates_that_the_device_accepts() {
        assert_eq!(
            common_rates(|rate| (44_100..=96_000).contains(&rate)),
            [44_100, 48_000, 88_200, 96_000]
        );
    }
}
