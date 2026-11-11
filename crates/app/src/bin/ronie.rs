//! The standalone app: the native UI, or a headless audio smoke run with `--smoke`.

// A release build on Windows opens no console window next to the UI.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::{ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use eyre::{WrapErr, eyre};
use ronie_app::{
    audio::{AudioBackendKind, AudioWorker, CpalBackend, DeviceConfig, DummyBackend},
    auth::configured_community,
    standalone_backend::{StandaloneBackend, standalone_processor},
};
use ronie_core::{PREFERENCES_SETTINGS, PresetStore, SettingsStore, UI_PREFERENCES_SETTINGS};
use ronie_ui::shell::NativeShellOptions;

fn main() -> eyre::Result<()> {
    let options = Options::parse(std::env::args().skip(1))?;
    match options.mode {
        Mode::Help => {
            println!("{}", help_text());
            Ok(())
        }
        Mode::Ui => run_ui(options),
        Mode::Smoke => run_smoke(&options),
    }
}

fn help_text() -> String {
    let drivers = format!("  B is {}.", AudioBackendKind::flag_values());
    [
        "Ronnie standalone",
        "  ronie [--ui] [--dummy | --backend B] [--selftest] [--capture PATH] [--report PATH] [--scale N]",
        "      Opens the window. This is the default.",
        "  ronie --smoke [--dummy | --backend B] [--seconds N]",
        "      Runs the audio with no window for N seconds (default 1), then stops.",
        "  ronie --help",
        &drivers,
    ]
    .join("\n")
}

fn run_ui(options: Options) -> eyre::Result<()> {
    let root = standalone_config_root();
    std::fs::create_dir_all(&root).wrap_err("creating standalone settings directory")?;
    let processor = standalone_processor(&root).map_err(|error| eyre!(error.to_string()))?;
    let session = processor.clone();
    let presets = PresetStore::new(root.join("presets"), root.join("system"))
        .list()
        .map_err(|error| eyre!(error.to_string()))?;
    let community = configured_community(&root).map_err(|error| eyre!(error.to_string()))?;
    let backend = if options.backend == Some(AudioBackendKind::Dummy) {
        StandaloneBackend::dummy(processor, &root.join(PREFERENCES_SETTINGS)).map(|backend| {
            backend.with_folder_opener(Arc::new(ronie_app::file_manager::open_in_file_manager))
        })
    } else {
        StandaloneBackend::real(processor, &root.join(PREFERENCES_SETTINGS), options.backend)
    }
    .map_err(|error| eyre!(error.to_string()))?;
    let shell = ronie_ui::shell::run_native(NativeShellOptions {
        selftest: options.selftest,
        capture: options.capture,
        report: options.report,
        scale: options.scale,
        backend: Some(Arc::new(backend)),
        presets,
        community,
        key_setup: Some(Box::new(ronie_app::auth::UserKeySetup::new(&root))),
        update_settings: Some(SettingsStore::standalone(&root, UI_PREFERENCES_SETTINGS)),
        ..NativeShellOptions::default()
    });
    // A compile can still hold the backend here, and the exit does not wait for it.
    if let Err(error) = session.save_at_exit() {
        eprintln!("Ronnie could not save the standalone session: {error}");
    }
    shell.map_err(|error| eyre!(error.to_string()))
}

fn run_smoke(options: &Options) -> eyre::Result<()> {
    let kind = options.backend.unwrap_or_default();
    let config = DeviceConfig {
        backend: kind,
        sample_rate: Some(48_000),
        buffer_frames: Some(ronie_app::audio::DEFAULT_BUFFER_FRAMES),
        ..DeviceConfig::default()
    };
    match kind {
        AudioBackendKind::Dummy => {
            let mut backend = DummyBackend::connected();
            let mut stream = backend
                .open_dummy(&config)
                .wrap_err("opening dummy audio")?;
            stream.start().wrap_err("starting dummy audio")?;
            stream.render().wrap_err("rendering dummy audio")?;
            std::thread::sleep(Duration::from_secs(options.seconds));
            stream.close().wrap_err("closing dummy audio")?;
        }
        AudioBackendKind::System | AudioBackendKind::Alsa | AudioBackendKind::Jack => {
            let worker = AudioWorker::new(&config).wrap_err("preparing audio worker")?;
            let mut backend = CpalBackend::new(kind);
            let _ = ronie_app::audio::AudioBackend::connect(&mut backend);
            let mut stream = backend
                .open(&config, worker)
                .wrap_err("opening audio device")?;
            stream.start().wrap_err("starting audio device")?;
            std::thread::sleep(Duration::from_secs(options.seconds));
            stream.close().wrap_err("closing audio device")?;
        }
    }
    println!("Ronnie audio smoke run completed");
    Ok(())
}

/// What one start of the binary does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The native window. This is the default.
    Ui,
    /// One headless audio run.
    Smoke,
    /// The help text.
    Help,
}

#[derive(Debug)]
struct Options {
    mode: Mode,
    /// The driver from the command line. Without it, the saved driver applies.
    backend: Option<AudioBackendKind>,
    seconds: u64,
    selftest: bool,
    capture: Option<PathBuf>,
    report: Option<PathBuf>,
    scale: Option<f32>,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = String>) -> eyre::Result<Self> {
        let mut backend = None;
        let mut seconds = None;
        let mut help = false;
        let mut ui = false;
        let mut smoke = false;
        // The first flag that only the window uses, for an error with `--smoke`.
        let mut window_flag = None;
        let mut selftest = false;
        let mut capture = None;
        let mut report = None;
        let mut scale = None;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--dummy" => backend = Some(AudioBackendKind::Dummy),
                "--backend" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| eyre!("--backend needs a value"))?;
                    backend = Some(
                        AudioBackendKind::from_flag(&value)
                            .map_err(|error| eyre!("--backend {value}: {error}"))?,
                    );
                }
                "--seconds" => {
                    seconds = Some(
                        arguments
                            .next()
                            .ok_or_else(|| eyre!("--seconds needs a value"))?
                            .parse()
                            .wrap_err("parsing --seconds")?,
                    );
                }
                "--ui" => ui = true,
                "--smoke" => smoke = true,
                "--selftest" => {
                    window_flag.get_or_insert("--selftest");
                    selftest = true;
                }
                "--capture" => {
                    window_flag.get_or_insert("--capture");
                    capture = Some(PathBuf::from(
                        arguments
                            .next()
                            .ok_or_else(|| eyre!("--capture needs a path"))?,
                    ));
                }
                "--report" => {
                    window_flag.get_or_insert("--report");
                    report = Some(PathBuf::from(
                        arguments
                            .next()
                            .ok_or_else(|| eyre!("--report needs a path"))?,
                    ));
                }
                "--scale" => {
                    window_flag.get_or_insert("--scale");
                    scale = Some(
                        arguments
                            .next()
                            .ok_or_else(|| eyre!("--scale needs a value"))?
                            .parse()
                            .wrap_err("parsing --scale")?,
                    );
                }
                "--help" | "-h" => help = true,
                _ => return Err(eyre!("unknown argument {argument}")),
            }
        }
        let mode = if help {
            Mode::Help
        } else if smoke {
            if ui {
                return Err(eyre!("--ui and --smoke cannot run together"));
            }
            if let Some(flag) = window_flag {
                return Err(eyre!("{flag} is only for the window, not for --smoke"));
            }
            Mode::Smoke
        } else {
            if seconds.is_some() {
                return Err(eyre!("--seconds is only for --smoke"));
            }
            Mode::Ui
        };
        Ok(Self {
            mode,
            backend,
            seconds: seconds.unwrap_or(1),
            selftest,
            capture,
            report,
            scale,
        })
    }
}

/// The settings directory of the standalone app on this operating system.
fn standalone_config_root() -> PathBuf {
    config_root(std::env::consts::OS, |name| std::env::var_os(name))
        .unwrap_or_else(|| std::env::temp_dir().join("ronie"))
}

/// The settings directory on `os`, as [`std::env::consts::OS`] names it, from the environment
/// that `var` reads. `None` means that the environment does not tell.
///
/// - Linux and other Unix systems: `$XDG_CONFIG_HOME/Ronie`, else `$HOME/.config/Ronie`.
/// - macOS: `$HOME/Library/Application Support/Ronie`.
/// - Windows: `%APPDATA%\Ronie`.
fn config_root(os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    match os {
        "macos" => var("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Ronie")
        }),
        "windows" => var("APPDATA").map(|folder| PathBuf::from(folder).join("Ronie")),
        _ => var("XDG_CONFIG_HOME")
            .map(|folder| PathBuf::from(folder).join("Ronie"))
            .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".config").join("Ronie"))),
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use super::{AudioBackendKind, Mode, Options, config_root, help_text};

    fn parse(arguments: &[&str]) -> eyre::Result<Options> {
        Options::parse(arguments.iter().map(|argument| (*argument).to_owned()))
    }

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn no_arguments_open_the_window() {
        let options = parse(&[]).expect("options");
        assert_eq!(options.mode, Mode::Ui);
        assert_eq!(options.backend, None);
        assert_eq!(parse(&["--ui"]).expect("options").mode, Mode::Ui);
        let dummy = parse(&["--dummy"]).expect("options");
        assert_eq!(dummy.mode, Mode::Ui);
        assert_eq!(dummy.backend, Some(AudioBackendKind::Dummy));
    }

    #[test]
    fn the_smoke_run_needs_its_own_flag() {
        let options = parse(&["--smoke", "--dummy", "--seconds", "0"]).expect("options");
        assert_eq!(options.mode, Mode::Smoke);
        assert_eq!(options.backend, Some(AudioBackendKind::Dummy));
        assert_eq!(options.seconds, 0);
        assert_eq!(parse(&["--smoke"]).expect("options").seconds, 1);
    }

    #[test]
    fn a_flag_of_the_other_mode_is_an_error() {
        for arguments in [
            &["--seconds", "0"][..],
            &["--ui", "--smoke"],
            &["--smoke", "--selftest"],
            &["--smoke", "--capture", "a.png"],
            &["--smoke", "--report", "a.json"],
            &["--smoke", "--scale", "2"],
        ] {
            assert!(parse(arguments).is_err(), "{arguments:?}");
        }
    }

    #[test]
    fn the_backend_flag_names_its_error() {
        let error = parse(&["--backend", "asio"]).expect_err("unknown backend");
        assert!(
            error.to_string().contains("unknown backend asio"),
            "{error}"
        );
        assert_eq!(
            parse(&["--backend", "system"]).expect("options").backend,
            Some(AudioBackendKind::System)
        );
    }

    #[test]
    fn the_help_names_the_smoke_run_and_the_drivers_of_this_system() {
        let text = help_text();
        assert!(text.contains("--smoke"), "{text}");
        assert!(text.contains(AudioBackendKind::flag_values()), "{text}");
        assert_eq!(parse(&["--help"]).expect("options").mode, Mode::Help);
    }

    #[test]
    fn linux_keeps_its_settings_in_the_xdg_folder() {
        let both = environment(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/a")]);
        assert_eq!(
            config_root("linux", both),
            Some(PathBuf::from("/xdg/Ronie"))
        );
        let home = environment(&[("HOME", "/home/a")]);
        assert_eq!(
            config_root("linux", home),
            Some(PathBuf::from("/home/a/.config/Ronie"))
        );
        assert_eq!(config_root("linux", environment(&[])), None);
    }

    #[test]
    fn macos_keeps_its_settings_in_application_support() {
        let variables = environment(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/Users/a")]);
        assert_eq!(
            config_root("macos", variables),
            Some(
                PathBuf::from("/Users/a")
                    .join("Library")
                    .join("Application Support")
                    .join("Ronie")
            )
        );
        assert_eq!(config_root("macos", environment(&[])), None);
    }

    #[test]
    fn windows_keeps_its_settings_in_the_roaming_app_data() {
        let variables = environment(&[
            ("APPDATA", r"C:\Users\a\AppData\Roaming"),
            ("HOME", "/home/a"),
        ]);
        assert_eq!(
            config_root("windows", variables),
            Some(PathBuf::from(r"C:\Users\a\AppData\Roaming").join("Ronie"))
        );
        let no_app_data = environment(&[("HOME", "/home/a")]);
        assert_eq!(config_root("windows", no_app_data), None);
    }
}
