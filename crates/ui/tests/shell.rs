//! Design-space, chrome, font, clock, theme, and icon foundation tests.

use std::{cell::Cell, rc::Rc};

use ronie_ui::{
    clock::UiClock,
    fonts::{FontFace, Fonts, css_baseline, normal_line_height},
    icons::{IconCache, IconId},
    shell::{BrowserLayout, Chrome, LayoutTransform, UiPreferences, ViewMode, max_start_scale},
    theme::Theme,
};

#[test]
fn default_preferences_select_reference_mode_and_hint_chrome() {
    let preferences = UiPreferences::default();
    assert_eq!(preferences.view_mode, ViewMode::ReferenceDefault);
    assert!(preferences.hints_enabled);
    assert_eq!(preferences.editor_extra_height, 36);
    assert_eq!(Chrome::from(&preferences).design_height(), 614);
}

#[test]
fn chrome_grows_instead_of_squishing_the_core() {
    let chrome = Chrome {
        banner: true,
        hints: true,
    };
    assert_eq!(chrome.extra_height(), 80);
    assert_eq!(chrome.design_height(), 658);
    assert_eq!(chrome.window_size(1.5), (1536, 987));

    let layout = LayoutTransform::fit(1536, 987, chrome);
    assert_eq!(layout.scale, 1.5);
    assert_eq!(layout.x, 0);
    assert_eq!(layout.y, 0);
}

#[test]
fn reference_sizes_stay_centered_top_anchored_and_aspect_locked() {
    let chrome = Chrome::default();
    let one = LayoutTransform::fit(1024, 578, chrome);
    assert_eq!((one.scale, one.x, one.y), (1.0, 0, 0));
    let two = LayoutTransform::fit(2048, 1156, chrome);
    assert_eq!((two.scale, two.x, two.y), (2.0, 0, 0));
    let letterboxed = LayoutTransform::fit(1200, 578, chrome);
    assert_eq!(
        (letterboxed.scale, letterboxed.x, letterboxed.y),
        (1.0, 88, 0)
    );
    let squeezed = LayoutTransform::fit(512, 289, chrome);
    assert_eq!((squeezed.scale, squeezed.x, squeezed.y), (0.5, 0, 0));
}

#[test]
fn standalone_start_scale_accounts_for_the_native_title_bar() {
    let chrome = Chrome {
        banner: true,
        hints: true,
    };
    assert_eq!(max_start_scale(2048, 1356, chrome), 2.0);
    assert_eq!(max_start_scale(1280, 900, chrome), 1.25);
    assert_eq!(max_start_scale(640, 400, chrome), 1.0);
}

#[test]
fn browser_counter_scales_at_reference_zoom_levels() {
    for (zoom, width, height) in [(1.3, 1560, 910), (1.6, 1920, 1120)] {
        let layout = BrowserLayout::new(1200, 700, zoom);
        assert_eq!(layout.scale, 1.0 / zoom);
        assert_eq!(layout.content_width, width);
        assert_eq!(layout.content_height, height);
    }
}

#[test]
fn embedded_fonts_and_css_metrics_have_stable_fallbacks() {
    assert_eq!(Fonts::embedded().len(), 6);
    assert!(!Fonts::bytes(FontFace::ArimoRegular).is_empty());
    assert!(!Fonts::bytes(FontFace::ArimoBoldItalic).is_empty());
    assert!(!Fonts::bytes(FontFace::RobotoMonoRegular).is_empty());
    assert_eq!(normal_line_height(14.0), 16);
    assert_eq!(css_baseline(14.0, 18.0), 14.0);
    assert_eq!(css_baseline(110.0, 110.0), 93.0);
}

#[test]
fn one_clock_polls_at_30hz_and_hidden_windows_only_wake_at_4hz() {
    let calls = Rc::new(Cell::new(0));
    let mut clock = UiClock::default();
    let observed = Rc::clone(&calls);
    clock.add_listener(move || observed.set(observed.get() + 1));
    assert_eq!(clock.period_ms(), 33);
    clock.tick(false);
    assert_eq!(calls.get(), 0);
    assert_eq!(clock.period_ms(), 250);
    clock.tick(false);
    assert_eq!(calls.get(), 0);
    clock.wake();
    assert_eq!(clock.period_ms(), 33);
    clock.tick(true);
    assert_eq!(calls.get(), 1);
    clock.tick(true);
    assert_eq!(calls.get(), 2);
}

#[test]
fn theme_and_icons_use_neutral_cached_assets() {
    let theme = Theme::default();
    assert_eq!(theme.black, [0, 0, 0, 255]);
    assert_eq!(theme.surface, [0x15, 0x15, 0x17, 0xff]);
    assert_eq!(theme.panel_corner, 14.0);
    let mut icons = IconCache::default();
    let first = icons.get(IconId::Back) as *const _;
    let second = icons.get(IconId::Back) as *const _;
    assert_eq!(first, second);
    assert!(icons.get(IconId::Back).svg.contains("<svg"));
}

#[test]
fn real_default_scene_has_no_unnamed_focus_stops() {
    let report = ronie_ui::native_scenario::run_logic_selftest();
    assert!(report["accesskit_nodes"].as_u64().unwrap_or(0) >= 20);
    assert_eq!(report["accesskit_unnamed_focusable"], 0);
    assert_eq!(report["accesskit_passed"], true);
}

#[test]
fn detail_scene_has_no_unnamed_focus_stops() {
    let scenario =
        ronie_ui::testbed::load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "main-detail")
            .expect("detail scenario");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario));
    let mut chrome = ronie_ui::default::ChromeController::new(backend, Vec::new(), false, true);
    chrome.open_detail("blk-1").expect("open detail");
    let report = ronie_ui::shell::audit_chrome_accessibility(&mut chrome);
    assert_eq!(report["unnamed_focusable"], 0);
    assert_eq!(report["passed"], true);
}

#[test]
fn eq_scene_has_no_unnamed_focus_stops() {
    let scenario =
        ronie_ui::testbed::load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == "main-detail-eq-sliders")
            .expect("EQ scenario");
    let backend = std::sync::Arc::new(ronie_ui::backend::MockBackend::from_scenario(&scenario));
    let mut chrome = ronie_ui::default::ChromeController::new(backend, Vec::new(), false, true);
    chrome.open_detail("blk-1").expect("open detail");
    chrome
        .detail_mut()
        .expect("detail")
        .set_view(ronie_ui::default::BlockView::Eq);
    let report = ronie_ui::shell::audit_chrome_accessibility(&mut chrome);
    assert_eq!(report["unnamed_focusable"], 0, "{report}");
    assert_eq!(report["passed"], true);
}

#[test]
fn embedded_fonts_shape_real_text_in_egui() {
    let context = eframe::egui::Context::default();
    Fonts::install(&context);
    let mut output = context.run_ui(eframe::egui::RawInput::default(), |_| {});
    output.textures_delta.clear();
    let galley = context.fonts_mut(|fonts| {
        fonts.layout_no_wrap(
            "Ronie".to_owned(),
            eframe::egui::FontId::new(14.0, eframe::egui::FontFamily::Proportional),
            eframe::egui::Color32::WHITE,
        )
    });
    assert!(galley.size().x > 0.0);
    assert!(galley.size().y > 0.0);
}

#[test]
fn responsive_zoom_fits_the_design_box_on_any_display_scale() {
    use ronie_ui::shell::responsive_zoom;
    let chrome = Chrome {
        banner: true,
        hints: true,
    };
    let design_height = chrome.design_height() as f32;
    for native in [1.0, 1.25, 1.5, 2.0] {
        assert_eq!(responsive_zoom(1024.0, design_height, native, chrome), 1.0);
        assert_eq!(
            responsive_zoom(1536.0, 1.5 * design_height, native, chrome),
            1.5
        );
    }
}

#[test]
fn responsive_zoom_never_makes_the_layout_smaller_than_the_design_box() {
    use ronie_ui::shell::responsive_zoom;
    let chrome = Chrome {
        banner: true,
        hints: true,
    };
    for native in [1.0, 1.25, 1.5, 1.75, 2.0] {
        for width in (512..2800).step_by(37) {
            for height in (289..1600).step_by(41) {
                let zoom = responsive_zoom(width as f32, height as f32, native, chrome);
                assert!(
                    width as f32 / zoom >= 1024.0,
                    "{width}x{height} at {native}"
                );
                assert!(
                    height as f32 / zoom >= chrome.design_height() as f32 - 0.01,
                    "{width}x{height} at {native}"
                );
            }
        }
    }
}

#[test]
fn responsive_zoom_snaps_so_a_small_drag_keeps_the_font_atlas() {
    use ronie_ui::shell::responsive_zoom;
    let chrome = Chrome::default();
    assert_eq!(
        responsive_zoom(1100.0, 1000.0, 1.0, chrome),
        responsive_zoom(1120.0, 1000.0, 1.0, chrome)
    );
}

#[test]
fn a_window_with_no_real_size_gets_the_design_zoom() {
    let chrome = Chrome {
        banner: false,
        hints: false,
    };
    assert_eq!(
        ronie_ui::shell::responsive_zoom(f32::NAN, f32::NAN, 1.0, chrome),
        1.0
    );
}

proptest::proptest! {
    #[test]
    fn responsive_zoom_is_always_finite_and_in_range(
        width in proptest::num::f32::ANY,
        height in proptest::num::f32::ANY,
        native in proptest::num::f32::ANY,
        banner: bool,
        hints: bool,
    ) {
        let zoom = ronie_ui::shell::responsive_zoom(width, height, native, Chrome { banner, hints });
        proptest::prop_assert!(zoom.is_finite());
        proptest::prop_assert!((0.25..=32.0).contains(&zoom));
    }

    #[test]
    fn fire_ramp_accepts_any_position(position in proptest::num::f32::ANY) {
        let color = ronie_ui::theme::palette::fire_ramp(position);
        proptest::prop_assert_eq!(color.a(), 255);
    }
}

/// Plays one native frame. egui-winit gives the window size in points of the current zoom.
fn zoom_frame(
    context: &eframe::egui::Context,
    window: eframe::egui::Vec2,
    chrome: Chrome,
    events: Vec<eframe::egui::Event>,
    mut body: impl FnMut(&mut eframe::egui::Ui),
) {
    use eframe::egui;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            window / context.zoom_factor(),
        )),
        events,
        ..Default::default()
    };
    let mut output = context.run_ui(input, |ui| {
        ronie_ui::shell::refit_zoom(ui.ctx(), chrome);
        body(ui);
    });
    output.textures_delta.clear();
}

#[test]
fn live_zoom_is_exact_on_the_first_frame_and_stable_after_it() {
    use eframe::egui;
    let chrome = Chrome {
        banner: true,
        hints: true,
    };
    for scale in [0.5, 1.0, 1.5, 2.0, 2.5] {
        let context = egui::Context::default();
        let window = egui::vec2(1024.0, chrome.design_height() as f32) * scale;
        for frame in 0..5 {
            let mut seen = None;
            zoom_frame(&context, window, chrome, Vec::new(), |ui| {
                seen = Some(ui.ctx().zoom_factor());
            });
            assert_eq!(seen, Some(scale), "scale {scale}, frame {frame}");
        }
    }
}

#[test]
fn live_zoom_stays_correct_while_the_window_shrinks_and_grows() {
    use eframe::egui;
    let chrome = Chrome::default();
    let context = egui::Context::default();
    for (width, height, zoom) in [
        (2048.0, 1156.0, 2.0),
        (1024.0, 578.0, 1.0),
        (512.0, 289.0, 0.5),
        (1536.0, 867.0, 1.5),
    ] {
        zoom_frame(
            &context,
            egui::vec2(width, height),
            chrome,
            Vec::new(),
            |_| {},
        );
        assert_eq!(context.zoom_factor(), zoom, "{width}x{height}");
    }
}

#[test]
fn a_zoom_change_keeps_keyboard_focus_and_typed_text() {
    use eframe::egui;
    let chrome = Chrome::default();
    let context = egui::Context::default();
    let mut text = String::new();
    let mut field_id = None;
    let mut run = |window: egui::Vec2, events: Vec<egui::Event>, focus: bool| {
        zoom_frame(&context, window, chrome, events, |ui| {
            let response = ui.text_edit_singleline(&mut text);
            field_id = Some(response.id);
            if focus {
                response.request_focus();
            }
        });
    };
    let typed = |key: &str| vec![egui::Event::Text(key.to_owned())];
    let design = egui::vec2(1024.0, 578.0);
    run(design, Vec::new(), true);
    run(design, typed("a"), false);
    // The window grows on the same frames as the keys, as a banner or a drag can do.
    run(design * 1.5, typed("b"), false);
    run(design * 2.0, typed("c"), false);
    run(design * 2.0, Vec::new(), false);
    assert_eq!(text, "abc");
    let id = field_id.expect("text field");
    assert!(context.memory(|memory| memory.has_focus(id)));
}

#[test]
fn embedded_header_artwork_is_square_and_high_resolution() {
    let image = image::load_from_memory(include_bytes!("../../../assets/ronnie-header.png"))
        .expect("embedded header artwork");
    assert_eq!(image.width(), image.height());
    assert!(image.width() >= 512);
}
