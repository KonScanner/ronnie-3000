//! The standalone app: the native UI, or a headless audio smoke run with `--smoke`.

// A release build on Windows opens no console window next to the UI.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::{ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use eyre::{WrapErr, eyre};
use ronnie_app::{
    audio::{AudioBackendKind, AudioWorker, CpalBackend, DeviceConfig, DummyBackend},
    auth::configured_community,
    diagnostics::DiagnosticLog,
    standalone_backend::{StandaloneBackend, standalone_processor},
};
use ronnie_core::{PREFERENCES_SETTINGS, PresetStore, SettingsStore, UI_PREFERENCES_SETTINGS};
use ronnie_ui::{
    services::{GithubReleaseFeed, ReleaseFeed},
    shell::NativeShellOptions,
};

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
        "  ronnie [--ui] [--dummy | --backend B] [--selftest] [--capture PATH] [--report PATH] [--scale N]",
        "        [--layout stage | reference]",
        "      Opens the window. This is the default. The window opens the Stage layout;",
        "      --layout reference opens the classic layout.",
        "  ronnie --smoke [--dummy | --backend B] [--seconds N]",
        "      Runs the audio with no window for N seconds (default 1), then stops.",
        "  ronnie --help",
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
            backend.with_folder_opener(Arc::new(ronnie_app::file_manager::open_in_file_manager))
        })
    } else {
        StandaloneBackend::real(processor, &root.join(PREFERENCES_SETTINGS), options.backend)
    }
    .map_err(|error| eyre!(error.to_string()))?;
    // A self-test or a capture must give the same result each time, so it stays offline.
    let release_feed = (!options.selftest && options.capture.is_none())
        .then(|| release_feed(backend.log_handle()))
        .flatten();
    let shell = ronnie_ui::shell::run_native(NativeShellOptions {
        selftest: options.selftest,
        capture: options.capture,
        report: options.report,
        scale: options.scale,
        view_mode: options.view_mode,
        backend: Some(Arc::new(backend)),
        presets,
        community,
        key_setup: Some(Box::new(ronnie_app::auth::UserKeySetup::new(&root))),
        update_settings: Some(SettingsStore::standalone(&root, UI_PREFERENCES_SETTINGS)),
        release_feed,
        ..NativeShellOptions::default()
    });
    // A compile can still hold the backend here, and the exit does not wait for it.
    if let Err(error) = session.save_at_exit() {
        eprintln!("Ronnie could not save the standalone session: {error}");
    }
    shell.map_err(|error| eyre!(error.to_string()))
}

/// The daily release check of this app, which asks only the GitHub API. A failure writes one
/// line to the diagnostic log. With no HTTP client, the app does not check.
fn release_feed(log: DiagnosticLog) -> Option<Arc<dyn ReleaseFeed>> {
    match GithubReleaseFeed::github() {
        Ok(feed) => Some(Arc::new(
            feed.with_log(Arc::new(move |line: &str| log.write(line))),
        )),
        Err(error) => {
            log.write(&format!("The update check is not available: {error}."));
            None
        }
    }
}

fn run_smoke(options: &Options) -> eyre::Result<()> {
    let kind = options.backend.unwrap_or_default();
    let config = DeviceConfig {
        backend: kind,
        sample_rate: Some(48_000),
        buffer_frames: Some(ronnie_app::audio::DEFAULT_BUFFER_FRAMES),
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
            let _ = ronnie_app::audio::AudioBackend::connect(&mut backend);
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
    view_mode: ronnie_ui::shell::ViewMode,
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
        let mut view_mode = ronnie_ui::shell::ViewMode::RonnieFlow;
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
                "--layout" => {
                    window_flag.get_or_insert("--layout");
                    view_mode = match arguments
                        .next()
                        .ok_or_else(|| eyre!("--layout needs stage or reference"))?
                        .as_str()
                    {
                        "stage" => ronnie_ui::shell::ViewMode::RonnieFlow,
                        "reference" => ronnie_ui::shell::ViewMode::ReferenceDefault,
                        other => {
                            return Err(eyre!("unknown layout {other}: use stage or reference"));
                        }
                    };
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
            view_mode,
        })
    }
}

/// The settings directory of the standalone app on this operating system.
fn standalone_config_root() -> PathBuf {
    let root = config_root(std::env::consts::OS, |name| std::env::var_os(name))
        .unwrap_or_else(|| std::env::temp_dir().join("ronnie"));
    adopt_legacy_root(root)
}

/// The name of the settings directory before the app was renamed to Ronnie.
const LEGACY_FOLDER: &str = "Ronie";

/// Moves the settings directory of an old version, `Ronie`, to `root` when `root` does not
/// exist yet. The presets and the settings then stay with the user. If the move fails, the
/// old directory stays the settings directory, so that no data is lost.
fn adopt_legacy_root(root: PathBuf) -> PathBuf {
    let Some(legacy) = root.parent().map(|parent| parent.join(LEGACY_FOLDER)) else {
        return root;
    };
    if root.exists() || !legacy.is_dir() {
        return root;
    }
    match std::fs::rename(&legacy, &root) {
        Ok(()) => root,
        Err(error) => {
            eprintln!(
                "Ronnie could not move {} to {}: {error}. Ronnie uses the old folder.",
                legacy.display(),
                root.display()
            );
            legacy
        }
    }
}

/// The settings directory on `os`, as [`std::env::consts::OS`] names it, from the environment
/// that `var` reads. `None` means that the environment does not tell.
///
/// - Linux and other Unix systems: `$XDG_CONFIG_HOME/Ronnie`, else `$HOME/.config/Ronnie`.
/// - macOS: `$HOME/Library/Application Support/Ronnie`.
/// - Windows: `%APPDATA%\Ronnie`.
fn config_root(os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    match os {
        "macos" => var("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Ronnie")
        }),
        "windows" => var("APPDATA").map(|folder| PathBuf::from(folder).join("Ronnie")),
        _ => var("XDG_CONFIG_HOME")
            .map(|folder| PathBuf::from(folder).join("Ronnie"))
            .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".config").join("Ronnie"))),
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use super::{AudioBackendKind, Mode, Options, adopt_legacy_root, config_root, help_text};

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
    fn the_layout_flag_chooses_the_view_mode() {
        use ronnie_ui::shell::ViewMode;
        assert_eq!(parse(&[]).expect("options").view_mode, ViewMode::RonnieFlow);
        let stage = parse(&["--layout", "stage"]).expect("options");
        assert_eq!(stage.view_mode, ViewMode::RonnieFlow);
        let reference = parse(&["--layout", "reference"]).expect("options");
        assert_eq!(reference.view_mode, ViewMode::ReferenceDefault);
        assert!(parse(&["--layout", "nope"]).is_err());
        assert!(parse(&["--layout"]).is_err());
        assert!(parse(&["--smoke", "--layout", "stage"]).is_err());
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
            Some(PathBuf::from("/xdg/Ronnie"))
        );
        let home = environment(&[("HOME", "/home/a")]);
        assert_eq!(
            config_root("linux", home),
            Some(PathBuf::from("/home/a/.config/Ronnie"))
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
                    .join("Ronnie")
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
            Some(PathBuf::from(r"C:\Users\a\AppData\Roaming").join("Ronnie"))
        );
        let no_app_data = environment(&[("HOME", "/home/a")]);
        assert_eq!(config_root("windows", no_app_data), None);
    }

    #[test]
    fn the_settings_of_an_old_version_move_to_the_new_folder() {
        let parent = std::env::temp_dir().join(format!("ronnie-legacy-{}", std::process::id()));
        let legacy = parent.join("Ronie");
        let root = parent.join("Ronnie");
        std::fs::create_dir_all(legacy.join("presets")).expect("legacy folder");
        std::fs::write(legacy.join("preferences.settings"), "{}").expect("legacy settings");

        assert_eq!(adopt_legacy_root(root.clone()), root);
        assert!(root.join("presets").is_dir());
        assert!(root.join("preferences.settings").is_file());
        assert!(!legacy.exists());

        // A second start finds the new folder and does not touch a new old folder.
        std::fs::create_dir_all(&legacy).expect("second legacy folder");
        assert_eq!(adopt_legacy_root(root.clone()), root);
        assert!(legacy.is_dir());

        std::fs::remove_dir_all(&parent).expect("cleanup");
    }

    #[test]
    fn no_old_folder_keeps_the_new_folder() {
        let root = std::env::temp_dir()
            .join(format!("ronnie-no-legacy-{}", std::process::id()))
            .join("Ronnie");
        assert_eq!(adopt_legacy_root(root.clone()), root);
        assert!(!root.exists());
    }
}
