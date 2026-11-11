//! All nine popovers through real input: real clicks, real Escape, and no wall clock.

use eframe::egui::{self, Key};
use ronie_ui::{
    backend::{MockBackend, UiEffect},
    default::{
        DeckKind, DismissReason, Overlay,
        popover::{self, Placement, Popover, Surface},
    },
    driver::{DriverError, UiDriver},
    native_scenario::driver_for_scenario,
    testbed::load_scenarios,
};

fn start(id: &str) -> UiDriver {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .unwrap_or_else(|| panic!("scenario {id}"));
    let backend = MockBackend::from_scenario(&scenario);
    driver_for_scenario(&scenario, backend).expect("driver")
}

fn knob_label(driver: &UiDriver, prefix: &str) -> String {
    driver
        .label_starting_with(prefix)
        .unwrap_or_else(|| panic!("no knob {prefix}: {:?}", driver.visible_labels()))
}

/// One row of the table: how to reach a popover, how to open it, and what proves it is open.
struct Case {
    name: &'static str,
    scenario: &'static str,
    prepare: fn(&mut UiDriver),
    /// Label of the control that opens the popover.
    trigger: fn(&UiDriver) -> String,
    secondary: bool,
    /// Label that only the open popover shows.
    inside: &'static str,
}

fn nothing(_: &mut UiDriver) {}

fn shows(driver: &UiDriver, label: &str) -> bool {
    driver.label_starting_with(label).is_some()
}

fn expand_filters(driver: &mut UiDriver) {
    let _ = driver.click("Filters").expect("filter toggle");
}

fn open_midi_mapping(driver: &mut UiDriver) {
    driver
        .chrome_mut()
        .settings_mut()
        .expect("settings")
        .scroll_to_heading("MIDI Mapping");
    driver.settle();
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "account menu",
            scenario: "chrome-account-signed-out",
            prepare: nothing,
            trigger: |_| "Account".to_owned(),
            secondary: false,
            inside: "Settings",
        },
        Case {
            name: "save preset",
            scenario: "chrome-preset-save",
            prepare: nothing,
            trigger: |_| "Save preset".to_owned(),
            secondary: false,
            inside: "Name",
        },
        Case {
            name: "preset browser",
            scenario: "chrome-preset-browse",
            prepare: nothing,
            trigger: |_| "Browse presets".to_owned(),
            secondary: false,
            inside: "Search presets",
        },
        Case {
            name: "input mode menu",
            scenario: "chrome-input-mode",
            prepare: nothing,
            trigger: |_| "Input mode".to_owned(),
            secondary: false,
            inside: "Left",
        },
        Case {
            name: "advanced deck",
            scenario: "chrome-gate-deck",
            prepare: nothing,
            trigger: |driver| knob_label(driver, "Gate:"),
            secondary: true,
            inside: "Release:",
        },
        Case {
            name: "tile menu",
            scenario: "chrome-tile-menu",
            prepare: nothing,
            trigger: |_| "'02 Vox AC30/6 Top Boost".to_owned(),
            secondary: true,
            inside: "Load File",
        },
        Case {
            name: "model select",
            scenario: "main-detail-model-select",
            prepare: nothing,
            trigger: |_| "Select model".to_owned(),
            secondary: false,
            inside: "AC30-6 TB Brl 3",
        },
        Case {
            name: "MIDI target picker",
            scenario: "settings-midi-picker",
            prepare: open_midi_mapping,
            trigger: |_| "Control to map".to_owned(),
            secondary: false,
            inside: "Next Preset",
        },
        Case {
            name: "filter menu",
            scenario: "browser-sort-menu",
            prepare: expand_filters,
            trigger: |_| "Trending".to_owned(),
            secondary: false,
            inside: "Newest",
        },
        Case {
            name: "filter menu at browser zoom",
            scenario: "browser-zoom-menu",
            prepare: expand_filters,
            trigger: |_| "Trending".to_owned(),
            secondary: false,
            inside: "Newest",
        },
    ]
}

fn open(driver: &mut UiDriver, case: &Case) -> String {
    let label = (case.trigger)(driver);
    let result = if case.secondary {
        driver.secondary_click(&label)
    } else {
        driver.click(&label)
    };
    let _ = result.unwrap_or_else(|error| panic!("{}: {error}", case.name));
    label
}

#[test]
fn every_popover_opens_with_a_real_click_and_closes_with_an_outside_click_and_escape() {
    for case in cases() {
        let mut driver = start(case.scenario);
        (case.prepare)(&mut driver);
        assert!(
            !popover::any_open(driver.context()),
            "{}: nothing is open at the start",
            case.name
        );
        assert!(!shows(&driver, case.inside), "{}", case.name);

        let _ = open(&mut driver, &case);
        assert!(
            shows(&driver, case.inside),
            "{}: the opening click must not close it, and {:?} must show. The screen shows {:?}",
            case.name,
            case.inside,
            driver.visible_labels()
        );
        assert!(popover::any_open(driver.context()), "{}", case.name);

        let _ = driver.click_empty_space();
        assert!(
            !shows(&driver, case.inside),
            "{}: an outside click must close it",
            case.name
        );
        assert!(!popover::any_open(driver.context()), "{}", case.name);

        let trigger_again = open(&mut driver, &case);
        assert!(shows(&driver, case.inside), "{}: reopens", case.name);
        let _ = driver.press_key(Key::Escape);
        assert!(
            !shows(&driver, case.inside),
            "{}: Escape must close it",
            case.name
        );
        assert!(!popover::any_open(driver.context()), "{}", case.name);
        assert_eq!(
            driver.focused(),
            Some(trigger_again.as_str()),
            "{}: Escape must return focus to the trigger",
            case.name
        );
    }
}

#[test]
fn one_escape_closes_only_the_midi_picker_and_not_the_settings_under_it() {
    let mut driver = start("settings-midi-picker");
    open_midi_mapping(&mut driver);
    let _ = driver.click("Control to map").expect("picker trigger");
    assert!(popover::any_open(driver.context()));

    let _ = driver.press_key(Key::Escape);
    assert!(!popover::any_open(driver.context()));
    assert!(driver.chrome().settings().is_some(), "settings stay open");

    let _ = driver.press_key(Key::Escape);
    assert!(
        driver.chrome().settings().is_none(),
        "the next Escape closes them"
    );
}

/// The menu always paints before the deck in a frame, so a test must try both opening orders.
/// With one order, the first popover in the frame is also the top one.
fn stacked(deck_on_top: bool) -> UiDriver {
    let mut driver = start("chrome-input-mode");
    if deck_on_top {
        driver.chrome_mut().open_input_mode();
        driver.settle();
        driver.chrome_mut().open_deck(DeckKind::Gate);
    } else {
        driver.chrome_mut().open_deck(DeckKind::Gate);
        driver.settle();
        driver.chrome_mut().open_input_mode();
    }
    driver.settle();
    assert_eq!(popover::open_rects(driver.context()).len(), 2);
    driver
}

#[test]
fn one_escape_closes_only_the_top_of_stacked_popovers() {
    for deck_on_top in [false, true] {
        let mut driver = stacked(deck_on_top);
        let menu = |driver: &UiDriver| driver.is_visible("Left");
        let deck = |driver: &UiDriver| driver.label_starting_with("Release:").is_some();
        assert!(menu(&driver) && deck(&driver));

        let _ = driver.press_key(Key::Escape);
        assert_eq!(
            (menu(&driver), deck(&driver)),
            (deck_on_top, !deck_on_top),
            "the newest popover closes first, and the older one stays"
        );

        let _ = driver.press_key(Key::Escape);
        assert!(!menu(&driver) && !deck(&driver));
        assert!(!popover::any_open(driver.context()));
    }
}

fn escape(pressed: bool) -> egui::Event {
    egui::Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// egui marks a press as a repeat while the key is down, so a held key sends these presses.
fn hold_escape(driver: &mut UiDriver) {
    driver.play(vec![escape(true)]);
    for _ in 0..3 {
        driver.play(vec![escape(true)]);
    }
    driver.settle();
}

#[test]
fn a_held_escape_closes_only_the_top_surface() {
    let mut driver = start("settings-midi-picker");
    open_midi_mapping(&mut driver);
    let _ = driver.click("Control to map").expect("picker trigger");

    hold_escape(&mut driver);
    assert!(!popover::any_open(driver.context()));
    assert!(
        driver.chrome().settings().is_some(),
        "the key repeat of one press must not close the settings"
    );

    driver.play(vec![escape(false)]);
    driver.play(vec![escape(true)]);
    driver.settle();
    assert!(
        driver.chrome().settings().is_none(),
        "a new press closes them"
    );
}

#[test]
fn a_held_escape_closes_only_the_top_of_stacked_popovers() {
    let mut driver = stacked(true);
    hold_escape(&mut driver);
    assert_eq!(popover::open_rects(driver.context()).len(), 1);
}

#[test]
fn a_press_in_the_top_popover_keeps_the_popover_below_it_in_either_order() {
    for deck_on_top in [false, true] {
        let mut driver = stacked(deck_on_top);
        let rects = popover::open_rects(driver.context());
        let (lower, upper) = (rects[0], rects[1]);
        let inside_upper_only = [upper.left_center(), upper.right_center()]
            .into_iter()
            .map(|edge| edge + (upper.center() - edge).normalized() * 3.0)
            .find(|pos| !lower.contains(*pos))
            .expect("a part of the upper popover is clear of the lower one");
        let _ = driver.click_at(inside_upper_only);
        assert_eq!(
            popover::open_rects(driver.context()).len(),
            2,
            "deck_on_top is {deck_on_top}"
        );
    }
}

#[test]
fn a_press_in_the_lower_popover_closes_only_the_upper_one() {
    for deck_on_top in [false, true] {
        let mut driver = stacked(deck_on_top);
        let rects = popover::open_rects(driver.context());
        let (lower, upper) = (rects[0], rects[1]);
        let inside_lower_only = [lower.left_center(), lower.right_center()]
            .into_iter()
            .map(|edge| edge + (lower.center() - edge).normalized() * 3.0)
            .find(|pos| !upper.contains(*pos))
            .expect("a part of the lower popover is clear of the upper one");
        let _ = driver.click_at(inside_lower_only);
        assert_eq!(popover::open_rects(driver.context()), vec![lower]);
    }
}

#[test]
fn a_pick_in_the_top_popover_keeps_the_popover_below_it() {
    let mut driver = stacked(false);

    let picked = driver.click("Left").expect("input mode row");
    assert!(
        picked.effects.contains(&UiEffect::ChainChanged),
        "{picked:?}"
    );
    assert!(!driver.is_visible("Left"));
    assert!(
        driver.label_starting_with("Release:").is_some(),
        "the deck stays open"
    );
}

#[test]
fn a_pick_closes_the_popover_and_returns_focus_to_its_trigger() {
    let mut driver = start("chrome-input-mode");
    let _ = driver.click("Input mode").expect("trigger");
    let _ = driver.click("Left").expect("row");
    assert!(!popover::any_open(driver.context()));
    assert_eq!(driver.focused(), Some("Input mode"));
}

#[test]
fn an_outside_press_does_not_take_focus_for_the_trigger() {
    let mut driver = start("chrome-input-mode");
    let _ = driver.click("Input mode").expect("trigger");
    let rect = popover::open_rects(driver.context())[0];
    let pos = rect.right_top() + egui::vec2(40.0, 0.0);
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    driver.play(vec![egui::Event::PointerMoved(pos)]);
    driver.play(vec![button(true)]);
    assert!(!popover::any_open(driver.context()));
    // A release clears focus anyway, so only the frames of the held press can show this.
    driver.frame();
    assert_ne!(driver.focused(), Some("Input mode"));
    driver.play(vec![button(false)]);
    driver.settle();
    assert_ne!(driver.focused(), Some("Input mode"));
}

/// Shows a popover with no trigger, for one frame of input.
fn show_bare_popover(
    context: &egui::Context,
    events: Vec<egui::Event>,
) -> Option<popover::Shown<()>> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(400.0, 400.0),
        )),
        events,
        ..Default::default()
    };
    let mut shown = None;
    let mut output = context.run_ui(input, |ui| {
        shown = Some(
            Popover::new(
                egui::Id::unique("bare-popover"),
                Placement::At(egui::pos2(10.0, 10.0)),
                egui::vec2(50.0, 50.0),
                Surface::edged(4.0),
            )
            .show(ui, |_, _| None::<()>),
        );
    });
    output.textures_delta.clear();
    shown
}

#[test]
fn a_popover_takes_only_its_own_area_from_the_first_frame() {
    for case in cases() {
        let mut driver = start(case.scenario);
        (case.prepare)(&mut driver);
        let _ = open(&mut driver, &case);
        let first = popover::open_rects(driver.context())[0];
        for _ in 0..40 {
            driver.frame();
        }
        assert_eq!(
            popover::open_rects(driver.context())[0],
            first,
            "{}: the area must not change after the panel paints",
            case.name
        );
    }
}

#[test]
fn a_press_just_beside_the_account_menu_closes_it_at_once() {
    let mut driver = start("chrome-account-signed-out");
    let _ = driver.click("Account").expect("open");
    let panel = popover::open_rects(driver.context())[0];
    assert_eq!(panel.size(), egui::vec2(190.0, 94.0));
    let _ = driver.click_at(panel.left_center() - egui::vec2(20.0, 0.0));
    assert!(!popover::any_open(driver.context()));
}

#[test]
fn a_panel_past_the_window_edge_keeps_its_own_area() {
    let context = egui::Context::default();
    let frame = |events| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 400.0),
            )),
            events,
            ..Default::default()
        };
        let mut shown = None;
        let mut output = context.run_ui(input, |ui| {
            shown = Some(
                Popover::new(
                    egui::Id::unique("edge-popover"),
                    Placement::At(egui::pos2(350.0, 10.0)),
                    egui::vec2(100.0, 50.0),
                    Surface::edged(4.0),
                )
                .show(ui, |_, _| None::<()>),
            );
        });
        output.textures_delta.clear();
        shown.expect("shown")
    };
    for _ in 0..40 {
        let _ = frame(Vec::new());
    }
    let expected = egui::Rect::from_min_size(egui::pos2(350.0, 10.0), egui::vec2(100.0, 50.0));
    assert_eq!(popover::open_rects(&context), vec![expected]);
    let pos = egui::pos2(100.0, 30.0);
    let shown = frame(vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    assert_eq!(shown.dismissed, Some(DismissReason::Outside));
}

#[test]
fn the_opening_frame_ignores_a_press_outside_and_the_next_frame_does_not() {
    let press = |pressed| {
        vec![
            egui::Event::PointerMoved(egui::pos2(300.0, 300.0)),
            egui::Event::PointerButton {
                pos: egui::pos2(300.0, 300.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    let context = egui::Context::default();
    let opening = show_bare_popover(&context, press(true)).expect("shown");
    assert_eq!(opening.dismissed, None, "the press that opens it stays");
    let release = show_bare_popover(&context, press(false)).expect("shown");
    assert_eq!(release.dismissed, None);
    let next = show_bare_popover(&context, press(true)).expect("shown");
    assert_eq!(next.dismissed, Some(DismissReason::Outside));
}

#[test]
fn a_click_that_a_frame_batches_opens_every_popover() {
    for case in cases() {
        let mut driver = start(case.scenario);
        (case.prepare)(&mut driver);
        let label = (case.trigger)(&driver);
        let pos = driver
            .controls()
            .iter()
            .find(|control| control.label == label && control.clickable)
            .unwrap_or_else(|| panic!("{}: no control {label}", case.name))
            .rect
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: if case.secondary {
                egui::PointerButton::Secondary
            } else {
                egui::PointerButton::Primary
            },
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        driver.play(vec![egui::Event::PointerMoved(pos)]);
        driver.play(vec![button(true), button(false)]);
        driver.settle();
        assert!(shows(&driver, case.inside), "{}", case.name);
    }
}

#[test]
fn a_second_click_on_a_zoomed_trigger_toggles_the_menu_closed() {
    let mut driver = start("browser-zoom-menu");
    expand_filters(&mut driver);
    let chip = driver
        .controls()
        .iter()
        .find(|control| control.label == "Trending")
        .expect("sort chip")
        .rect
        .center();
    let _ = driver.click_at(chip);
    assert!(popover::any_open(driver.context()));
    let _ = driver.click_at(chip);
    assert!(!popover::any_open(driver.context()));
}

#[test]
fn a_second_click_on_the_trigger_toggles_the_popover_closed() {
    for label in ["Account", "Save preset", "Browse presets"] {
        let mut driver = start("chrome-account-signed-out");
        let _ = driver.click(label).expect("open");
        assert!(popover::any_open(driver.context()), "{label}");
        let _ = driver.click(label).expect("close");
        assert!(!popover::any_open(driver.context()), "{label}");
    }
}

#[test]
fn escape_with_a_form_popup_open_leaves_the_settings_open() {
    let mut driver = start("settings-midi-picker");
    open_midi_mapping(&mut driver);
    let _ = driver.click("MIDI channel").expect("channel select");
    assert!(driver.is_visible("Omni"), "{:?}", driver.visible_labels());
    let _ = driver.press_key(Key::Escape);
    assert!(driver.chrome().settings().is_some());
    assert!(!driver.is_visible("Omni"));
}

#[test]
fn an_absent_label_fails_clearly_and_plays_no_input() {
    let mut driver = start("chrome-account-signed-out");
    let Err(DriverError::Absent { label, visible }) = driver.click("No such control") else {
        panic!("an absent label must be an error");
    };
    assert_eq!(label, "No such control");
    assert!(visible.iter().any(|label| label == "Account"));
    assert!(!popover::any_open(driver.context()));
}

#[test]
fn a_control_under_a_popover_cannot_take_a_click() {
    let mut driver = start("chrome-preset-browse");
    let before = driver.controls().to_vec();
    let _ = driver.click("Browse presets").expect("open");
    let rect = popover::open_rects(driver.context())[0];
    let covered = before
        .iter()
        .find(|control| control.clickable && rect.contains(control.rect.center()))
        .unwrap_or_else(|| panic!("nothing is under {rect:?}: {before:?}"));
    let result = driver.click(&covered.label);
    assert!(
        matches!(result, Err(DriverError::Covered { .. })),
        "{}: {result:?}",
        covered.label
    );
}

#[test]
fn a_press_inside_that_ends_outside_keeps_the_popover_open() {
    let mut driver = start("chrome-preset-browse");
    let _ = driver.click("Browse presets").expect("open");
    let rect = popover::open_rects(driver.context())[0];
    let _ = driver.drag(
        rect.center(),
        rect.right_bottom() + eframe::egui::vec2(40.0, 40.0),
    );
    assert!(
        popover::any_open(driver.context()),
        "a selection drag must not close it"
    );
}

#[test]
fn a_press_on_another_trigger_closes_the_popover_and_its_release_opens_the_other() {
    let mut driver = start("chrome-account-signed-out");
    let _ = driver.click("Account").expect("account");
    assert_eq!(driver.chrome().overlay(), Overlay::Account);
    let _ = driver.click("Browse presets").expect("presets");
    assert_eq!(driver.chrome().overlay(), Overlay::PresetBrowser);
    assert_eq!(popover::open_rects(driver.context()).len(), 1);
}

#[test]
fn the_filter_menu_follows_the_browser_zoom() {
    let row_width = |id: &str| {
        let mut driver = start(id);
        expand_filters(&mut driver);
        let _ = driver.click("Trending").expect("sort chip");
        driver
            .controls()
            .iter()
            .find(|control| control.label == "Newest")
            .expect("menu row")
            .rect
            .width()
    };
    let plain = row_width("browser-sort-menu");
    let zoomed = row_width("browser-zoom-menu");
    assert!(
        (zoomed - plain / 1.6).abs() < 1.0,
        "a zoomed body counter-scales its layer by 1 / 1.6, and the menu must follow: {plain} and {zoomed}"
    );
    let mut driver = start("browser-zoom-menu");
    expand_filters(&mut driver);
    let chip = driver
        .controls()
        .iter()
        .find(|control| control.label == "Trending")
        .expect("sort chip")
        .rect;
    let _ = driver.click("Trending").expect("sort chip");
    let rect = popover::open_rects(driver.context())[0];
    assert!(
        (rect.left() - chip.left()).abs() < 1.0,
        "the panel starts under its chip in window coordinates: {rect:?} and {chip:?}"
    );
    assert!(rect.top() >= chip.bottom(), "{rect:?} and {chip:?}");
}
