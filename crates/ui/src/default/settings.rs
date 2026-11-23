//! Full-window settings takeover, tab selection, focus, and scrolling.

use std::sync::Arc;

use eframe::egui;

use crate::{accessibility::FocusManager, backend::UiBackend};

use super::{PluginSettingsController, SystemSettingsController, forms};

/// Available settings pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    /// Processor, preferences, MIDI mapping, presets, and diagnostics.
    Plugin,
    /// Standalone audio device and MIDI hardware.
    System,
}

/// Mounted settings takeover with independent page controllers.
pub struct SettingsController {
    backend: Arc<dyn UiBackend>,
    standalone: bool,
    tab: SettingsTab,
    closed: bool,
    focus: FocusManager,
    scroll_target: Option<String>,
    scroll_offset: Option<f32>,
    scroll_center: bool,
    plugin: PluginSettingsController,
    system: SystemSettingsController,
}

impl SettingsController {
    /// Creates a takeover. Hosted mode always selects the plugin page.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>, standalone: bool, tab: SettingsTab) -> Self {
        Self {
            plugin: PluginSettingsController::new(backend.clone()),
            system: SystemSettingsController::new(backend.clone()),
            backend,
            standalone,
            tab: if standalone { tab } else { SettingsTab::Plugin },
            closed: false,
            focus: FocusManager::default(),
            scroll_target: None,
            scroll_offset: None,
            scroll_center: false,
        }
    }

    /// Whether the standalone tab bar is available.
    #[must_use]
    pub const fn standalone(&self) -> bool {
        self.standalone
    }

    /// Current page.
    #[must_use]
    pub const fn tab(&self) -> SettingsTab {
        self.tab
    }

    /// Selects a page, respecting hosted mode.
    pub fn set_tab(&mut self, tab: SettingsTab) {
        self.tab = if self.standalone {
            tab
        } else {
            SettingsTab::Plugin
        };
    }

    /// Plugin page controller.
    #[must_use]
    pub const fn plugin(&self) -> &PluginSettingsController {
        &self.plugin
    }

    /// Mutable plugin page controller.
    pub fn plugin_mut(&mut self) -> &mut PluginSettingsController {
        &mut self.plugin
    }

    /// Standalone device page controller.
    #[must_use]
    pub const fn system(&self) -> &SystemSettingsController {
        &self.system
    }

    /// Mutable standalone device page controller.
    pub fn system_mut(&mut self) -> &mut SystemSettingsController {
        &mut self.system
    }

    /// Requests that a section heading be placed at the top of the page.
    pub fn scroll_to_heading(&mut self, heading: &str) {
        self.reveal_heading(heading);
        self.scroll_center = false;
        self.scroll_target = Some(heading.to_owned());
    }

    /// Requests that a section heading be centered in the viewport.
    pub fn scroll_to_heading_centered(&mut self, heading: &str) {
        self.reveal_heading(heading);
        self.scroll_center = true;
        self.scroll_target = Some(heading.to_owned());
    }

    /// A heading in a closed part of the page has no place to scroll to.
    fn reveal_heading(&self, heading: &str) {
        if heading == "MIDI Inputs" {
            self.system.set_advanced_open(true);
        }
    }

    /// Current scroll request.
    #[must_use]
    pub fn scroll_target(&self) -> Option<&str> {
        self.scroll_target.as_deref()
    }

    /// Focused control identity, or none before the first Tab.
    #[must_use]
    pub fn focused(&self) -> Option<&String> {
        self.focus.focused()
    }

    /// Focus order confined to the settings takeover.
    #[must_use]
    pub fn focus_order(&self) -> Vec<String> {
        let mut order = vec!["close-settings".to_owned()];
        if self.standalone {
            order.extend(["tab-system".to_owned(), "tab-plugin".to_owned()]);
        }
        order.extend(match self.tab {
            SettingsTab::Plugin => [
                "show-hints".to_owned(),
                "calibration".to_owned(),
                "oversampling".to_owned(),
                "midi-target".to_owned(),
                "open-presets".to_owned(),
            ]
            .into_iter()
            .collect::<Vec<_>>(),
            SettingsTab::System => [
                "hear-yourself".to_owned(),
                "input-device".to_owned(),
                "output-device".to_owned(),
                "buffer-size".to_owned(),
                "sample-rate".to_owned(),
                "midi-inputs".to_owned(),
            ]
            .into_iter()
            .collect::<Vec<_>>(),
        });
        order
    }

    /// Advances or reverses the takeover-local Tab cycle.
    pub fn tab_focus(&mut self, shift: bool) -> bool {
        self.focus.tab(&self.focus_order(), shift)
    }

    /// Closes the takeover and clears its focus.
    pub fn close(&mut self) {
        self.closed = true;
        self.focus.clear();
    }

    /// Whether the takeover requested closure.
    #[must_use]
    pub const fn closed(&self) -> bool {
        self.closed
    }

    /// Backend used by the mounted pages.
    #[must_use]
    pub fn backend(&self) -> &Arc<dyn UiBackend> {
        &self.backend
    }

    /// Renders the takeover at the supplied design height and returns whether it closed.
    pub fn show(&mut self, ui: &mut egui::Ui, height: f32) -> bool {
        if self.closed {
            return true;
        }
        if super::popover::escape_reaches_surface(ui.ctx()) {
            self.close();
            return true;
        }
        if ui.input(|input| input.key_pressed(egui::Key::Tab)) {
            let shift = ui.input(|input| input.modifiers.shift);
            self.tab_focus(shift);
        }
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, egui::Color32::BLACK);
        let (left, width) = forms::column_bounds(ui.available_width());
        ui.horizontal(|ui| {
            ui.add_space(left);
            ui.allocate_ui_with_layout(
                egui::vec2(width, height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    let target = self.scroll_target.clone();
                    let pending_offset = self.scroll_offset.take();
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt("settings-takeover-scroll")
                        .animated(false);
                    if let Some(offset) = pending_offset {
                        scroll = scroll.vertical_scroll_offset(offset);
                    }
                    ui.style_mut().spacing.scroll.fade.strength = 0.0;
                    let output = scroll.show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if let Some(target) = target.clone() {
                            ui.data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::unique("settings-scroll-target"),
                                    target,
                                );
                            });
                        }
                        ui.add_space(28.0);
                        let header_width = ui.available_width();
                        let (header, _) = ui.allocate_exact_size(
                            egui::vec2(header_width, 28.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().text(
                            header.left_center(),
                            egui::Align2::LEFT_CENTER,
                            "Settings",
                            egui::FontId::new(22.0, egui::FontFamily::Name("arimo-bold".into())),
                            egui::Color32::WHITE,
                        );
                        let close = egui::Rect::from_min_size(
                            egui::pos2(header.right() - 28.0, header.top()),
                            egui::Vec2::splat(28.0),
                        );
                        let close_response = ui.interact(
                            close,
                            ui.scope_id().with("close-settings"),
                            egui::Sense::click(),
                        );
                        close_response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, true, "Close settings")
                        });
                        let close_stroke = egui::Stroke::new(1.667, egui::Color32::WHITE);
                        for (start, end) in [
                            (egui::vec2(-5.0, -5.0), egui::vec2(5.0, 5.0)),
                            (egui::vec2(-5.0, 5.0), egui::vec2(5.0, -5.0)),
                        ] {
                            ui.painter().line_segment(
                                [close.center() + start, close.center() + end],
                                close_stroke,
                            );
                        }
                        if close_response.clicked() {
                            self.close();
                        }
                        ui.add_space(20.0);
                        if self.standalone {
                            let tab_width = ui.available_width();
                            let (tabs, _) = ui.allocate_exact_size(
                                egui::vec2(tab_width, 42.0),
                                egui::Sense::hover(),
                            );
                            let painter = ui.painter_at(tabs);
                            painter.line_segment(
                                [tabs.left_bottom(), tabs.right_bottom()],
                                egui::Stroke::new(1.0, crate::theme::palette::PANEL_EDGE),
                            );
                            for (index, tab, label) in [
                                (0, SettingsTab::System, "System Settings"),
                                (1, SettingsTab::Plugin, "Plugin Settings"),
                            ] {
                                let left = tabs.left() + tab_width * index as f32 / 2.0;
                                let cell = egui::Rect::from_min_size(
                                    egui::pos2(left, tabs.top()),
                                    egui::vec2(tab_width / 2.0, tabs.height()),
                                );
                                let response = ui.interact(
                                    cell,
                                    ui.scope_id().with(if tab == SettingsTab::System {
                                        "tab-system"
                                    } else {
                                        "tab-plugin"
                                    }),
                                    egui::Sense::click(),
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::Button, true, label)
                                });
                                let selected = self.tab == tab;
                                let color = if selected {
                                    egui::Color32::WHITE
                                } else {
                                    crate::theme::palette::TEXT_SUBTLE
                                };
                                let font = egui::FontId::new(
                                    14.0,
                                    egui::FontFamily::Name("arimo-bold".into()),
                                );
                                let galley = ui.ctx().fonts_mut(|fonts| {
                                    fonts.layout_no_wrap(label.to_owned(), font, color)
                                });
                                let icon_width = if tab == SettingsTab::System {
                                    24.0
                                } else {
                                    0.0
                                };
                                let content_width = galley.size().x + icon_width;
                                let content_left = cell.center().x - content_width / 2.0;
                                let content_center = egui::pos2(content_left, cell.top() + 20.0);
                                if tab == SettingsTab::System {
                                    let icon = egui::Rect::from_center_size(
                                        content_center + egui::vec2(8.0, 0.0),
                                        egui::Vec2::splat(16.0),
                                    );
                                    let scale = icon.width() / 24.0;
                                    let point = |x: f32, y: f32| {
                                        icon.left_top() + egui::vec2(x * scale, y * scale)
                                    };
                                    let stroke = egui::Stroke::new(2.0 * scale, color);
                                    painter.rect_stroke(
                                        egui::Rect::from_min_max(
                                            point(4.0, 5.0),
                                            point(20.0, 16.0),
                                        ),
                                        2.0 * scale,
                                        stroke,
                                        egui::StrokeKind::Inside,
                                    );
                                    for (start, end) in [
                                        (point(4.0, 16.0), point(20.0, 16.0)),
                                        (point(20.0, 16.0), point(22.0, 19.0)),
                                        (point(22.0, 19.0), point(2.0, 19.0)),
                                        (point(2.0, 19.0), point(4.0, 16.0)),
                                    ] {
                                        painter.line_segment([start, end], stroke);
                                    }
                                }
                                painter.galley(
                                    egui::pos2(content_left + icon_width, cell.top() + 12.0),
                                    galley,
                                    color,
                                );
                                if selected {
                                    painter.rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(cell.left(), cell.bottom() - 2.0),
                                            cell.right_bottom(),
                                        ),
                                        0.0,
                                        egui::Color32::WHITE,
                                    );
                                }
                                if response.clicked() {
                                    self.set_tab(tab);
                                }
                            }
                            ui.add_space(28.0);
                        }
                        if self.closed {
                            return;
                        }
                        match self.tab {
                            SettingsTab::Plugin => self.plugin.show(ui),
                            SettingsTab::System => self.system.show(ui),
                        }
                        ui.add_space(40.0);
                    });
                    if target.is_some() {
                        let heading_top = ui.data(|data| {
                            data.get_temp::<f32>(egui::Id::unique("settings-scroll-heading-top"))
                        });
                        if let Some(heading_top) = heading_top {
                            let target_y = if self.scroll_center {
                                output.inner_rect.top() + height / 2.0
                            } else {
                                output.inner_rect.top() + 28.0
                            };
                            let offset = output.state.clamped_offset().y + heading_top - target_y;
                            let max_offset =
                                (output.content_size.y - output.inner_rect.height()).max(0.0);
                            self.scroll_offset = Some(offset.clamp(0.0, max_offset));
                            self.scroll_target = None;
                            ui.ctx().request_repaint();
                        }
                    }
                },
            );
        });
        self.closed
    }
}
