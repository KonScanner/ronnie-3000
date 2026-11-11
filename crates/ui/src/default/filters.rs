//! Browser filter menus and profile-parked controls.

pub use crate::services::catalog::FilterMenu;

use eframe::egui::{self, Color32, Rect, Sense};
use ronie_core::Taxonomy;

use super::popover::{Placement, Popover, Surface, Trigger};
use crate::services::{
    catalog::{CatalogStore, Profile},
    session::SessionStatus,
};

/// Most creator avatars that one filter bar holds as textures.
const MAX_CREATOR_TEXTURES: usize = 1024;

/// Filter bar state preserved with the browser.
#[derive(Default)]
pub struct FilterBar {
    expanded: bool,
    open: Option<FilterMenu>,
    menu_x: f32,
    menu_trigger: Option<Trigger>,
    profile_avatar: Option<egui::TextureHandle>,
    creator_avatars: std::collections::BTreeMap<String, egui::TextureHandle>,
    search: String,
    scroll_menu: Option<FilterMenu>,
    menu_scroll: f32,
    /// Menu and search text of the newest taxonomy request.
    taxonomy_asked: Option<(FilterMenu, String)>,
}

impl FilterBar {
    /// Whether the secondary filter row is expanded.
    #[must_use]
    pub const fn expanded(&self) -> bool {
        self.expanded
    }

    /// Currently open menu.
    #[must_use]
    pub const fn open_menu(&self) -> Option<FilterMenu> {
        self.open
    }

    /// Sets whether the secondary filter row is expanded.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Opens or closes one filter menu.
    pub fn set_open_menu(&mut self, menu: Option<FilterMenu>) {
        if self.open != menu {
            self.search.clear();
        }
        self.open = menu;
    }

    /// Supplies the session avatar used by the profile chip.
    pub fn set_profile_avatar(&mut self, avatar: egui::TextureHandle) {
        self.profile_avatar = Some(avatar);
    }

    /// Supplies one creator avatar for taxonomy menu rows.
    pub fn set_creator_avatar(&mut self, name: &str, avatar: egui::TextureHandle) {
        // A server chooses the names, so the map has a limit.
        if self.creator_avatars.len() < MAX_CREATOR_TEXTURES
            || self.creator_avatars.contains_key(name)
        {
            self.creator_avatars.insert(name.to_owned(), avatar);
        }
    }

    /// Whether one creator row has a decoded avatar.
    #[must_use]
    pub fn has_creator_avatar(&self, name: &str) -> bool {
        self.creator_avatars.contains_key(name)
    }

    /// Forgets the newest taxonomy request, because its answer cannot arrive.
    pub(super) fn forget_taxonomy_request(&mut self) {
        self.taxonomy_asked = None;
    }

    /// Removes what belongs to the previous session.
    pub(super) fn drop_session_state(&mut self) {
        self.forget_taxonomy_request();
        self.set_open_menu(None);
        self.profile_avatar = None;
    }

    /// Renders filter controls and updates the shared query.
    pub fn show_at(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        store: &mut CatalogStore,
        session: SessionStatus,
        scale: f32,
    ) {
        self.menu_trigger = None;
        let mut x = rect.left();
        if session.signed_in() {
            let profile_locked = store.query().profile != Profile::None;
            let filters_active = {
                let query = store.query();
                query.sort.is_some()
                    || !query.format.is_empty()
                    || !query.tags.is_empty()
                    || !query.makes.is_empty()
                    || !query.creators.is_empty()
                    || query.calibrated
            };
            let toggle_width = if !self.expanded && filters_active {
                50.0 * scale
            } else {
                38.0 * scale
            };
            let toggle_rect = Rect::from_min_size(
                egui::pos2(x, rect.top()),
                egui::vec2(toggle_width, 38.0 * scale),
            );
            let toggle = chip(
                ui,
                toggle_rect,
                "",
                !self.expanded && filters_active,
                false,
                "Filters",
                scale,
            );
            draw_filter_icon(ui, toggle_rect, self.expanded, filters_active, scale);
            if profile_locked && !self.expanded {
                ui.painter_at(toggle_rect).rect_filled(
                    toggle_rect,
                    19.0 * scale,
                    crate::theme::palette::DISABLED_VEIL,
                );
            }
            if toggle.clicked() && !(profile_locked && !self.expanded) {
                self.expanded = !self.expanded;
            }
            x += toggle_width + 10.0 * scale;
            if !self.expanded {
                ui.painter().line_segment(
                    [
                        egui::pos2(x, rect.top() + 7.0 * scale),
                        egui::pos2(x, rect.top() + 31.0 * scale),
                    ],
                    egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                );
                x += 11.0 * scale;
            }
            if self.expanded {
                let sort_label = match store.query().sort {
                    Some(ronie_core::ToneSort::BestMatch) => "Best match",
                    Some(ronie_core::ToneSort::Trending) => "Trending",
                    Some(ronie_core::ToneSort::Popular) => "Popular",
                    Some(ronie_core::ToneSort::Newest) => "Newest",
                    Some(ronie_core::ToneSort::Oldest) => "Oldest",
                    None if store.query().text.is_empty() => "Trending",
                    None => "Best match",
                };
                let query = store.query();
                let entries = [
                    (
                        FilterMenu::Sort,
                        sort_label.to_owned(),
                        query.sort.is_some(),
                    ),
                    (
                        FilterMenu::Format,
                        if query.format.is_empty() {
                            "Format".to_owned()
                        } else {
                            query.format.to_uppercase()
                        },
                        !query.format.is_empty(),
                    ),
                    (
                        FilterMenu::Tags,
                        if query.tags.is_empty() {
                            "Tags".to_owned()
                        } else {
                            query.tags.join(", ")
                        },
                        !query.tags.is_empty(),
                    ),
                    (
                        FilterMenu::Makes,
                        if query.makes.is_empty() {
                            "Makes".to_owned()
                        } else {
                            query.makes.join(", ")
                        },
                        !query.makes.is_empty(),
                    ),
                    (
                        FilterMenu::Creators,
                        if query.creators.is_empty() {
                            "Creators".to_owned()
                        } else {
                            query.creators.join(", ")
                        },
                        !query.creators.is_empty(),
                    ),
                ];
                for (menu, label, has_value) in entries {
                    if self.open == Some(menu) {
                        self.menu_x = x;
                    }
                    let width = text_width(ui, &label, 14.0 * scale).round() + 56.0 * scale;
                    let chip_rect = Rect::from_min_size(
                        egui::pos2(x, rect.top()),
                        egui::vec2(width, 38.0 * scale),
                    );
                    let response = chip(
                        ui,
                        chip_rect,
                        if has_value { "" } else { &label },
                        has_value,
                        !has_value,
                        &label,
                        scale,
                    );
                    if has_value {
                        ui.painter_at(chip_rect).text(
                            chip_rect.left_center() + egui::vec2(17.0 * scale, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &label,
                            egui::FontId::proportional(14.0 * scale),
                            Color32::WHITE,
                        );
                        let trailing = egui::Rect::from_center_size(
                            chip_rect.right_center() - egui::vec2(25.0 * scale, 0.0),
                            egui::Vec2::splat(16.0 * scale),
                        );
                        let point = |x: f32, y: f32| {
                            trailing.min + egui::vec2(x * 16.0 / 24.0, y * 16.0 / 24.0) * scale
                        };
                        let stroke = egui::Stroke::new(16.0 * scale * 2.0 / 24.0, Color32::WHITE);
                        ui.painter_at(trailing)
                            .line_segment([point(18.0, 6.0), point(6.0, 18.0)], stroke);
                        ui.painter_at(trailing)
                            .line_segment([point(6.0, 6.0), point(18.0, 18.0)], stroke);
                    }
                    if response.clicked() {
                        let clear = has_value
                            && response.interact_pointer_pos().is_some_and(|position| {
                                position.x >= chip_rect.right() - 39.0 * scale
                            });
                        if clear {
                            match menu {
                                FilterMenu::Sort => store.query_mut().sort = None,
                                FilterMenu::Format => store.query_mut().format.clear(),
                                FilterMenu::Tags => store.query_mut().tags.clear(),
                                FilterMenu::Makes => store.query_mut().makes.clear(),
                                FilterMenu::Creators => store.query_mut().creators.clear(),
                                FilterMenu::Profile => {}
                            }
                            store.begin_query();
                            self.open = None;
                        } else {
                            self.open = (self.open != Some(menu)).then_some(menu);
                            self.menu_x = x;
                        }
                    }
                    if self.open == Some(menu) {
                        self.menu_trigger = Some(Trigger::from(&response));
                    }
                    x += width + 10.0 * scale;
                }
                let width = text_width(ui, "Calibrated", 14.0 * scale).round() + 34.0 * scale;
                if chip(
                    ui,
                    Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(width, 38.0 * scale)),
                    "Calibrated",
                    store.query().calibrated,
                    false,
                    "Calibrated",
                    scale,
                )
                .clicked()
                {
                    store.query_mut().calibrated = !store.query().calibrated;
                    store.begin_query();
                }
                x += width + 10.0 * scale;
                ui.painter().line_segment(
                    [
                        egui::pos2(x, rect.top() + 7.0 * scale),
                        egui::pos2(x, rect.top() + 31.0 * scale),
                    ],
                    egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                );
                x += 11.0 * scale;
            }
            let verified_rect = Rect::from_min_size(
                egui::pos2(x, rect.top()),
                egui::vec2(38.0 * scale, 38.0 * scale),
            );
            let verified = chip(
                ui,
                verified_rect,
                "",
                store.query().verified,
                false,
                "Verified",
                scale,
            );
            if verified.clicked() && !profile_locked {
                store.query_mut().verified = !store.query().verified;
                store.begin_query();
            }
            draw_check_icon(ui, verified_rect, scale);
            if profile_locked {
                ui.painter_at(verified_rect).rect_filled(
                    verified_rect,
                    19.0 * scale,
                    crate::theme::palette::DISABLED_VEIL,
                );
            }
            x += 48.0 * scale;
            if self.open == Some(FilterMenu::Profile) {
                self.menu_x = x;
            }
            let profile = store.query().profile;
            let profile_label = match profile {
                Profile::Downloaded => "Recent",
                Profile::Favorited => "Favorites",
                Profile::Created => "Created",
                Profile::None => "",
            };
            let profile_width = if profile == Profile::None {
                78.0 * scale
            } else {
                text_width(ui, profile_label, 14.0 * scale).round() + 84.0 * scale
            };
            let profile_rect = Rect::from_min_size(
                egui::pos2(x, rect.top()),
                egui::vec2(profile_width, 38.0 * scale),
            );
            let response = chip(
                ui,
                profile_rect,
                "",
                profile != Profile::None,
                false,
                "Profile",
                scale,
            );
            if response.clicked() {
                let clear = profile != Profile::None
                    && response
                        .interact_pointer_pos()
                        .is_some_and(|position| position.x >= profile_rect.right() - 28.0 * scale);
                if clear {
                    store.set_profile(Profile::None);
                    store.begin_query();
                    self.open = None;
                } else {
                    self.open =
                        (self.open != Some(FilterMenu::Profile)).then_some(FilterMenu::Profile);
                    self.menu_x = x;
                }
            }
            if self.open == Some(FilterMenu::Profile) {
                self.menu_trigger = Some(Trigger::from(&response));
            }
            draw_profile_icon(
                ui,
                profile_rect,
                self.profile_avatar.as_ref(),
                profile_label,
                scale,
            );
            x += profile_width + 10.0 * scale;
        }
        let gears = [
            ("amp+cab", "Amp + Cab"),
            ("amp", "Amp Head"),
            ("cab", "Cabinet"),
            ("pedal", "Pedal"),
            ("outboard", "Outboard"),
            ("space", "Spaces"),
            ("experimental", "Experimental"),
        ];
        for (gear, label) in gears {
            let width = text_width(ui, label, 14.0 * scale).round() + 62.0 * scale;
            let active = store.query().gear == gear;
            if gear_chip(
                ui,
                Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(width, 38.0 * scale)),
                gear,
                label,
                active,
                scale,
            )
            .clicked()
            {
                store.set_gear(if active { "" } else { gear });
                // Nothing answers a query with no service, so its loading state would not end.
                if session.has_service() {
                    store.begin_query();
                }
            }
            x += width + 10.0 * scale;
        }
        self.show_menu(ui, rect, store, scale);
    }

    fn show_menu(&mut self, ui: &mut egui::Ui, row: Rect, store: &mut CatalogStore, scale: f32) {
        let Some(menu) = self.open else {
            self.scroll_menu = None;
            self.menu_scroll = 0.0;
            self.taxonomy_asked = None;
            return;
        };
        self.request_taxonomy(menu, store);
        if self.scroll_menu != Some(menu) {
            self.scroll_menu = Some(menu);
            self.menu_scroll = 0.0;
        }
        let options = self.menu_options(store, menu);
        let searchable = matches!(
            menu,
            FilterMenu::Tags | FilterMenu::Makes | FilterMenu::Creators
        );
        let width = options
            .iter()
            .map(|option| {
                let mut chrome = 24.0 * scale;
                if option.checks {
                    chrome += 24.0 * scale;
                }
                if option.bookmark {
                    chrome += 24.0 * scale;
                }
                if option.avatar.is_some() {
                    chrome += 30.0 * scale;
                }
                (chrome + text_width(ui, &option.label, 14.0 * scale) + 19.0 * scale).ceil()
            })
            .fold(200.0 * scale, f32::max)
            .min(320.0 * scale);
        let search_height = if searchable { 38.0 * scale } else { 0.0 };
        let visible_rows = options.len().clamp(1, 6);
        let height = 18.0 * scale + search_height + visible_rows as f32 * 36.0 * scale;
        let shown = Popover::new(
            ui.scope_id().with(("filter-menu", menu)),
            Placement::At(egui::pos2(self.menu_x, row.bottom() + 8.0 * scale)),
            egui::vec2(width, height),
            Surface::hairline(14.0 * scale),
        )
        .trigger(self.menu_trigger)
        .constrain_to(ui.max_rect())
        .show(ui, |ui, panel| {
            let mut pick = None;
            let mut y = panel.top() + 9.0 * scale;
            if searchable {
                let search_rect = egui::Rect::from_min_size(
                    egui::pos2(panel.left() + 9.0 * scale, y),
                    egui::vec2(panel.width() - 18.0 * scale, 32.0 * scale),
                );
                ui.painter_at(search_rect).rect_filled(
                    search_rect,
                    8.0 * scale,
                    crate::theme::palette::SURFACE_RAISED,
                );
                ui.painter_at(search_rect).rect_stroke(
                    search_rect,
                    8.0 * scale,
                    egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                    egui::StrokeKind::Inside,
                );
                let editor_rect = egui::Rect::from_min_max(
                    search_rect.min + egui::vec2(32.0, 6.0) * scale,
                    search_rect.max - egui::vec2(10.0, 6.0) * scale,
                );
                let mut search_ui = ui.new_child(egui::UiBuilder::new().max_rect(editor_rect));
                search_ui
                    .style_mut()
                    .text_styles
                    .values_mut()
                    .for_each(|font| {
                        font.size *= scale;
                    });
                let placeholder = match menu {
                    FilterMenu::Tags => "Search tags",
                    FilterMenu::Makes => "Search makes and models",
                    _ => "Search creators",
                };
                let response = search_ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(
                            egui::RichText::new(placeholder)
                                .color(crate::theme::palette::TEXT_MUTED),
                        )
                        .font(egui::FontId::proportional(14.0 * scale))
                        .align(egui::Align2::LEFT_CENTER)
                        .frame(egui::Frame::NONE),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, placeholder)
                });
                let search_icon = search_rect.left_center() + egui::vec2(18.0 * scale, 0.0);
                let point = |x: f32, y: f32| {
                    search_icon
                        + egui::vec2((x - 12.0) * 14.0 / 24.0, (y - 12.0) * 14.0 / 24.0) * scale
                };
                let icon_stroke =
                    egui::Stroke::new(14.0 * scale * 2.0 / 24.0, crate::theme::palette::TEXT_MUTED);
                ui.painter_at(search_rect).circle_stroke(
                    point(11.0, 11.0),
                    8.0 * scale * 14.0 / 24.0,
                    icon_stroke,
                );
                ui.painter_at(search_rect)
                    .line_segment([point(16.65, 16.65), point(21.0, 21.0)], icon_stroke);
                y += 38.0 * scale;
            }
            let list_rect = egui::Rect::from_min_size(
                egui::pos2(panel.left() + 9.0 * scale, y),
                egui::vec2(panel.width() - 18.0 * scale, 216.0 * scale),
            );
            let max_scroll = (options.len() as f32 * 36.0 * scale - list_rect.height()).max(0.0);
            if ui.rect_contains_pointer(list_rect) {
                let delta = ui.input(|input| input.smooth_scroll_delta.y);
                self.menu_scroll = (self.menu_scroll - delta * scale).clamp(0.0, max_scroll);
            }
            let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
            let ui = &mut list_ui;
            for option in options {
                let row = egui::Rect::from_min_size(
                    egui::pos2(panel.left() + 9.0 * scale, y - self.menu_scroll),
                    egui::vec2(panel.width() - 18.0 * scale, 36.0 * scale),
                );
                if row.bottom() <= list_rect.top() || row.top() >= list_rect.bottom() {
                    y += 36.0 * scale;
                    continue;
                }
                let response = ui.interact(
                    row,
                    ui.scope_id().with(("filter-option", menu, &option.id)),
                    Sense::click(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, option.label.clone())
                });
                if response.hovered() {
                    ui.painter_at(row).rect_filled(
                        row,
                        8.0 * scale,
                        crate::theme::palette::HOVER_FILL,
                    );
                }
                let mut x = row.left() + 12.0 * scale;
                if option.checks {
                    if option.selected {
                        let check = egui::Rect::from_center_size(
                            egui::pos2(x + 8.0 * scale, row.center().y),
                            egui::Vec2::splat(16.0 * scale),
                        );
                        let point = |px: f32, py: f32| {
                            check.min + egui::vec2(px * 16.0 / 24.0, py * 16.0 / 24.0) * scale
                        };
                        ui.painter_at(check).add(egui::epaint::PathShape::line(
                            vec![point(20.0, 6.0), point(9.0, 17.0), point(4.0, 12.0)],
                            egui::Stroke::new(16.0 * scale * 2.0 / 24.0, Color32::WHITE),
                        ));
                    }
                    x += 24.0 * scale;
                }
                if option.bookmark {
                    let bookmark = egui::Rect::from_center_size(
                        egui::pos2(x + 8.0 * scale, row.center().y),
                        egui::Vec2::splat(16.0 * scale),
                    );
                    let point = |px: f32, py: f32| {
                        bookmark.min + egui::vec2(px * 16.0 / 24.0, py * 16.0 / 24.0) * scale
                    };
                    ui.painter_at(bookmark).add(egui::epaint::PathShape::line(
                        vec![
                            point(17.0, 3.0),
                            point(18.0, 3.6),
                            point(19.0, 5.0),
                            point(19.0, 20.0),
                            point(17.504, 20.868),
                            point(12.992, 18.29),
                            point(11.008, 18.29),
                            point(6.496, 20.868),
                            point(5.0, 20.0),
                            point(5.0, 5.0),
                            point(5.6, 3.6),
                            point(7.0, 3.0),
                            point(17.0, 3.0),
                        ],
                        egui::Stroke::new(16.0 * scale * 2.0 / 24.0, muted()),
                    ));
                    x += 24.0 * scale;
                }
                if let Some(texture) = &option.avatar {
                    let avatar = egui::Rect::from_center_size(
                        egui::pos2(x + 11.0 * scale, row.center().y),
                        egui::Vec2::splat(22.0 * scale),
                    );
                    egui::Image::from_texture(texture)
                        .corner_radius(11.0 * scale)
                        .paint_at(ui, avatar);
                    x += 30.0 * scale;
                }
                ui.painter().text(
                    egui::pos2(x, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    &option.label,
                    egui::FontId::proportional(14.0 * scale),
                    if option.selected {
                        Color32::WHITE
                    } else {
                        muted()
                    },
                );
                if response.clicked() {
                    pick = Some(option.id.clone());
                }
                y += 36.0 * scale;
            }
            pick
        });
        if let Some(id) = shown.pick {
            self.apply_menu_pick(menu, &id, store);
        }
        if shown.dismissed.is_some() {
            self.open = None;
        }
    }

    /// Asks for the rows of an open menu one time for each search text.
    fn request_taxonomy(&mut self, menu: FilterMenu, store: &mut CatalogStore) {
        let Some(kind) = taxonomy_kind(menu) else {
            self.taxonomy_asked = None;
            return;
        };
        let asked = self
            .taxonomy_asked
            .as_ref()
            .is_some_and(|(held, text)| *held == menu && *text == self.search);
        if !asked {
            store.request_taxonomy(kind, &self.search);
            self.taxonomy_asked = Some((menu, self.search.clone()));
        }
    }

    fn apply_menu_pick(&mut self, menu: FilterMenu, id: &str, store: &mut CatalogStore) {
        match menu {
            FilterMenu::Sort => {
                store.query_mut().sort = match id {
                    "trending" => Some(ronie_core::ToneSort::Trending),
                    "downloads-all-time" => Some(ronie_core::ToneSort::Popular),
                    "newest" => Some(ronie_core::ToneSort::Newest),
                    "oldest" => Some(ronie_core::ToneSort::Oldest),
                    _ => None,
                };
            }
            FilterMenu::Format => store.set_format(id),
            FilterMenu::Profile => {
                let profile = match id {
                    "downloaded" => Profile::Downloaded,
                    "favorited" => Profile::Favorited,
                    "created" => Profile::Created,
                    _ => Profile::None,
                };
                store.set_profile(if store.query().profile == profile {
                    Profile::None
                } else {
                    profile
                });
            }
            FilterMenu::Tags | FilterMenu::Makes => {
                let values = if menu == FilterMenu::Tags {
                    &mut store.query_mut().tags
                } else {
                    &mut store.query_mut().makes
                };
                toggle_pick(values, id);
            }
            FilterMenu::Creators => toggle_pick(&mut store.query_mut().creators, id),
        }
        store.begin_query();
        self.open = None;
    }

    /// The fetched rows, and each selected value that the rows do not hold.
    fn fetched_options(
        &self,
        store: &CatalogStore,
        menu: FilterMenu,
        rows: &[ronie_core::TaxonomyEntry],
        search: &str,
        options: &mut Vec<MenuOption>,
    ) {
        let query = store.query();
        let selected = match menu {
            FilterMenu::Tags => &query.tags,
            FilterMenu::Makes => &query.makes,
            _ => &query.creators,
        };
        let avatar = |name: &str| {
            (menu == FilterMenu::Creators)
                .then(|| self.creator_avatars.get(name).cloned())
                .flatten()
        };
        for row in rows {
            // A creator row can name its account in a field of its own.
            let name = row.raw["username"].as_str().unwrap_or(&row.name);
            if !name.is_empty() {
                let chosen = selected.iter().any(|value| value == name);
                push_taxonomy_option(options, name, chosen, search, true, avatar(name));
            }
        }
        for name in selected {
            push_taxonomy_option(options, name, true, "", true, avatar(name));
        }
    }

    fn menu_options(&self, store: &CatalogStore, menu: FilterMenu) -> Vec<MenuOption> {
        match menu {
            FilterMenu::Sort => {
                let effective = store
                    .query()
                    .sort
                    .unwrap_or(if store.query().text.is_empty() {
                        ronie_core::ToneSort::Trending
                    } else {
                        ronie_core::ToneSort::BestMatch
                    });
                [
                    (ronie_core::ToneSort::BestMatch, "best-match", "Best match"),
                    (ronie_core::ToneSort::Trending, "trending", "Trending"),
                    (
                        ronie_core::ToneSort::Popular,
                        "downloads-all-time",
                        "Popular",
                    ),
                    (ronie_core::ToneSort::Newest, "newest", "Newest"),
                    (ronie_core::ToneSort::Oldest, "oldest", "Oldest"),
                ]
                .into_iter()
                .filter(|(sort, _, _)| {
                    *sort != ronie_core::ToneSort::BestMatch || !store.query().text.is_empty()
                })
                .map(|(sort, id, label)| MenuOption {
                    id: id.to_owned(),
                    label: label.to_owned(),
                    selected: sort == effective,
                    checks: false,
                    bookmark: false,
                    avatar: None,
                })
                .collect()
            }
            FilterMenu::Format => ["nam", "ir"]
                .into_iter()
                .map(|id| MenuOption {
                    id: id.to_owned(),
                    label: if id == "nam" {
                        "Neural Amp Modeler (NAM)".to_owned()
                    } else {
                        "Impulse Response (IR)".to_owned()
                    },
                    selected: store.query().format == id,
                    checks: false,
                    bookmark: false,
                    avatar: None,
                })
                .collect(),
            FilterMenu::Profile => [
                (Profile::Downloaded, "downloaded", "Recently used"),
                (Profile::Favorited, "favorited", "Favorites"),
                (Profile::Created, "created", "Created"),
            ]
            .into_iter()
            .map(|(profile, id, label)| MenuOption {
                id: id.to_owned(),
                label: label.to_owned(),
                selected: store.query().profile == profile,
                checks: false,
                bookmark: profile == Profile::Favorited,
                avatar: None,
            })
            .collect(),
            FilterMenu::Tags | FilterMenu::Makes | FilterMenu::Creators => {
                let mut options = Vec::new();
                let search = self.search.to_lowercase();
                if let Some(rows) = taxonomy_kind(menu).and_then(|kind| store.taxonomy(kind)) {
                    self.fetched_options(store, menu, rows, &search, &mut options);
                    return options;
                }
                for tone in store.result().into_iter().flat_map(|page| &page.data) {
                    match menu {
                        FilterMenu::Tags | FilterMenu::Makes => {
                            let field = if menu == FilterMenu::Tags {
                                "tags"
                            } else {
                                "makes"
                            };
                            for name in tone.raw[field]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|entry| entry["name"].as_str())
                            {
                                let selected = if menu == FilterMenu::Tags {
                                    store.query().tags.iter().any(|value| value == name)
                                } else {
                                    store.query().makes.iter().any(|value| value == name)
                                };
                                push_taxonomy_option(
                                    &mut options,
                                    name,
                                    selected,
                                    &search,
                                    true,
                                    None,
                                );
                            }
                        }
                        FilterMenu::Creators => {
                            let Some(name) = tone.raw["user"]["username"].as_str() else {
                                continue;
                            };
                            let selected = store.query().creators.iter().any(|value| value == name);
                            push_taxonomy_option(
                                &mut options,
                                name,
                                selected,
                                &search,
                                true,
                                self.creator_avatars.get(name).cloned(),
                            );
                        }
                        _ => {}
                    }
                }
                options
            }
        }
    }
}

const fn taxonomy_kind(menu: FilterMenu) -> Option<Taxonomy> {
    match menu {
        FilterMenu::Tags => Some(Taxonomy::Tags),
        FilterMenu::Makes => Some(Taxonomy::Makes),
        FilterMenu::Creators => Some(Taxonomy::Creators),
        _ => None,
    }
}

struct MenuOption {
    id: String,
    label: String,
    selected: bool,
    checks: bool,
    bookmark: bool,
    avatar: Option<egui::TextureHandle>,
}

fn push_taxonomy_option(
    options: &mut Vec<MenuOption>,
    name: &str,
    selected: bool,
    search: &str,
    checks: bool,
    avatar: Option<egui::TextureHandle>,
) {
    if options
        .iter()
        .any(|option| option.id.eq_ignore_ascii_case(name))
        || (!search.is_empty() && !name.to_lowercase().contains(search))
    {
        return;
    }
    options.push(MenuOption {
        id: name.to_owned(),
        label: name.to_owned(),
        selected,
        checks,
        bookmark: false,
        avatar,
    });
}

fn toggle_pick(values: &mut Vec<String>, id: &str) {
    if let Some(index) = values.iter().position(|value| value == id) {
        values.remove(index);
    } else {
        values.push(id.to_owned());
    }
}

fn draw_filter_icon(ui: &egui::Ui, rect: Rect, expanded: bool, filtered: bool, scale: f32) {
    let painter = ui.painter_at(rect);
    let stroke = egui::Stroke::new(1.4 * scale, crate::theme::palette::TEXT_MUTED);
    if expanded {
        let center = rect.center();
        let point = |x: f32, y: f32| {
            center + egui::vec2((x - 12.0) * 20.0 / 24.0, (y - 12.0) * 20.0 / 24.0) * scale
        };
        painter.add(egui::epaint::PathShape::line(
            vec![point(15.0, 18.0), point(9.0, 12.0), point(15.0, 6.0)],
            egui::Stroke::new(20.0 * scale * 2.0 / 24.0, crate::theme::palette::TEXT_MUTED),
        ));
        return;
    }
    for (index, width) in [12.0, 9.0, 5.0].into_iter().enumerate() {
        let y = rect.center().y + (index as f32 - 1.0) * 5.0 * scale;
        painter.line_segment(
            [
                egui::pos2(rect.center().x - width * scale / 2.0, y),
                egui::pos2(rect.center().x + width * scale / 2.0, y),
            ],
            stroke,
        );
    }
    if filtered {
        painter.circle_filled(
            rect.right_center() - egui::vec2(12.0 * scale, 0.0),
            3.0 * scale,
            Color32::WHITE,
        );
    }
}

fn draw_check_icon(ui: &egui::Ui, rect: Rect, scale: f32) {
    let painter = ui.painter_at(rect);
    let badge = Rect::from_center_size(rect.center(), egui::vec2(15.75, 18.0) * scale);
    let point = |x: f32, y: f32| {
        badge.min + egui::vec2(x * badge.width() / 17.5, y * badge.height() / 20.0)
    };
    for (points, color) in [
        (
            [
                (17.5, 5.88),
                (5.47, 20.0),
                (0.0, 12.96),
                (2.83, 11.81),
                (5.58, 15.34),
                (11.6, 8.27),
            ],
            crate::theme::palette::EMBER,
        ),
        (
            [
                (17.5, 0.0),
                (5.47, 14.12),
                (0.0, 7.08),
                (2.83, 5.93),
                (5.58, 9.46),
                (11.6, 2.38),
            ],
            crate::theme::palette::FIRE,
        ),
        (
            [
                (17.5, 2.94),
                (5.47, 17.06),
                (0.0, 10.02),
                (2.83, 8.87),
                (5.58, 12.4),
                (11.6, 5.32),
            ],
            crate::theme::palette::BLAZE,
        ),
    ] {
        painter.add(egui::Shape::convex_polygon(
            points.into_iter().map(|(x, y)| point(x, y)).collect(),
            color,
            egui::Stroke::NONE,
        ));
    }
}

fn draw_profile_icon(
    ui: &egui::Ui,
    rect: Rect,
    avatar: Option<&egui::TextureHandle>,
    label: &str,
    scale: f32,
) {
    let painter = ui.painter_at(rect);
    let center = egui::pos2(rect.left() + 27.0 * scale, rect.center().y);
    painter.circle_filled(center, 10.0 * scale, crate::theme::palette::EMBER);
    if let Some(texture) = avatar {
        egui::Image::from_texture(texture)
            .corner_radius(10.0 * scale)
            .paint_at(
                ui,
                Rect::from_center_size(center, egui::Vec2::splat(20.0 * scale)),
            );
    }
    if label.is_empty() {
        let chevron = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 53.0 * scale, rect.center().y),
            egui::Vec2::splat(16.0 * scale),
        );
        let point =
            |x: f32, y: f32| chevron.min + egui::vec2(x * 16.0 / 24.0, y * 16.0 / 24.0) * scale;
        painter.add(egui::epaint::PathShape::line(
            vec![point(6.0, 9.0), point(12.0, 15.0), point(18.0, 9.0)],
            egui::Stroke::new(1.0 * scale, crate::theme::palette::TEXT_MUTED),
        ));
    } else {
        painter.text(
            egui::pos2(rect.left() + 45.0 * scale, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(14.0 * scale),
            Color32::WHITE,
        );
        let center = rect.right_center() - egui::vec2(17.0 * scale, 0.0);
        let stroke = egui::Stroke::new(1.3 * scale, Color32::WHITE);
        painter.line_segment(
            [
                center + egui::vec2(-3.0, -3.0) * scale,
                center + egui::vec2(3.0, 3.0) * scale,
            ],
            stroke,
        );
        painter.line_segment(
            [
                center + egui::vec2(-3.0, 3.0) * scale,
                center + egui::vec2(3.0, -3.0) * scale,
            ],
            stroke,
        );
    }
}

fn gear_chip(
    ui: &mut egui::Ui,
    rect: Rect,
    gear: &str,
    label: &str,
    active: bool,
    scale: f32,
) -> egui::Response {
    let response = chip(ui, rect, "", active, false, label, scale);
    let painter = ui.painter_at(rect);
    let text_size = 14.0 * scale;
    let left = rect.left();
    super::gear::draw(
        &painter,
        Rect::from_center_size(
            egui::pos2(left + 27.0 * scale, rect.center().y),
            egui::Vec2::splat(20.0 * scale),
        ),
        gear,
        if active {
            Color32::WHITE
        } else {
            crate::theme::palette::TEXT_MUTED
        },
    );
    painter.text(
        egui::pos2(left + 45.0 * scale, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(text_size),
        if active {
            Color32::WHITE
        } else {
            crate::theme::palette::TEXT_MUTED
        },
    );
    response
}

fn chip(
    ui: &mut egui::Ui,
    rect: Rect,
    text: &str,
    active: bool,
    chevron: bool,
    accessible_name: &str,
    scale: f32,
) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with((text, rect.left() as i32)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, accessible_name.to_owned())
    });
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect,
        19.0 * scale,
        egui::Stroke::new(
            1.0,
            if active {
                Color32::WHITE
            } else {
                crate::theme::palette::HAIRLINE
            },
        ),
        egui::StrokeKind::Inside,
    );
    let color = if active {
        Color32::WHITE
    } else {
        crate::theme::palette::TEXT_MUTED
    };
    if chevron {
        painter.with_clip_rect(rect.shrink(6.0 * scale)).text(
            rect.left_center() + egui::vec2(17.0 * scale, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(14.0 * scale),
            color,
        );
        let trailing = egui::Rect::from_center_size(
            rect.right_center() - egui::vec2(25.0 * scale, 0.0),
            egui::Vec2::splat(16.0 * scale),
        );
        let point =
            |x: f32, y: f32| trailing.min + egui::vec2(x * 16.0 / 24.0, y * 16.0 / 24.0) * scale;
        painter.add(egui::epaint::PathShape::line(
            vec![point(6.0, 9.0), point(12.0, 15.0), point(18.0, 9.0)],
            egui::Stroke::new(1.0 * scale, color),
        ));
    } else {
        painter.with_clip_rect(rect.shrink(6.0 * scale)).text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(14.0 * scale),
            color,
        );
    }
    response
}

fn muted() -> Color32 {
    crate::theme::palette::TEXT_SECONDARY
}

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.ctx().fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                Color32::WHITE,
            )
            .size()
            .x
    })
}

#[cfg(test)]
mod tests {
    use super::SessionStatus;
    #[test]
    fn filter_menu_adopts_the_parent_layer_counter_scale() {
        let context = eframe::egui::Context::default();
        let parent_layer = eframe::egui::LayerId::new(
            eframe::egui::Order::Background,
            eframe::egui::Id::unique("zoomed-browser-body"),
        );
        let counter_scale = eframe::egui::emath::TSTransform {
            translation: eframe::egui::vec2(12.0, 28.0),
            scaling: 0.625,
        };
        context.set_transform_layer(parent_layer, counter_scale);
        let input = eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(1638.0, 900.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            eframe::egui::Area::new(parent_layer.id)
                .order(parent_layer.order)
                .fixed_pos(eframe::egui::Pos2::ZERO)
                .default_size(eframe::egui::vec2(1638.0, 900.0))
                .show(ui.ctx(), |ui| {
                    let mut filters = super::FilterBar::default();
                    filters.set_open_menu(Some(super::FilterMenu::Sort));
                    let mut store = crate::services::catalog::CatalogStore::new();
                    filters.show_at(
                        ui,
                        eframe::egui::Rect::from_min_size(
                            eframe::egui::pos2(48.0, 118.0),
                            eframe::egui::vec2(1536.0, 38.0),
                        ),
                        &mut store,
                        SessionStatus::SignedIn,
                        1.0,
                    );
                    let menu_layer = eframe::egui::LayerId::new(
                        eframe::egui::Order::Foreground,
                        ui.scope_id().with(("filter-menu", super::FilterMenu::Sort)),
                    );
                    assert_eq!(
                        ui.ctx().layer_transform_to_global(menu_layer),
                        Some(counter_scale),
                        "the menu must share the body's graphics and input transform"
                    );
                });
        });
        output.textures_delta.clear();
    }

    #[test]
    fn transformed_filter_menu_applies_clicks_and_constrains_near_edge() {
        let context = eframe::egui::Context::default();
        let parent_layer = eframe::egui::LayerId::new(
            eframe::egui::Order::Background,
            eframe::egui::Id::unique("zoomed-filter-click"),
        );
        let counter_scale = eframe::egui::emath::TSTransform {
            translation: eframe::egui::vec2(12.0, 28.0),
            scaling: 0.625,
        };
        context.set_transform_layer(parent_layer, counter_scale);
        let mut filters = super::FilterBar::default();
        filters.set_open_menu(Some(super::FilterMenu::Sort));
        filters.menu_x = 1500.0;
        let mut store = crate::services::catalog::CatalogStore::new();
        let row = eframe::egui::Rect::from_min_size(
            eframe::egui::pos2(48.0, 118.0),
            eframe::egui::vec2(1536.0, 38.0),
        );
        let input = |events| eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(1638.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        for events in [Vec::new(), Vec::new()] {
            let mut output = context.run_ui(input(events), |ui| {
                eframe::egui::Area::new(parent_layer.id)
                    .order(parent_layer.order)
                    .fixed_pos(eframe::egui::Pos2::ZERO)
                    .default_size(eframe::egui::vec2(1638.0, 900.0))
                    .show(ui.ctx(), |ui| {
                        ui.set_min_size(eframe::egui::vec2(1638.0, 900.0));
                        filters.show_at(ui, row, &mut store, SessionStatus::SignedIn, 1.0);
                    });
            });
            output.textures_delta.clear();
        }
        let popular_local = eframe::egui::pos2(1438.0 + 100.0, 164.0 + 9.0 + 36.0 + 18.0);
        let popular_global = counter_scale * popular_local;
        for pressed in [true, false] {
            let mut events = vec![eframe::egui::Event::PointerButton {
                pos: popular_global,
                button: eframe::egui::PointerButton::Primary,
                pressed,
                modifiers: eframe::egui::Modifiers::NONE,
            }];
            if pressed {
                events.insert(0, eframe::egui::Event::PointerMoved(popular_global));
            }
            let mut output = context.run_ui(input(events), |ui| {
                eframe::egui::Area::new(parent_layer.id)
                    .order(parent_layer.order)
                    .fixed_pos(eframe::egui::Pos2::ZERO)
                    .default_size(eframe::egui::vec2(1638.0, 900.0))
                    .show(ui.ctx(), |ui| {
                        ui.set_min_size(eframe::egui::vec2(1638.0, 900.0));
                        filters.show_at(ui, row, &mut store, SessionStatus::SignedIn, 1.0);
                    });
            });
            output.textures_delta.clear();
        }
        assert_eq!(store.query().sort, Some(ronie_core::ToneSort::Popular));
        assert_eq!(filters.open_menu(), None);
    }
}
