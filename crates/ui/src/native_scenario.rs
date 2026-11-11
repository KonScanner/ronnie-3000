//! The one module that drives a running [`ChromeController`] to a named Default scenario.
//!
//! A scenario is a list of [`Step`]s, and [`ScenarioScript`] applies them at the points of
//! the shell's [`ChromeScript`] seam. The shell knows no scenario ID, so every branch on an
//! ID lives here.
//!
//! Three routes share one list of steps:
//!
//! - [`NativeScenario::run`] opens a real window and captures its pixels. It calls controller
//!   operations, because real input in the window leaves hover and focus state in the pixels.
//! - [`reach_scenario`] applies the same steps with no window, so a test can check every state.
//! - [`driver_for_scenario`] keeps only the steps that the fixture's drives do not replay,
//!   and those drives then play real input from that state.
//!
//! [`CatalogSeed`] is the narrow hook for what input cannot reach, because catalog results,
//! errors, and requests in flight need a catalog.

mod artwork;

use std::{path::PathBuf, sync::Arc};

use eframe::egui;
use ronie_core::{Page, PresetInfo, Tone, ToneQuery, ToneSort};

use self::artwork::FixtureArtwork;
use crate::{
    backend::MockBackend,
    default::{
        AuthPhase, AuthSnapshot, BlockDetail, BlockView, CatalogSeed, ChromeController, DeckKind,
        FilterMenu, SettingsTab, SystemDeviceState,
    },
    services::{
        key_setup::{CommunityParts, KeySetup, KeySetupError},
        session::{AuthFlowAdapter, FlowEvent},
    },
    shell::{ChromeScript, NativeShellOptions},
    testbed::{UiScenario, UiSection},
};

/// One change that moves a running controller toward a named state.
///
/// Each step calls the controller operation that the matching control calls, except for
/// [`Step::Catalog`], which seeds what only a catalog can produce.
#[derive(Debug, Clone)]
pub enum Step {
    /// Sets the contextual help, as a pointer on a control does.
    Hint(String),
    /// Opens one block in detail view.
    OpenDetail(String),
    /// Selects the detail body.
    DetailView(BlockView),
    /// Selects the EQ curve instead of the sliders.
    EqCurve,
    /// Opens the model picker in block detail.
    ModelPicker,
    /// Opens one advanced faceplate deck.
    Deck(DeckKind),
    /// Opens the input-routing chooser above the faceplate.
    InputMode,
    /// Opens the account menu.
    Account,
    /// Opens the save-preset editor.
    SavePreset,
    /// Opens the preset browser.
    PresetBrowser,
    /// Types a name into the save-preset draft.
    PresetDraft(String),
    /// Saves the draft as a real preset, so the saved toast shows.
    SaveDraft,
    /// Scrolls the first insert tile into view.
    RevealInsert,
    /// Opens tone selection for the last tile of the left lane.
    OpenBrowser,
    /// Sets the independent browser zoom.
    BrowserZoom(f32),
    /// Seeds a catalog state in the open browser.
    Catalog(CatalogSeed),
    /// Expands the secondary filter row.
    FiltersExpanded,
    /// Opens one browser filter menu.
    FilterMenu(FilterMenu),
    /// Opens the tuner and lets its display settle.
    Tuner,
    /// Shows a timed toast.
    Toast(String),
    /// Opens one settings page.
    Settings(SettingsTab),
    /// Scrolls one settings heading into view.
    SettingsHeading(String),
    /// Opens the MIDI target picker.
    MidiPicker,
    /// Shows an authentication flow that already started.
    ActivateAuth,
    /// Scrolls the browser grid. This waits for the first frame, which sizes the grid.
    BrowserScroll(f32),
    /// Opens one tile context menu. This waits for the first frame, which places the tile.
    TileMenu(String),
    /// Picks a catalog card. This waits for the first clean backdrop, which the busy card
    /// shows. A signed-out user gets the sign-in page and not the tone.
    PickCard(Option<Tone>),
    /// Takes the network away, so the browse action shows its modal. This waits for two
    /// clean backdrops, because the modal paints over a frame without it.
    Offline,
    /// Applies an update payload, so its notice shows. This waits for a clean backdrop.
    UpdateOffer(serde_json::Value),
    /// Asks for a sign-in with no key, so the key page opens.
    KeyPage,
    /// Types a value that is not a key on the key page and saves it, so the page shows its
    /// error.
    RejectKey(String),
}

/// The point of the shell's run at which a step applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    Start,
    FirstFrame,
    Backdrop(u32),
}

impl Step {
    fn when(&self) -> When {
        match self {
            Self::BrowserScroll(_) | Self::TileMenu(_) => When::FirstFrame,
            Self::PickCard(_) | Self::UpdateOffer(_) => When::Backdrop(1),
            Self::Offline => When::Backdrop(2),
            _ => When::Start,
        }
    }

    /// Whether a real-input driver starts after this step, because no drive replays it.
    const fn is_before_drives(&self) -> bool {
        matches!(
            self,
            Self::OpenDetail(_)
                | Self::RevealInsert
                | Self::OpenBrowser
                | Self::BrowserZoom(_)
                | Self::Catalog(
                    CatalogSeed::Page { .. } | CatalogSeed::Failed(_) | CatalogSeed::Pending
                )
                | Self::Settings(_)
        )
    }

    fn apply(&self, chrome: &mut ChromeController) -> Result<(), String> {
        fn detail(chrome: &mut ChromeController) -> Result<&mut BlockDetail, String> {
            chrome
                .detail_mut()
                .ok_or_else(|| "no block detail is open".to_owned())
        }
        fn browser(
            chrome: &mut ChromeController,
        ) -> Result<&mut crate::default::BrowserView, String> {
            chrome
                .browser_mut()
                .ok_or_else(|| "no tone browser is open".to_owned())
        }
        fn settings(
            chrome: &mut ChromeController,
        ) -> Result<&mut crate::default::SettingsController, String> {
            chrome
                .settings_mut()
                .ok_or_else(|| "no settings page is open".to_owned())
        }
        match self {
            Self::Hint(text) => chrome.set_hint(text.clone()),
            Self::OpenDetail(id) => chrome.open_detail(id).map_err(|error| error.to_string())?,
            Self::DetailView(view) => detail(chrome)?.set_view(*view),
            Self::EqCurve => detail(chrome)?.set_eq_curve(true),
            Self::ModelPicker => {
                let session = chrome.session();
                detail(chrome)?.open_model_picker(session);
            }
            Self::Deck(kind) => chrome.open_deck(*kind),
            Self::InputMode => chrome.open_input_mode(),
            Self::Account => chrome.open_account(),
            Self::SavePreset => chrome.open_save(),
            Self::PresetBrowser => chrome.open_presets(),
            Self::PresetDraft(name) => chrome.set_draft(name.clone()),
            Self::SaveDraft => chrome.save().map_err(|error| error.to_string())?,
            Self::RevealInsert => {
                let target = chrome
                    .backend()
                    .chain_state(0)
                    .chain
                    .left
                    .iter()
                    .find(|block| block.is_insert)
                    .map(|block| block.id.clone())
                    .ok_or_else(|| "the left lane has no insert tile".to_owned())?;
                chrome.gallery_mut().reveal_tile(&target.0);
            }
            Self::OpenBrowser => {
                let target = chrome
                    .backend()
                    .chain_state(0)
                    .chain
                    .left
                    .last()
                    .map(|block| block.id.clone())
                    .ok_or_else(|| "the left lane has no tile".to_owned())?;
                chrome
                    .gallery_mut()
                    .select_tone(ronie_core::ChainSide::Left, target);
                chrome.sync_browser();
            }
            Self::BrowserZoom(zoom) => browser(chrome)?.set_zoom(*zoom),
            Self::Catalog(seed) => {
                if !chrome.seed_catalog(seed.clone()) {
                    return Err("no tone browser is open".to_owned());
                }
            }
            Self::FiltersExpanded => browser(chrome)?.set_filters_expanded(true),
            Self::FilterMenu(menu) => browser(chrome)?.set_filter_menu(Some(*menu)),
            Self::Tuner => {
                chrome.set_tuner(true);
                for tick in 0..20 {
                    chrome.sync_tuner(tick * 33);
                }
            }
            Self::Toast(message) => chrome.show_toast(message.clone(), unix_time_ms()),
            Self::Settings(SettingsTab::Plugin) => chrome.open_settings(),
            Self::Settings(SettingsTab::System) => chrome.open_system_settings(),
            Self::SettingsHeading(heading) => {
                let settings = settings(chrome)?;
                if heading == "MIDI Inputs" {
                    settings.scroll_to_heading_centered(heading);
                } else {
                    settings.scroll_to_heading(heading);
                }
            }
            Self::MidiPicker => settings(chrome)?.plugin_mut().midi_mut().open_picker(),
            Self::ActivateAuth => chrome.activate_auth(),
            Self::BrowserScroll(scroll) => browser(chrome)?.set_scroll(*scroll),
            Self::TileMenu(id) => chrome.gallery_mut().open_tile_menu(id),
            Self::PickCard(tone) => {
                if chrome.browser().is_some() && chrome.session().can_sign_in() {
                    browser(chrome)?.set_sign_in_page(true);
                } else {
                    let tone = tone.clone().ok_or_else(|| "no card to pick".to_owned())?;
                    chrome
                        .load_flow_mut()
                        .begin_load(tone)
                        .map_err(|error| error.to_string())?;
                    chrome.sync_browser();
                }
            }
            Self::Offline => {
                chrome.set_online(false);
                let _ = chrome.require_connection("browse");
            }
            Self::KeyPage => {
                chrome.sign_in().map_err(|error| error.to_string())?;
                if chrome.key_page().is_none() {
                    return Err("the sign-in did not open the key page".to_owned());
                }
            }
            Self::RejectKey(value) => {
                chrome
                    .key_page_mut()
                    .ok_or_else(|| "no key page is open".to_owned())?
                    .set_draft(value.clone());
                if chrome.save_key().is_ok() {
                    return Err("the key page saved a value that is not a key".to_owned());
                }
            }
            Self::UpdateOffer(payload) => {
                chrome.apply_update_payload(payload.clone(), unix_time_ms());
            }
        }
        Ok(())
    }
}

/// Applies a list of steps at the points of the shell's seam.
pub struct ScenarioScript {
    steps: Vec<Step>,
    waits_for_modal: bool,
    artwork: Option<FixtureArtwork>,
    errors: Vec<String>,
}

impl ScenarioScript {
    /// Creates a script that applies `steps` in order, each one at its own point of the run.
    #[must_use]
    pub fn new(steps: Vec<Step>) -> Self {
        let waits_for_modal = steps
            .iter()
            .any(|step| matches!(step, Step::Offline | Step::UpdateOffer(_)));
        Self {
            steps,
            waits_for_modal,
            artwork: None,
            errors: Vec::new(),
        }
    }

    fn apply_due(&mut self, chrome: &mut ChromeController, due: impl Fn(When) -> bool) {
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.steps)
            .into_iter()
            .partition(|step| due(step.when()));
        self.steps = waiting;
        for step in ready {
            if let Err(error) = step.apply(chrome) {
                self.errors.push(format!("{step:?}: {error}"));
            }
        }
    }

    fn apply_start(&mut self, chrome: &mut ChromeController) {
        self.apply_due(chrome, |when| when == When::Start);
    }

    /// Fails with every step that went wrong or that did not get its turn.
    fn finish(&self) -> Result<(), String> {
        if self.errors.is_empty() && self.steps.is_empty() {
            return Ok(());
        }
        Err(format!(
            "failed steps {:?}; steps without a turn {:?}",
            self.errors, self.steps
        ))
    }
}

impl ChromeScript for ScenarioScript {
    fn start(&mut self, chrome: &mut ChromeController, context: &egui::Context) {
        self.apply_start(chrome);
        if let Some(fixture) = &self.artwork {
            artwork::install(fixture, context, chrome);
        }
    }

    fn after_frame(&mut self, chrome: &mut ChromeController, frame: u64) {
        if frame == 1 {
            self.apply_due(chrome, |when| when == When::FirstFrame);
        }
    }

    fn backdrop_ready(&mut self, chrome: &mut ChromeController, count: u32) {
        self.apply_due(
            chrome,
            |when| matches!(when, When::Backdrop(needed) if needed <= count),
        );
    }

    fn wants_clean_backdrops(&self) -> bool {
        self.steps
            .iter()
            .any(|step| matches!(step.when(), When::Backdrop(_)))
    }

    fn capture_waits_for_modal(&self) -> bool {
        self.waits_for_modal
    }

    fn report(&self) -> serde_json::Value {
        serde_json::json!({
            "steps_without_a_turn": self.steps.len(),
            "failed_steps": self.errors,
        })
    }

    fn passed(&self) -> bool {
        self.finish().is_ok()
    }
}

/// One Default scenario, ready for a window.
pub struct NativeScenario {
    /// Production inputs of the shell, with the scenario's mock as the backend.
    pub options: NativeShellOptions,
    /// Steps that move the running shell to the scenario's named state.
    pub script: ScenarioScript,
    backend: MockBackend,
}

impl NativeScenario {
    /// Prepares one deterministic Default scenario.
    ///
    /// # Errors
    /// Returns an invalid scenario payload or unsupported-state error.
    pub fn new(
        scenario: &UiScenario,
        capture: Option<PathBuf>,
        report: Option<PathBuf>,
    ) -> Result<Self, String> {
        let scenario = &prepared_payload(scenario);
        let id = scenario.id.as_str();
        let state = &scenario.state;
        let backend = MockBackend::from_scenario(scenario);
        let presets = state["presets"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|preset| {
                        Some(PresetInfo {
                            id: preset["id"].as_str()?.to_owned(),
                            name: preset["name"].as_str()?.to_owned(),
                            factory: preset["factory"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let options = NativeShellOptions {
            selftest: true,
            capture,
            report,
            scale: Some(scenario.viewport.scale),
            backend: Some(Arc::new(backend.clone())),
            standalone: state["chain"]["standalone"].as_bool().unwrap_or(false),
            presets,
            authenticated: state["auth"].as_bool().unwrap_or(false),
            hints_enabled: state["hints"].as_bool().unwrap_or(true),
            auth: auth_flow(id).map(|flow| Box::new(flow) as Box<dyn AuthFlowAdapter>),
            key_setup: key_setup(id).map(|setup| Box::new(setup) as Box<dyn KeySetup>),
            ..Default::default()
        };
        let mut script = ScenarioScript::new(steps_for(scenario));
        script.artwork = fixture_artwork(scenario, &backend);
        Ok(Self {
            options,
            script,
            backend,
        })
    }

    /// Opens the window at this size in native points, and not at the scenario's scale.
    pub fn use_window_size(&mut self, size: [f32; 2]) {
        self.options.scale = None;
        self.options.window_size = Some(size);
        if let Some(artwork) = &mut self.script.artwork {
            artwork.scale = 1.0;
        }
    }

    /// Runs the scenario in a real window.
    ///
    /// # Errors
    /// Returns native window creation, rendering, or event-loop errors.
    pub fn run(self) -> eframe::Result<()> {
        crate::shell::run_scripted(self.options, self.script)
    }
}

/// Brings a scenario to its named state with no window, on the same steps as a capture.
///
/// The shell's seam points run in the same order as in a window, and every clean backdrop
/// that a step waits for arrives.
///
/// # Errors
/// Returns an error for a step that fails or that does not get its turn.
pub fn reach_scenario(scenario: &UiScenario) -> Result<crate::driver::UiDriver, String> {
    let NativeScenario {
        mut options,
        mut script,
        backend,
    } = NativeScenario::new(scenario, None, None)?;
    let mut chrome = crate::shell::build_chrome(&mut options)
        .ok_or_else(|| format!("{}: the scenario has no backend", scenario.id))?;
    script.apply_start(&mut chrome);
    let mut driver = crate::driver::UiDriver::new(chrome, backend);
    script.after_frame(driver.chrome_mut(), 1);
    driver.settle();
    for count in 1..=2 {
        // The shell takes no clean backdrop while a system modal is on the screen.
        if !driver.chrome().system_modal_visible() {
            script.backdrop_ready(driver.chrome_mut(), count);
            driver.settle();
        }
    }
    script
        .finish()
        .map_err(|error| format!("{}: {error}", scenario.id))?;
    Ok(driver)
}

/// Builds a real-input driver at the state before a scenario's drives.
///
/// A capture opens its surface directly, because it only needs the pixels. A driven scenario
/// must open that surface with real input, so every opener stays closed here. `backend`
/// replaces the scenario's own mock, so the caller keeps its event log.
///
/// # Errors
/// Returns an invalid scenario payload error.
pub fn driver_for_scenario(
    scenario: &UiScenario,
    backend: MockBackend,
) -> Result<crate::driver::UiDriver, String> {
    let NativeScenario {
        mut options,
        mut script,
        ..
    } = NativeScenario::new(scenario, None, None)?;
    options.backend = Some(Arc::new(backend.clone()));
    options.auth = auth_flow(&scenario.id)
        .map(|flow| Box::new(flow.before_drives()) as Box<dyn AuthFlowAdapter>);
    let mut chrome = crate::shell::build_chrome(&mut options)
        .ok_or_else(|| format!("{}: the scenario has no backend", scenario.id))?;
    script.steps.retain(Step::is_before_drives);
    script.apply_start(&mut chrome);
    script.steps.clear();
    script
        .finish()
        .map_err(|error| format!("{}: {error}", scenario.id))?;
    Ok(crate::driver::UiDriver::new(chrome, backend))
}

/// Runs the shell's logic checks on the scene of the `main-mono` scenario, with no window.
#[must_use]
pub fn run_logic_selftest() -> serde_json::Value {
    let accessibility = crate::testbed::load_scenarios(include_str!(
        "../../../fixtures/ui-scenarios.json"
    ))
    .ok()
    .and_then(|scenarios| {
        scenarios
            .into_iter()
            .find(|scenario| scenario.id == "main-mono")
    })
    .map_or_else(
        || serde_json::json!({"passed": false, "node_count": 0, "focusable_count": 0, "unnamed_focusable": 0}),
        |scenario| {
            let backend = Arc::new(MockBackend::from_scenario(&scenario));
            let mut chrome = ChromeController::new(backend, Vec::new(), false, true);
            crate::shell::audit_chrome_accessibility(&mut chrome)
        },
    );
    crate::shell::logic_report(&accessibility)
}

/// Applies the changes that a scenario needs in its fixture payload.
fn prepared_payload(scenario: &UiScenario) -> UiScenario {
    let mut scenario = scenario.clone();
    if scenario.id == "main-detail-info" {
        scenario.state["chain"]["chain"][1]["tone"]["description"] = serde_json::Value::String(
            "A 2002 Vox AC30/6 Top Boost captured through the Brilliant and Normal channels at a range of volume settings. The Brl (Brilliant) models run the Top Boost tone stack with the cut control at noon; the Nrm models are the flatter Normal channel. Recorded with a Suhr Reactive Load and a matched-level reamp, so the input calibration metadata is included: set your interface to 12.5 dBu for 0 dBFS and the model responds like the amp did.".to_owned(),
        );
    }
    if scenario.id == "main-detail-model-select" {
        let tone = &mut scenario.state["chain"]["chain"][1]["tone"];
        let base = tone["models"][0].clone();
        let first_id = base["id"].as_i64().unwrap_or(1101);
        let mut models = vec![base.clone()];
        for (index, name) in [
            "AC30-6 TB Brl 2",
            "AC30-6 TB Brl 4",
            "AC30-6 TB Nrm 2",
            "AC30-6 TB Nrm 3",
        ]
        .into_iter()
        .enumerate()
        {
            let mut model = base.clone();
            model["id"] = serde_json::Value::from(first_id + index as i64 + 1);
            model["name"] = serde_json::Value::from(name);
            models.push(model);
        }
        tone["models"] = serde_json::Value::Array(models);
    }
    let authenticated = scenario.state["auth"].as_bool().unwrap_or(false);
    for lane in ["chain", "chainRight"] {
        if scenario.state["chain"].get(lane).is_none() {
            continue;
        }
        for block in scenario.state["chain"][lane]
            .as_array_mut()
            .into_iter()
            .flatten()
        {
            if authenticated
                && !scenario.id.starts_with("load-detail")
                && block["tone"].is_object()
                && block["tone"]["favorites_count"].is_null()
            {
                block["tone"]["favorites_count"] = serde_json::Value::from(320);
            }
        }
    }
    scenario
}

/// The steps of one scenario. Each step applies at its own point of the run, and steps at
/// one point apply in this order.
fn steps_for(scenario: &UiScenario) -> Vec<Step> {
    let id = scenario.id.as_str();
    let state = &scenario.state;
    let catalog = visible_catalog_page(scenario);
    let mut steps = Vec::new();
    if let Some(hint) = hint_text(id) {
        steps.push(Step::Hint(hint.to_owned()));
    }
    if let Some(target) = detail_target(id) {
        steps.push(Step::OpenDetail(target));
    }
    if let Some(view) = detail_view(id) {
        steps.push(Step::DetailView(view));
    }
    if id == "main-detail-eq-curve" {
        steps.push(Step::EqCurve);
    }
    if id == "main-detail-model-select" {
        steps.push(Step::ModelPicker);
    }
    if let Some(kind) = deck(id) {
        steps.push(Step::Deck(kind));
    }
    if id == "chrome-input-mode" {
        steps.push(Step::InputMode);
    }
    match id {
        "chrome-account-signed-out" | "chrome-account-signed-in" => steps.push(Step::Account),
        "chrome-preset-save" => {
            steps.push(Step::SavePreset);
            steps.push(Step::PresetDraft("Stadium Lead".to_owned()));
        }
        "chrome-preset-browse" => steps.push(Step::PresetBrowser),
        "chrome-toast-saved" => {
            steps.push(Step::PresetDraft("Stadium Lead".to_owned()));
            steps.push(Step::SaveDraft);
        }
        _ => {}
    }
    if id == "load-offline-modal" {
        steps.push(Step::RevealInsert);
    }
    if scenario.section == UiSection::Browser {
        steps.push(Step::OpenBrowser);
        steps.push(Step::BrowserZoom(
            state["zoom"].as_f64().unwrap_or(1.0) as f32
        ));
        steps.push(Step::Catalog(CatalogSeed::Query(browser_query(id))));
        if let Some(page) = catalog.clone() {
            steps.push(Step::Catalog(CatalogSeed::Page {
                page,
                trending: !state["auth"].as_bool().unwrap_or(false),
            }));
        }
        if state["api"]["search"].as_str() == Some("error")
            || state["api"]["trending"].as_str() == Some("error")
        {
            steps.push(Step::Catalog(CatalogSeed::Failed(
                "request failed".to_owned(),
            )));
        }
        if catalog_pending(id) {
            steps.push(Step::Catalog(CatalogSeed::Pending));
        }
        if filter_expanded(id) {
            steps.push(Step::FiltersExpanded);
        }
        if let Some(menu) = filter_menu(id) {
            steps.push(Step::FilterMenu(menu));
        }
    }
    if id.starts_with("main-tuner") {
        steps.push(Step::Tuner);
    }
    if scenario.section == UiSection::Settings {
        steps.push(Step::Settings(if id.starts_with("settings-system") {
            SettingsTab::System
        } else {
            SettingsTab::Plugin
        }));
    }
    let heading = scenario.drives.iter().find_map(|drive| match drive {
        crate::backend::UiDrive::ScrollSettings { heading } => Some(heading.clone()),
        _ => None,
    });
    if let Some(heading) = heading {
        steps.push(Step::SettingsHeading(heading));
    }
    if id == "settings-midi-picker" {
        steps.push(Step::MidiPicker);
    }
    if id.starts_with("load-oauth") || id == "browser-signed-out-signing-in" {
        steps.push(Step::ActivateAuth);
    }
    match id {
        "chrome-key-setup" => steps.push(Step::KeyPage),
        "chrome-key-setup-error" => {
            steps.push(Step::KeyPage);
            steps.push(Step::RejectKey("not-a-key".to_owned()));
        }
        _ => {}
    }
    match id {
        "browser-scrolled" => steps.push(Step::BrowserScroll(100.0)),
        "browser-return" => steps.push(Step::BrowserScroll(240.0)),
        "browser-signed-out-footer" => steps.push(Step::BrowserScroll(10_000.0)),
        "chrome-tile-menu" => steps.push(Step::TileMenu("blk-2".to_owned())),
        "browser-picking" | "browser-signed-out-card" => {
            let tone = catalog.and_then(|page| page.data.first().cloned());
            steps.push(Step::PickCard(tone));
        }
        "load-offline-modal" => steps.push(Step::Offline),
        "load-update-notice" | "settings-plugin" => {
            steps.push(Step::UpdateOffer(state["api"]["version"].clone()));
        }
        _ => {}
    }
    steps
}

fn hint_text(id: &str) -> Option<&'static str> {
    Some(match id {
        "chrome-gate-deck" => {
            "Gate: noise gate threshold, -100 to 0 dB. Right-click: advanced. Shift+drag: fine · double-click: type · Alt+click: reset"
        }
        "chrome-pitch-deck" | "chrome-pitch-tuning" | "chrome-pitch-tuning-bass5" => {
            "Pitch Shift: transpose ±24 st. STEP off: whammy sweep. Right-click: tuning picker, advanced. Shift+drag: fine · double-click: type · Alt+click: reset"
        }
        "chrome-pitch-optimize" | "chrome-pitch-optimize-off" | "chrome-pitch-optimize-delay" => {
            "OPTIMIZE: keeps the lowest delay for the instrument and the shift. A Buffer change turns it off."
        }
        "chrome-spread-deck" | "chrome-spread-on" => {
            "Offset: double-track lag, ≤24 ms toward L or R. Center: off. Right-click: advanced. Shift+drag: fine · double-click: type · Alt+click: reset"
        }
        "chrome-align-deck" => "Align: corrective chain time alignment. Click: enable.",
        "chrome-auto-balance" => {
            "Auto Balance: click, play ~2 s to match chain levels. Click again: cancel."
        }
        "chrome-input-mode" => "Input Mode: how the stereo source feeds the chain. Click: choose.",
        "chrome-account-signed-in" => "Account: settings and sign-out.",
        "chrome-account-signed-out" => "Account: settings and sign-in.",
        "browser-gear" | "browser-signed-out-gear" => {
            "Gear: only tones of this type. Click again: all types."
        }
        "browser-search" | "browser-search-typed" => {
            "Search: find tones on TONE3000 by title, gear, tags or creator."
        }
        "browser-profile-menu" => "Profile: your recently used, favorited or created tones.",
        "browser-filters-expanded" => {
            "Filters: sort, and narrow by format, tags, makes, creators or calibration."
        }
        "browser-sort-menu" | "browser-zoom-menu" => "Sort: order the results.",
        "browser-sort-cleared" | "browser-sort-newest" | "browser-format-ir" => {
            "Click the name: change this filter · ×: clear it."
        }
        "browser-format-menu" => "Format: only NAM captures or impulse responses.",
        "browser-tags-menu" | "browser-tags-picked" => {
            "Tags: only tones carrying any of the chosen tags."
        }
        "browser-creators-menu" => "Creators: only tones by the chosen creators.",
        "browser-calibrated-ir" => {
            "Not for impulse responses: cabinets and spaces carry no calibration data."
        }
        "browser-filters-active-collapsed" => "Fewer filters: fold these away. They stay applied.",
        "chrome-preset-save" => "Save Preset: store the current chain. Same name: overwrite.",
        "chrome-preset-browse" => "Presets: browse factory and user presets.",
        "chrome-tile-menu" | "chrome-toast-saved" => {
            "'02 Vox AC30/6 Top Boost. Click: open · drag: reorder · Alt+drag: duplicate · right-click: copy / load file."
        }
        "main-tuner-idle"
        | "main-tuner-intune"
        | "main-tuner-flat"
        | "main-tuner-sharp"
        | "main-tuner-accidental"
        | "main-tuner-shifted" => "Tuner: chromatic tuner. Click again: back.",
        "main-detail-info" | "main-detail-info-signed-out" => {
            "Info: tone description, makes, and tags from TONE3000."
        }
        "main-detail-eq-sliders" => "EQ: 6-band EQ editor. Outline: EQ shaping the sound.",
        "main-detail-eq-curve" => "Curve: parametric freq/gain/Q editor.",
        _ => return None,
    })
}

fn deck(id: &str) -> Option<DeckKind> {
    match id {
        "chrome-gate-deck" => Some(DeckKind::Gate),
        "chrome-pitch-deck"
        | "chrome-pitch-tuning"
        | "chrome-pitch-tuning-bass5"
        | "chrome-pitch-optimize"
        | "chrome-pitch-optimize-off"
        | "chrome-pitch-optimize-delay" => Some(DeckKind::Pitch),
        "chrome-spread-deck" => Some(DeckKind::Spread),
        "chrome-align-deck" => Some(DeckKind::Align),
        "chrome-auto-balance" => Some(DeckKind::AutoBalance),
        _ => None,
    }
}

fn catalog_pending(id: &str) -> bool {
    matches!(id, "browser-loading" | "browser-signed-out-loading")
}

/// The catalog page that the scenario shows, which a request in flight does not have yet.
fn visible_catalog_page(scenario: &UiScenario) -> Option<Page<Tone>> {
    if catalog_pending(&scenario.id) {
        return None;
    }
    catalog_page(scenario)
}

fn fixture_artwork(scenario: &UiScenario, backend: &MockBackend) -> Option<FixtureArtwork> {
    if scenario.state["imagesOffline"].as_bool().unwrap_or(false) {
        return None;
    }
    let root = std::env::var_os("RONIE_REFERENCE_IMAGE_DIR").map(PathBuf::from)?;
    Some(FixtureArtwork {
        root,
        scale: scenario.viewport.scale.max(0.1),
        backend: Arc::new(backend.clone()),
        catalog: visible_catalog_page(scenario),
    })
}

fn unix_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

/// Compares complete Reference and candidate capture suites and writes a JSON report.
///
/// # Errors
/// Returns missing-image, comparison, report-write, or threshold errors.
pub fn compare_suite(
    reference_directory: &std::path::Path,
    candidate_directory: &std::path::Path,
    report_path: &std::path::Path,
    scenarios: &[UiScenario],
) -> Result<(), String> {
    let mut rows = Vec::new();
    let mut passed = true;
    for scenario in scenarios {
        let reference_path = reference_directory.join(format!("{}.png", scenario.id));
        let candidate_path = candidate_directory.join(format!("{}.png", scenario.id));
        if !reference_path.exists() {
            // A new scenario has no reference image yet. It fails, and the other scenarios
            // still get their rows, so the report lists each change.
            passed = false;
            rows.push(serde_json::json!({
                "id": scenario.id,
                "passed": false,
                "missing_reference": true,
            }));
            continue;
        }
        let mut reference = load_color_image(&reference_path)?;
        let mut candidate = load_color_image(&candidate_path)?;
        let branding_exclusion = mask_ronie_wordmark(&mut reference, &mut candidate, scenario);
        let diff_path = candidate_directory.join(format!("{}.diff.png", scenario.id));
        let comparison = crate::visual::compare_images(&reference, &candidate, Some(&diff_path))?;
        let scenario_passed = comparison.mismatch_ratio <= crate::visual::GLOBAL_MISMATCH_MAX
            && comparison.worst_tile_ratio <= crate::visual::WORST_TILE_MAX;
        passed &= scenario_passed;
        rows.push(serde_json::json!({
            "id": scenario.id,
            "passed": scenario_passed,
            "mismatch_ratio": comparison.mismatch_ratio,
            "worst_tile_ratio": comparison.worst_tile_ratio,
            "worst_tile": comparison.worst_tile,
            "mismatched": comparison.mismatched,
            "ronie_wordmark_exclusion": branding_exclusion,
        }));
    }
    let report = serde_json::json!({
        "suite": "reference-default",
        "passed": passed,
        "scenario_count": rows.len(),
        "global_mismatch_max": crate::visual::GLOBAL_MISMATCH_MAX,
        "worst_tile_64_max": crate::visual::WORST_TILE_MAX,
        "scenarios": rows,
    });
    std::fs::write(
        report_path,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if !passed {
        return Err(format!(
            "visual comparison failed for {} scenario(s)",
            rows.iter().filter(|row| row["passed"] == false).count()
        ));
    }
    Ok(())
}

fn mask_ronie_wordmark(
    reference: &mut eframe::egui::ColorImage,
    candidate: &mut eframe::egui::ColorImage,
    scenario: &UiScenario,
) -> [usize; 4] {
    let scale = reference.size[0] as f32 / 1024.0;
    let banner = SystemDeviceState::from_ui_value(&scenario.state["device"])
        .as_ref()
        .is_some_and(|device| crate::shell::initial_banner_visible(&device.banner));
    let offset = if banner { 44.0 } else { 0.0 };
    let x0 = (24.0 * scale).round() as usize;
    let y0 = ((8.0 + offset) * scale).round() as usize;
    let x1 = (184.0 * scale).round() as usize;
    let y1 = ((56.0 + offset) * scale).round() as usize;
    let x1 = x1.min(reference.size[0]);
    let y1 = y1.min(reference.size[1]);
    for y in y0..y1 {
        for x in x0..x1 {
            let index = y * reference.size[0] + x;
            reference.pixels[index] = eframe::egui::Color32::BLACK;
            candidate.pixels[index] = eframe::egui::Color32::BLACK;
        }
    }
    [x0, y0, x1 - x0, y1 - y0]
}

fn load_color_image(path: &std::path::Path) -> Result<eframe::egui::ColorImage, String> {
    let image = image::open(path)
        .map_err(|error| format!("loading {}: {error}", path.display()))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(eframe::egui::ColorImage::from_rgba_unmultiplied(
        size,
        &image.into_raw(),
    ))
}

fn detail_target(id: &str) -> Option<String> {
    if id.starts_with("main-detail") || id.starts_with("load-detail") {
        Some("blk-2".to_owned())
    } else {
        None
    }
}

fn detail_view(id: &str) -> Option<BlockView> {
    if id.starts_with("main-detail-info") {
        Some(BlockView::Info)
    } else if id.starts_with("main-detail-eq") {
        Some(BlockView::Eq)
    } else {
        None
    }
}

fn browser_query(id: &str) -> ToneQuery {
    let mut query = ToneQuery::default();
    match id {
        "browser-search-typed" => query.text = "vox".to_owned(),
        "browser-gear" | "browser-signed-out-gear" | "browser-return" => {
            query.gear = "pedal".to_owned();
        }
        "browser-signed-out-empty" => query.gear = "space".to_owned(),
        "browser-verified" => query.verified = true,
        "browser-sort-newest" => query.sort = Some(ToneSort::Newest),
        "browser-format-ir" | "browser-filters-active-collapsed" => {
            query.format = "ir".to_owned();
        }
        "browser-calibrated-ir" => query.gear = "cab".to_owned(),
        "browser-tags-picked" => query.tags = vec!["rock".to_owned()],
        "browser-profile-recent" | "browser-profile-search" => {
            query.profile = ronie_core::Profile::Downloaded;
            query.text = if id == "browser-profile-search" {
                "vox".to_owned()
            } else {
                String::new()
            };
        }
        "browser-profile-empty" => query.profile = ronie_core::Profile::Favorited,
        _ => {}
    }
    query
}

fn filter_expanded(id: &str) -> bool {
    matches!(
        id,
        "browser-filters-expanded"
            | "browser-sort-menu"
            | "browser-sort-cleared"
            | "browser-zoom-menu"
            | "browser-sort-newest"
            | "browser-calibrated-ir"
            | "browser-format-ir"
            | "browser-tags-menu"
            | "browser-tags-picked"
            | "browser-creators-menu"
            | "browser-format-menu"
    )
}

fn filter_menu(id: &str) -> Option<FilterMenu> {
    match id {
        "browser-profile-menu" => Some(FilterMenu::Profile),
        "browser-sort-menu" | "browser-zoom-menu" => Some(FilterMenu::Sort),
        "browser-format-menu" => Some(FilterMenu::Format),
        "browser-tags-menu" | "browser-tags-picked" => Some(FilterMenu::Tags),
        "browser-creators-menu" => Some(FilterMenu::Creators),
        _ => None,
    }
}

fn catalog_page(scenario: &UiScenario) -> Option<Page<Tone>> {
    let authenticated = scenario.state["auth"].as_bool().unwrap_or(false);
    let query = browser_query(&scenario.id);
    let profile = query.profile != ronie_core::Profile::None;
    let payload = if profile {
        &scenario.state["api"]["gated"]
    } else if authenticated {
        &scenario.state["api"]["search"]
    } else {
        &scenario.state["api"]["trending"]
    };
    let overridden = payload.is_object();
    let mut data = if overridden {
        payload["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| Tone::from_value(row).ok())
            .collect::<Vec<_>>()
    } else {
        fixture_tones().to_vec()
    };
    if authenticated {
        let text = query.text.trim().to_lowercase();
        data.retain(|tone| {
            (text.is_empty() || tone.title.to_lowercase().contains(&text))
                && (query.gear.is_empty()
                    || tone.raw["gear"]
                        .as_str()
                        .unwrap_or_default()
                        .eq_ignore_ascii_case(&query.gear))
                && (profile || catalog_filters_match(tone, &query))
        });
    } else if !query.gear.is_empty() {
        data.retain(|tone| {
            tone.raw["gear"]
                .as_str()
                .unwrap_or_default()
                .eq_ignore_ascii_case(&query.gear)
        });
        data.truncate(10);
    }
    let narrowed_profile = profile && (!query.text.is_empty() || !query.gear.is_empty());
    let page_size = payload["page_size"].as_u64().unwrap_or(20);
    let total = if narrowed_profile {
        data.len() as u64
    } else if profile && !overridden {
        18
    } else {
        payload["total"].as_u64().unwrap_or(data.len() as u64)
    };
    let total_pages = if narrowed_profile {
        1
    } else if profile && !overridden {
        3
    } else {
        payload["total_pages"]
            .as_u64()
            .unwrap_or_else(|| total.div_ceil(page_size.max(1)))
    };
    Some(Page {
        data,
        page: payload["page"].as_u64().unwrap_or(1),
        page_size,
        total,
        total_pages,
    })
}

fn fixture_tones() -> &'static [Tone] {
    static TONES: std::sync::OnceLock<Vec<Tone>> = std::sync::OnceLock::new();
    TONES.get_or_init(|| {
        // The fixture is compiled in, and every scenario test reads every row of it.
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/ui-catalog.json"))
                .expect("valid catalog fixture");
        fixture["tones"]
            .as_array()
            .expect("catalog tones")
            .iter()
            .map(|row| Tone::from_value(row.clone()).expect("valid catalog tone"))
            .collect()
    })
}

fn catalog_filters_match(tone: &Tone, query: &ToneQuery) -> bool {
    (query.format.is_empty()
        || tone.raw["format"]
            .as_str()
            .unwrap_or_default()
            .eq_ignore_ascii_case(&query.format))
        && names_match(&query.tags, &tone.raw["tags"])
        && names_match(&query.makes, &tone.raw["makes"])
        && (query.creators.is_empty()
            || query.creators.iter().any(|creator| {
                tone.raw["user"]["username"]
                    .as_str()
                    .unwrap_or_default()
                    .eq_ignore_ascii_case(creator)
            }))
        && (!query.verified || tone.raw["user"]["is_verified"].as_bool().unwrap_or(false))
}

fn names_match(picked: &[String], carried: &serde_json::Value) -> bool {
    picked.is_empty()
        || picked.iter().any(|picked_name| {
            carried.as_array().into_iter().flatten().any(|entry| {
                entry["name"]
                    .as_str()
                    .unwrap_or_default()
                    .eq_ignore_ascii_case(picked_name)
            })
        })
}

#[derive(Debug, Clone)]
struct ScenarioAuthFlow {
    snapshot: AuthSnapshot,
    browser_url: Option<String>,
}

impl ScenarioAuthFlow {
    fn new(id: &str) -> Self {
        let phase = match id {
            "load-oauth-device-requesting" => AuthPhase::DeviceRequesting,
            "load-oauth-device" => AuthPhase::DeviceWaiting,
            "load-oauth-returning" => AuthPhase::Returning,
            "browser-signed-out-signed-in" => AuthPhase::Authenticated,
            "browser-signed-out-error" => AuthPhase::Error,
            "load-oauth-device-expired" => AuthPhase::DeviceFailed,
            "load-oauth-error" => AuthPhase::Error,
            _ => AuthPhase::BrowserWaiting,
        };
        Self {
            browser_url: None,
            snapshot: AuthSnapshot {
                phase,
                copied: id == "load-oauth-copied",
                user_code: (id == "load-oauth-device").then(|| "BCDF-GHJK".to_owned()),
                verification_uri: (id == "load-oauth-device")
                    .then(|| "https://fixtures.invalid/activate".to_owned()),
                browser_problem: match id {
                    "load-oauth-browser-failed" | "load-oauth-copied" => Some(
                        "Your browser couldn't be opened. Copy link to sign in, then return here."
                            .to_owned(),
                    ),
                    _ => Some(
                        "Your browser didn't open? Copy link to sign in, then return here."
                            .to_owned(),
                    ),
                },
                pending_intent: "browse".to_owned(),
                error: match id {
                    "load-oauth-device-expired" => Some(
                        "That code expired. Get a new one, or finish on your browser.".to_owned(),
                    ),
                    "load-oauth-error" => Some(
                        "Could not reach Ronnie. Check your internet connection and try again."
                            .to_owned(),
                    ),
                    "browser-signed-out-error" => {
                        Some("The sign-in flow did not complete.".to_owned())
                    }
                    _ => None,
                },
            },
        }
    }
}

impl ScenarioAuthFlow {
    /// The same flow before the user starts it, because a click must reach the captured phase.
    fn before_drives(mut self) -> Self {
        self.snapshot.phase = AuthPhase::Idle;
        self.snapshot.copied = false;
        self
    }
}

impl AuthFlowAdapter for ScenarioAuthFlow {
    fn snapshot(&self) -> AuthSnapshot {
        self.snapshot.clone()
    }

    fn start_browser(&mut self, _runtime: &tokio::runtime::Handle) {
        self.snapshot.phase = AuthPhase::BrowserWaiting;
        self.browser_url = Some("https://fixtures.invalid/authorize".to_owned());
    }

    fn start_device(&mut self, _runtime: &tokio::runtime::Handle) {
        self.snapshot.phase = AuthPhase::DeviceRequesting;
    }

    fn poll(&mut self) -> Option<FlowEvent> {
        self.browser_url.take().map(FlowEvent::BrowserReady)
    }

    fn copy_link(&mut self) -> Option<String> {
        self.snapshot.copied = true;
        Some("https://fixtures.invalid/authorize".to_owned())
    }

    fn sign_out(&mut self) -> Result<(), ronie_core::OAuthError> {
        self.snapshot.phase = AuthPhase::Idle;
        Ok(())
    }

    fn close(&mut self) {
        self.snapshot.phase = AuthPhase::Idle;
    }
}

/// A key setup that keeps its state in memory, so a scenario reads and writes no file.
#[derive(Debug)]
struct FixtureKeySetup {
    saved: bool,
}

impl KeySetup for FixtureKeySetup {
    fn origin(&self) -> String {
        "https://www.fixtures.invalid".to_owned()
    }

    fn has_saved_key(&self) -> bool {
        self.saved
    }

    /// A scenario has no service, so a saved key starts none.
    fn save(&mut self, _key: &str) -> Result<Option<CommunityParts>, KeySetupError> {
        self.saved = true;
        Ok(None)
    }

    fn remove(&mut self) -> Result<Option<CommunityParts>, KeySetupError> {
        self.saved = false;
        Ok(None)
    }
}

/// The key setup of the scenarios of a build with no key.
fn key_setup(id: &str) -> Option<FixtureKeySetup> {
    match id {
        "chrome-key-setup" | "chrome-key-setup-error" => Some(FixtureKeySetup { saved: false }),
        "settings-system-key-saved" => Some(FixtureKeySetup { saved: true }),
        _ => None,
    }
}

fn auth_flow(id: &str) -> Option<ScenarioAuthFlow> {
    (id.starts_with("load-oauth")
        || matches!(
            id,
            "browser-signed-out"
                | "browser-signed-out-gear"
                | "browser-signed-out-empty"
                | "browser-signed-out-footer"
                | "chrome-account-signed-out"
                | "main-detail-info-signed-out"
                | "browser-signed-out-card"
                | "browser-signed-out-signing-in"
                | "browser-signed-out-signed-in"
                | "browser-signed-out-error"
                | "browser-signed-out-loading"
                | "browser-loading"
        ))
    .then(|| ScenarioAuthFlow::new(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{shell::build_chrome, testbed::load_scenarios};

    fn prepared(id: &str) -> (ChromeController, ScenarioScript) {
        let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == id)
            .expect("scenario");
        let mut run = NativeScenario::new(&scenario, None, None).expect("scenario");
        let mut chrome = build_chrome(&mut run.options).expect("chrome");
        run.script.apply_start(&mut chrome);
        (chrome, run.script)
    }

    #[test]
    fn the_offline_modal_waits_for_two_clean_backdrops() {
        let (mut chrome, mut script) = prepared("load-offline-modal");
        assert!(script.capture_waits_for_modal());
        assert!(!chrome.system_modal_visible());
        script.after_frame(&mut chrome, 1);
        assert!(!chrome.system_modal_visible());
        script.backdrop_ready(&mut chrome, 1);
        assert!(!chrome.system_modal_visible());
        script.backdrop_ready(&mut chrome, 2);
        assert!(chrome.system_modal_visible());
        script.finish().expect("every step ran");
    }

    #[test]
    fn the_update_notice_waits_for_one_clean_backdrop() {
        for id in ["load-update-notice", "settings-plugin"] {
            let (mut chrome, mut script) = prepared(id);
            assert!(script.capture_waits_for_modal(), "{id}");
            assert!(!chrome.system_modal_visible(), "{id}");
            script.backdrop_ready(&mut chrome, 1);
            assert!(chrome.system_modal_visible(), "{id}");
            script.finish().expect("every step ran");
        }
    }

    #[test]
    fn a_browser_scroll_waits_for_the_first_frame() {
        for (id, position) in [
            ("browser-scrolled", 100.0),
            ("browser-return", 240.0),
            ("browser-signed-out-footer", 10_000.0),
        ] {
            let (mut chrome, mut script) = prepared(id);
            let scroll =
                |chrome: &ChromeController| chrome.browser().map(|browser| browser.scroll());
            assert_eq!(scroll(&chrome), Some(0.0), "{id}");
            script.after_frame(&mut chrome, 0);
            assert_eq!(scroll(&chrome), Some(0.0), "{id}");
            script.after_frame(&mut chrome, 1);
            assert_eq!(scroll(&chrome), Some(position), "{id}");
        }
    }

    #[test]
    fn a_tile_menu_waits_for_the_first_frame() {
        let (mut chrome, mut script) = prepared("chrome-tile-menu");
        assert!(matches!(script.steps.as_slice(), [Step::TileMenu(_)]));
        script.after_frame(&mut chrome, 0);
        assert!(matches!(script.steps.as_slice(), [Step::TileMenu(_)]));
        script.after_frame(&mut chrome, 1);
        assert!(script.steps.is_empty());
        script.finish().expect("every step ran");
    }

    #[test]
    fn a_picked_card_waits_for_the_first_clean_backdrop() {
        let (mut chrome, mut script) = prepared("browser-picking");
        let pending = |chrome: &ChromeController| {
            chrome
                .browser()
                .and_then(crate::default::BrowserView::pending_tone)
        };
        script.after_frame(&mut chrome, 1);
        assert_eq!(pending(&chrome), None);
        script.backdrop_ready(&mut chrome, 1);
        assert!(pending(&chrome).is_some());
        script.finish().expect("every step ran");

        let (mut chrome, mut script) = prepared("browser-signed-out-card");
        let sign_in = |chrome: &ChromeController| {
            chrome
                .browser()
                .is_some_and(crate::default::BrowserView::sign_in_page)
        };
        script.after_frame(&mut chrome, 1);
        assert!(!sign_in(&chrome));
        script.backdrop_ready(&mut chrome, 1);
        assert!(sign_in(&chrome));
        script.finish().expect("every step ran");
    }

    #[test]
    fn a_settings_scenario_asks_for_the_heading_of_its_drive() {
        for (id, heading) in [
            ("settings-advanced", "Calibration"),
            ("settings-midi-empty", "MIDI Mapping"),
            ("settings-presets", "Presets"),
            ("settings-system-rate96", "MIDI Inputs"),
        ] {
            let (chrome, _script) = prepared(id);
            let target = chrome.settings().and_then(|page| page.scroll_target());
            assert_eq!(target, Some(heading), "{id}");
        }
    }

    #[test]
    fn the_offline_modal_reveals_the_insert_tile_before_any_frame() {
        let (_chrome, script) = prepared("load-offline-modal");
        assert!(matches!(script.steps.as_slice(), [Step::Offline]));
        let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "load-offline-modal")
            .expect("scenario");
        assert!(
            steps_for(&scenario)
                .iter()
                .any(|step| matches!(step, Step::RevealInsert))
        );
    }

    #[test]
    fn a_preset_is_saved_only_by_the_scenario_that_names_it() {
        let (chrome, _script) = prepared("chrome-preset-save");
        assert_eq!(chrome.overlay(), crate::default::Overlay::SavePreset);
        assert_eq!(chrome.toast(), None);
        let (chrome, _script) = prepared("chrome-toast-saved");
        assert_eq!(chrome.toast(), Some("Preset Saved"));
    }

    #[test]
    fn a_failed_step_is_reported_and_not_dropped() {
        let (mut chrome, _script) = prepared("main-mono");
        let mut script = ScenarioScript::new(vec![Step::BrowserZoom(2.0), Step::Offline]);
        script.apply_start(&mut chrome);
        let error = script.finish().expect_err("no browser, and no backdrop");
        assert!(error.contains("no tone browser is open"), "{error}");
        assert!(error.contains("Offline"), "{error}");
        assert_eq!(script.report()["steps_without_a_turn"], 1);
        assert_eq!(
            script.report()["failed_steps"].as_array().map(Vec::len),
            Some(1)
        );
        assert!(!script.passed());
    }
}
