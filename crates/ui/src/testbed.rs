//! Scenario parsing and backend-effect verification. Clicks run through the real UI.

use serde_json::Value;

use crate::{
    backend::{MockBackend, UiBackend, UiDrive, UiEffect, UiError},
    driver::UiDriver,
};

/// Official scenario sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiSection {
    /// Main chain, detail, stereo, and tuner views.
    Main,
    /// Model loading, OAuth, offline, and update views.
    Loading,
    /// Device and system banners.
    Banners,
    /// Tone discovery and taxonomy views.
    Browser,
    /// Plugin and standalone settings.
    Settings,
    /// Header, presets, decks, and global feedback.
    Chrome,
}

impl UiSection {
    fn parse(value: &str) -> Result<Self, UiError> {
        match value {
            "main" => Ok(Self::Main),
            "loading" => Ok(Self::Loading),
            "banners" => Ok(Self::Banners),
            "browser" => Ok(Self::Browser),
            "settings" => Ok(Self::Settings),
            "chrome" => Ok(Self::Chrome),
            other => Err(UiError::InvalidAction(format!(
                "unknown scenario section: {other}"
            ))),
        }
    }
}

/// Capture viewport independent of a rendering toolkit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiViewport {
    /// Width in logical points.
    pub width: u32,
    /// Height in logical points.
    pub height: u32,
    /// Physical pixels per logical point.
    pub scale: f32,
}

impl Default for UiViewport {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 578,
            scale: 2.0,
        }
    }
}

/// One neutral scenario and its interactions.
#[derive(Debug, Clone)]
pub struct UiScenario {
    /// Stable scenario identity.
    pub id: String,
    /// Official section.
    pub section: UiSection,
    /// Neutral state and metadata.
    pub state: Value,
    /// Ordered user interactions.
    pub drives: Vec<UiDrive>,
    /// Capture viewport.
    pub viewport: UiViewport,
}

/// Successful scenario verification summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioReport {
    /// Whether every required backend effect was observed.
    pub effects_verified: bool,
}

/// Aggregate selftest summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestbedReport {
    /// Number of passing scenarios.
    pub passed: usize,
    /// Number of failing scenarios.
    pub failed: usize,
    /// Scenario identities that failed, because the real UI cannot finish them yet.
    pub blocked: Vec<String>,
}

/// Fixtures that the real UI cannot finish yet, with the fault that blocks each one.
///
/// [`run`] returns an error for them, and [`run_all`] counts them as failed. A blocked fixture
/// that starts to pass is an error, so this list cannot keep a stale entry.
pub const BLOCKED_FIXTURES: &[(&str, &str)] = &[];

/// Parses all neutral scenarios with strict IDs, sections, drives, and viewports.
///
/// # Errors
/// Returns a JSON or schema error for malformed input.
pub fn load_scenarios(input: &str) -> Result<Vec<UiScenario>, UiError> {
    let root: Value =
        serde_json::from_str(input).map_err(|error| UiError::Control(error.to_string()))?;
    let entries = root["scenarios"]
        .as_array()
        .ok_or_else(|| UiError::InvalidAction("scenarios must be an array".to_owned()))?;
    entries.iter().map(parse_scenario).collect()
}

fn parse_scenario(value: &Value) -> Result<UiScenario, UiError> {
    let id = value["id"]
        .as_str()
        .ok_or_else(|| UiError::InvalidAction("scenario ID is missing".to_owned()))?;
    let section = UiSection::parse(value["section"].as_str().unwrap_or_default())?;
    let drives = value["drives"].as_array().map_or_else(
        || Ok(Vec::new()),
        |drives| drives.iter().map(parse_drive).collect(),
    )?;
    let viewport = UiViewport {
        width: value["viewport"]["width"].as_u64().unwrap_or(1024) as u32,
        height: value["viewport"]["height"].as_u64().unwrap_or(578) as u32,
        scale: value["viewport"]["scale"].as_f64().unwrap_or(2.0) as f32,
    };
    Ok(UiScenario {
        id: id.to_owned(),
        section,
        state: value["state"].clone(),
        drives,
        viewport,
    })
}

fn parse_drive(value: &Value) -> Result<UiDrive, UiError> {
    let kind = value["kind"].as_str().unwrap_or_default();
    let target = value["target"].as_str().unwrap_or_default().to_owned();
    let text = value["text"].as_str().unwrap_or_default().to_owned();
    match kind {
        "wait" => Ok(UiDrive::Wait {
            ms: value["ms"].as_u64().unwrap_or(0),
        }),
        "click" => Ok(UiDrive::Click { target }),
        "fill" => Ok(UiDrive::Fill { target, text }),
        "submit" => Ok(UiDrive::Submit { target, text }),
        "scroll" => Ok(UiDrive::Scroll {
            target,
            position: value["position"].as_f64().unwrap_or(0.0) as f32,
        }),
        "openSettings" => Ok(UiDrive::OpenSettings),
        "openSystemSettings" => Ok(UiDrive::OpenSystemSettings),
        "scrollSettings" => Ok(UiDrive::ScrollSettings { heading: target }),
        "unhover" => Ok(UiDrive::Unhover),
        "hover" => Ok(UiDrive::Hover { target }),
        "clearFilter" => Ok(UiDrive::ClearFilter { target }),
        _ => Err(UiError::InvalidAction(format!(
            "unknown drive kind: {kind}"
        ))),
    }
}

/// Runs one scenario against a fresh mock backend.
///
/// # Errors
/// Returns unknown targets, invalid drives, or missing backend effects.
pub fn run(scenario: &UiScenario) -> Result<ScenarioReport, UiError> {
    run_with_backend(scenario, MockBackend::from_scenario(scenario))
}

/// Runs one scenario against a caller-selected mock, preserving its event log.
///
/// A click runs through the real Default UI: the control must exist under its accessible
/// label and must take real pointer input. Other drives that a later click depends on also
/// reach the real UI.
///
/// # Errors
/// Returns unknown targets, invalid drives, controls that the real UI does not offer, or
/// missing backend effects.
pub fn run_with_backend(
    scenario: &UiScenario,
    backend: MockBackend,
) -> Result<ScenarioReport, UiError> {
    let _ = drive_scenario(scenario, backend)?;
    Ok(ScenarioReport {
        effects_verified: true,
    })
}

/// Plays a scenario's drives and returns the real UI at the state that real input reached.
/// The result is `None` for a scenario with no click, because it starts no real UI.
///
/// # Errors
/// Returns unknown targets, invalid drives, controls that the real UI does not offer, or
/// missing backend effects.
pub fn drive_scenario(
    scenario: &UiScenario,
    backend: MockBackend,
) -> Result<Option<UiDriver>, UiError> {
    let mut driver = start_driver(scenario, &backend)?;
    for drive in &scenario.drives {
        match drive {
            UiDrive::Wait { .. } => {
                if let Some(driver) = &mut driver {
                    driver.settle();
                }
            }
            UiDrive::Unhover => {
                if let Some(driver) = &mut driver {
                    driver.unhover();
                }
            }
            UiDrive::Click { target } => {
                scroll_into_view(&backend, target);
                let plan = click_plan(target)?;
                let Some(driver) = &mut driver else {
                    return Err(drive_failed(
                        scenario,
                        target,
                        "no real UI to click".to_owned(),
                    ));
                };
                click_for_real(driver, scenario, target, &plan)?;
            }
            UiDrive::Fill { target, text } => {
                if target != "Name" {
                    return Err(UiError::UnknownTarget(target.clone()));
                }
                scroll_into_view(&backend, target);
                if let Some(driver) = &mut driver {
                    let _ = driver
                        .fill(target, text)
                        .map_err(|error| drive_failed(scenario, target, error.to_string()))?;
                }
            }
            UiDrive::Submit { target, .. } => {
                if target != "Search…" {
                    return Err(UiError::UnknownTarget(target.clone()));
                }
                scroll_into_view(&backend, target);
            }
            UiDrive::Scroll { target, position } => {
                if !matches!(target.as_str(), "browser" | "vertical" | "settings") {
                    return Err(UiError::UnknownTarget(target.clone()));
                }
                backend.record_scroll(target, *position);
            }
            UiDrive::OpenSettings | UiDrive::OpenSystemSettings => {
                // A scenario with clicks starts on this page, because no fixture input opens it.
                if let Some(driver) = &driver
                    && driver.chrome().settings().is_none()
                {
                    return Err(drive_failed(
                        scenario,
                        "settings",
                        "the settings page is not open".to_owned(),
                    ));
                }
            }
            UiDrive::ScrollSettings { heading } => {
                if !matches!(
                    heading.as_str(),
                    "Calibration" | "MIDI Mapping" | "Presets" | "MIDI Inputs" | "Community"
                ) {
                    return Err(UiError::UnknownTarget(heading.clone()));
                }
                backend.record_scroll(heading, 0.0);
                if let Some(driver) = &mut driver {
                    if let Some(settings) = driver.chrome_mut().settings_mut() {
                        settings.scroll_to_heading(heading);
                    }
                    driver.settle();
                }
            }
            UiDrive::Hover { target } => {
                if !matches!(target.as_str(), "input meter" | "AMS Neve 88R LB") {
                    return Err(UiError::UnknownTarget(target.clone()));
                }
                scroll_into_view(&backend, target);
            }
            UiDrive::ClearFilter { target } => {
                scroll_into_view(&backend, target);
            }
        }
    }
    let _ = backend.chain_state(0);
    let _ = backend.meter_levels();
    let _ = backend.parameter(ronie_core::params::ParameterId("inputLevel"));
    Ok(driver)
}

/// Builds the real UI for a scenario with clicks. Every click target must be known first, so a
/// wrong fixture fails before any frame runs.
fn start_driver(scenario: &UiScenario, backend: &MockBackend) -> Result<Option<UiDriver>, UiError> {
    let mut has_click = false;
    for drive in &scenario.drives {
        if let UiDrive::Click { target } = drive {
            has_click = true;
            let _ = click_plan(target)?;
        }
    }
    if !has_click {
        return Ok(None);
    }
    crate::native_scenario::driver_for_scenario(scenario, backend.clone())
        .map(Some)
        .map_err(UiError::InvalidAction)
}

fn drive_failed(scenario: &UiScenario, target: &str, reason: String) -> UiError {
    UiError::DriveFailed {
        scenario: scenario.id.clone(),
        target: target.to_owned(),
        reason,
    }
}

/// Plays one real click and proves that it had the effect that the fixture requires.
fn click_for_real(
    driver: &mut UiDriver,
    scenario: &UiScenario,
    target: &str,
    plan: &ClickPlan,
) -> Result<(), UiError> {
    let label = plan
        .labels
        .iter()
        .find_map(|label| {
            if plan.prefix {
                driver.label_starting_with(label)
            } else {
                driver.is_visible(label).then(|| (*label).to_owned())
            }
        })
        .unwrap_or_else(|| plan.labels[0].to_owned());
    let before = visible_state(driver);
    let played = if plan.secondary {
        driver.secondary_click(&label)
    } else if plan.first_of_many {
        driver.click_first(&label)
    } else {
        driver.click(&label)
    };
    let interaction = played.map_err(|error| drive_failed(scenario, target, error.to_string()))?;
    let changed = visible_state(driver) != before;
    match plan.proof {
        ClickProof::Backend(expected) if !interaction.effects.contains(&expected) => {
            Err(UiError::MissingBackendEffect {
                expected,
                actual: interaction
                    .effects
                    .last()
                    .copied()
                    .unwrap_or(UiEffect::None),
            })
        }
        ClickProof::Visible if !changed => Err(drive_failed(
            scenario,
            target,
            "the click changed nothing on the screen".to_owned(),
        )),
        ClickProof::Backend(_) | ClickProof::Visible => Ok(()),
    }
}

fn visible_state(driver: &UiDriver) -> String {
    let chrome = driver.chrome();
    format!(
        "{:?} {:?} {:?} {:?} {} {} {} {} {}",
        driver.visible_labels(),
        chrome.overlay(),
        chrome
            .browser()
            .map(|browser| browser.store().query().clone()),
        crate::default::popover::open_rects(driver.context()),
        chrome.settings().is_some(),
        chrome.tuner_visible(),
        chrome.measuring(),
        chrome.auth_active(),
        chrome.session().signed_in(),
    )
}

/// Runs every scenario and returns aggregate counts. Only a fixture in [`BLOCKED_FIXTURES`]
/// can fail without an error, and the report names it.
///
/// # Errors
/// Returns the first error of a scenario that is not blocked, or an error for a blocked
/// scenario that now passes.
pub fn run_all(scenarios: &[UiScenario]) -> Result<TestbedReport, UiError> {
    let mut passed = 0;
    let mut blocked = Vec::new();
    for scenario in scenarios {
        let is_blocked = BLOCKED_FIXTURES
            .iter()
            .any(|(id, _)| *id == scenario.id.as_str());
        match run(scenario) {
            Ok(_) if is_blocked => {
                return Err(UiError::InvalidAction(format!(
                    "{} now passes, so remove it from BLOCKED_FIXTURES",
                    scenario.id
                )));
            }
            Ok(_) => passed += 1,
            Err(_) if is_blocked => blocked.push(scenario.id.clone()),
            Err(error) => return Err(error),
        }
    }
    Ok(TestbedReport {
        passed,
        failed: blocked.len(),
        blocked,
    })
}

fn scroll_into_view(backend: &MockBackend, target: &str) {
    backend.record_scroll(target, 0.0);
}

/// What proves that a click worked.
#[derive(Clone, Copy)]
enum ClickProof {
    /// The backend owns the change, so this effect must reach it.
    Backend(UiEffect),
    /// The UI owns the change, so a change on the screen proves it.
    Visible,
}

/// How one fixture click target reaches the real UI.
struct ClickPlan {
    /// Accessible labels to look for, in order. A label can carry a current value.
    labels: &'static [&'static str],
    /// Whether a label is the start of the accessible label.
    prefix: bool,
    /// Whether the fixture means the secondary button.
    secondary: bool,
    /// Whether several equal controls have this label, and any of them will do.
    first_of_many: bool,
    /// What the click must do.
    proof: ClickProof,
}

impl ClickPlan {
    const fn new(labels: &'static [&'static str], proof: ClickProof) -> Self {
        Self {
            labels,
            prefix: false,
            secondary: false,
            first_of_many: false,
            proof,
        }
    }

    const fn knob_deck(labels: &'static [&'static str]) -> Self {
        Self {
            labels,
            prefix: true,
            secondary: true,
            first_of_many: false,
            proof: ClickProof::Visible,
        }
    }
}

/// Maps a fixture click target to the real control that a user clicks.
fn click_plan(target: &str) -> Result<ClickPlan, UiError> {
    use ClickProof::Visible;
    Ok(match target {
        "Account:" => ClickPlan::new(&["Account"], Visible),
        "Save Preset" => ClickPlan::new(&["Save preset"], Visible),
        "Save" => ClickPlan::new(&["Save"], ClickProof::Backend(UiEffect::PresetSaved)),
        "Presets:" => ClickPlan::new(&["Browse presets"], Visible),
        "Auto Balance" => ClickPlan::new(&["Auto balance"], Visible),
        "Input Mode" => ClickPlan::new(&["Input mode"], Visible),
        "Gate:" => ClickPlan::knob_deck(&["Gate:"]),
        "Pitch Shift:" => ClickPlan::knob_deck(&["Pitch:"]),
        "Offset:" => ClickPlan::knob_deck(&["Offset:", "Enable Spread"]),
        "Align:" => ClickPlan::knob_deck(&["Offset:", "Enable Align"]),
        "Info:" => ClickPlan::new(&["Tone info"], Visible),
        "EQ:" => ClickPlan::new(&["EQ"], Visible),
        "Curve:" => ClickPlan::new(&["Curve"], Visible),
        "Tuner:" => ClickPlan::new(&["Tuner"], Visible),
        "Add Tone" => ClickPlan {
            first_of_many: true,
            ..ClickPlan::new(&["Add Tone"], Visible)
        },
        "Login" => ClickPlan::new(&["Sign in"], Visible),
        "Copy Link" => ClickPlan::new(&["Copy Link"], Visible),
        "Sign in on a different device" => {
            ClickPlan::new(&["Sign in on a different device"], Visible)
        }
        "Pedal" => ClickPlan::new(&["Pedal"], Visible),
        "Spaces" => ClickPlan::new(&["Spaces"], Visible),
        "Cabinet" => ClickPlan::new(&["Cabinet"], Visible),
        "Verified:" => ClickPlan::new(&["Verified"], Visible),
        "Sign in or create free account" => {
            ClickPlan::new(&["Sign in or create free account"], Visible)
        }
        "Profile:" => ClickPlan::new(&["Profile"], Visible),
        "Recently used" => ClickPlan::new(&["Recently used"], Visible),
        "Favorites" => ClickPlan::new(&["Favorites"], Visible),
        "Filters:" | "Fewer filters:" => ClickPlan::new(&["Filters"], Visible),
        "Sort:" => ClickPlan::new(&["Trending", "Best match"], Visible),
        "Newest" => ClickPlan::new(&["Newest"], Visible),
        "Impulse Response (IR)" => ClickPlan::new(&["Impulse Response (IR)"], Visible),
        "Tags:" => ClickPlan::new(&["Tags"], Visible),
        "rock" => ClickPlan::new(&["rock"], Visible),
        "Creators:" => ClickPlan::new(&["Creators"], Visible),
        "Format:" => ClickPlan::new(&["Format", "IR", "NAM"], Visible),
        "Close:" => ClickPlan::new(&["Close tone browser"], Visible),
        "Control to map" => ClickPlan::new(&["Control to map"], Visible),
        "model select" => ClickPlan::new(&["Select model"], Visible),
        "'02 Vox AC30/6 Top Boost" => ClickPlan::new(&["'02 Vox AC30/6 Top Boost"], Visible),
        "'02 Vox AC30/6 Top Boost." => ClickPlan {
            labels: &["'02 Vox AC30/6 Top Boost"],
            prefix: false,
            secondary: true,
            first_of_many: false,
            proof: Visible,
        },
        _ => return Err(UiError::UnknownTarget(target.to_owned())),
    })
}

/// Converts official scenario IDs into neutral declarative drives for the exporter.
#[must_use]
pub fn official_drives(id: &str) -> Vec<UiDrive> {
    let click = |target: &str| UiDrive::Click {
        target: target.to_owned(),
    };
    let wait = |ms| UiDrive::Wait { ms };
    let unhover = || UiDrive::Unhover;
    let submit = |text: &str| UiDrive::Submit {
        target: "Search…".to_owned(),
        text: text.to_owned(),
    };
    let settings = || UiDrive::OpenSettings;
    let system = || UiDrive::OpenSystemSettings;
    let scroll_settings = |heading: &str| UiDrive::ScrollSettings {
        heading: heading.to_owned(),
    };
    let filters = || click("Filters:");
    let sort = || click("Sort:");
    let profile = || click("Profile:");
    match id {
        "chrome-account-signed-out" | "chrome-account-signed-in" => {
            vec![click("Account:"), wait(200)]
        }
        "chrome-preset-save" => vec![
            click("Save Preset"),
            UiDrive::Fill {
                target: "Name".to_owned(),
                text: "Stadium Lead".to_owned(),
            },
            wait(200),
        ],
        "chrome-preset-browse" => vec![click("Presets:"), wait(300)],
        "chrome-auto-balance" => vec![click("Auto Balance"), wait(100)],
        "chrome-input-mode" => vec![click("Input Mode"), wait(200)],
        "main-meters-hot" => vec![
            wait(150),
            UiDrive::Hover {
                target: "input meter".to_owned(),
            },
            wait(100),
        ],
        "chrome-gate-deck" => vec![click("Gate:"), wait(200)],
        "chrome-pitch-deck" => vec![click("Pitch Shift:"), wait(200)],
        "chrome-spread-deck" => vec![click("Offset:"), wait(200)],
        "chrome-align-deck" => vec![click("Align:"), wait(200)],
        "chrome-tile-menu" => vec![click("'02 Vox AC30/6 Top Boost."), wait(200)],
        "main-detail" | "load-detail-loading" | "load-detail-failed" => vec![wait(300)],
        "main-detail-info" | "main-detail-info-signed-out" => vec![click("Info:"), wait(400)],
        "main-detail-eq-sliders" => vec![click("EQ:"), wait(300)],
        "main-detail-eq-curve" => vec![click("EQ:"), wait(200), click("Curve:"), wait(400)],
        "main-detail-model-select" => vec![wait(200), click("model select"), wait(400)],
        "main-tuner-idle" => vec![click("Tuner:"), wait(200)],
        "main-tuner-intune" | "main-tuner-flat" | "main-tuner-sharp" | "main-tuner-accidental" => {
            vec![click("Tuner:"), wait(400)]
        }
        "load-offline-modal" => vec![click("Add Tone"), wait(300), unhover()],
        "load-update-notice" => vec![wait(300)],
        "load-oauth-leaving" | "load-oauth-browser-failed" => {
            vec![
                click("Account:"),
                wait(100),
                click("Login"),
                wait(300),
                unhover(),
            ]
        }
        "load-oauth-copied" => vec![
            click("Account:"),
            wait(100),
            click("Login"),
            wait(300),
            click("Copy Link"),
            wait(200),
            unhover(),
        ],
        "load-oauth-device-requesting" => vec![
            click("Account:"),
            wait(100),
            click("Login"),
            wait(300),
            click("Sign in on a different device"),
            wait(300),
            unhover(),
        ],
        "load-oauth-device" | "load-oauth-device-expired" => vec![
            click("Account:"),
            wait(100),
            click("Login"),
            wait(300),
            click("Sign in on a different device"),
            wait(400),
            unhover(),
        ],
        "browser-picking" => vec![click("'02 Vox AC30/6 Top Boost"), wait(600), unhover()],
        "browser-scrolled" => vec![
            UiDrive::Scroll {
                target: "browser".to_owned(),
                position: 100.0,
            },
            wait(100),
        ],
        "browser-search-typed" => vec![submit("vox"), wait(400), unhover()],
        "browser-gear" | "browser-signed-out-gear" => vec![click("Pedal"), wait(400)],
        "browser-verified" => vec![click("Verified:"), wait(400)],
        "browser-signed-out-empty" => vec![click("Spaces"), wait(400), unhover()],
        "browser-signed-out-footer" => vec![
            UiDrive::Scroll {
                target: "browser".to_owned(),
                position: 10_000.0,
            },
            wait(300),
        ],
        "browser-signed-out-signed-in" => vec![
            click("Sign in or create free account"),
            wait(500),
            unhover(),
        ],
        "browser-signed-out-card" => vec![click("'02 Vox AC30/6 Top Boost"), wait(400), unhover()],
        "browser-signed-out-signing-in" => vec![
            click("'02 Vox AC30/6 Top Boost"),
            wait(300),
            click("Sign in or create free account"),
            wait(300),
            unhover(),
        ],
        "browser-profile-menu" => vec![profile(), wait(300)],
        "browser-profile-recent" => vec![
            profile(),
            wait(200),
            click("Recently used"),
            wait(400),
            unhover(),
        ],
        "browser-profile-search" => vec![
            profile(),
            wait(200),
            click("Recently used"),
            wait(300),
            submit("vox"),
            wait(400),
            unhover(),
        ],
        "browser-profile-empty" => vec![
            profile(),
            wait(200),
            click("Favorites"),
            wait(400),
            unhover(),
        ],
        "browser-filters-expanded" => vec![filters(), wait(300), unhover()],
        "browser-sort-menu" | "browser-zoom-menu" => vec![filters(), wait(200), sort(), wait(300)],
        "browser-sort-cleared" => vec![
            filters(),
            wait(200),
            sort(),
            wait(200),
            click("Newest"),
            wait(300),
            click("Newest"),
            wait(200),
            UiDrive::ClearFilter {
                target: "Newest".to_owned(),
            },
            wait(300),
        ],
        "browser-sort-newest" => vec![
            filters(),
            wait(200),
            sort(),
            wait(200),
            click("Newest"),
            wait(400),
            unhover(),
        ],
        "browser-calibrated-ir" => {
            vec![click("Cabinet"), wait(300), filters(), wait(400), unhover()]
        }
        "browser-format-ir" => vec![
            filters(),
            wait(200),
            click("Format:"),
            wait(200),
            click("Impulse Response (IR)"),
            wait(400),
            unhover(),
        ],
        "browser-tags-menu" => vec![filters(), wait(200), click("Tags:"), wait(400)],
        "browser-tags-picked" => vec![
            filters(),
            wait(200),
            click("Tags:"),
            wait(300),
            click("rock"),
            wait(300),
            click("rock"),
            wait(400),
        ],
        "browser-creators-menu" => vec![filters(), wait(200), click("Creators:"), wait(400)],
        "browser-format-menu" => vec![filters(), wait(200), click("Format:"), wait(300)],
        "browser-return" => vec![
            click("Pedal"),
            wait(400),
            click("Close:"),
            wait(300),
            click("Add Tone"),
            wait(300),
            unhover(),
        ],
        "browser-filters-active-collapsed" => vec![
            filters(),
            wait(200),
            click("Format:"),
            wait(200),
            click("Impulse Response (IR)"),
            wait(300),
            click("Fewer filters:"),
            wait(400),
            unhover(),
        ],
        "chrome-toast-saved" => vec![
            click("Save Preset"),
            UiDrive::Fill {
                target: "Name".to_owned(),
                text: "Stadium Lead".to_owned(),
            },
            click("Save"),
            wait(100),
        ],
        "settings-plugin" => vec![settings()],
        "settings-advanced" => vec![settings(), scroll_settings("Calibration")],
        "settings-midi-empty" | "settings-midi-learning" => {
            vec![settings(), scroll_settings("MIDI Mapping")]
        }
        "settings-midi-picker" => vec![
            settings(),
            scroll_settings("MIDI Mapping"),
            click("Control to map"),
            wait(400),
        ],
        "settings-presets" => vec![settings(), scroll_settings("Presets")],
        "settings-system"
        | "settings-system-mic-denied"
        | "settings-system-feedback-risk"
        | "settings-system-input-muted" => {
            vec![system()]
        }
        "settings-system-rate96" => vec![system(), scroll_settings("MIDI Inputs"), wait(300)],
        _ => Vec::new(),
    }
}

/// Public selftest entry points.
pub mod selftest {
    pub use super::{run, run_all, run_with_backend};
}
