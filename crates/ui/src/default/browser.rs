//! Searchable tone browser with filters, cards, and bounded artwork loading.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, Color32, TextureHandle};
use ronie_core::{Taxonomy, Tone, ToneId};

use crate::services::{
    catalog::CatalogStatus,
    images::{ART_SIDE, AVATAR_SIDE, ImageCache, RemoteImages},
    session::SessionStatus,
};

use super::{
    filters::FilterBar,
    tone_card::{self, ToneCardAction},
};

pub use crate::services::catalog::{BrowserController, CatalogStore};

/// Action selected in the browser.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserAction {
    /// Load one tone into the pending target.
    Tone(Tone),
    /// Change the favorite state of one tone.
    Favorite {
        /// Tone to change.
        tone: ToneId,
        /// New state.
        favorite: bool,
    },
    /// Start authentication.
    SignIn,
    /// Close the browser.
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum BrowserImage {
    Artwork(ToneId),
    Avatar(ToneId),
    Creator(String),
}

type GridLayout = (egui::Rect, f32, f32, f32);

/// Browser view and its optional shared image resources.
pub struct BrowserView {
    controller: BrowserController,
    filters: FilterBar,
    images: Option<RemoteImages<BrowserImage>>,
    artwork: BTreeMap<ToneId, TextureHandle>,
    avatars: BTreeMap<ToneId, TextureHandle>,
    pending_tone: Option<ToneId>,
    busy_backdrop: Option<TextureHandle>,
    busy_overlay_held: bool,
    sign_in_page: bool,
    zoom: f32,
    max_scroll: f32,
}

impl Default for BrowserView {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserView {
    /// Creates a browser with fallback glyphs only.
    #[must_use]
    pub fn new() -> Self {
        Self {
            controller: BrowserController::new(),
            filters: FilterBar::default(),
            images: None,
            artwork: BTreeMap::new(),
            avatars: BTreeMap::new(),
            pending_tone: None,
            busy_backdrop: None,
            busy_overlay_held: false,
            sign_in_page: false,
            zoom: 1.0,
            max_scroll: f32::MAX,
        }
    }

    /// Creates a browser with a shared bounded image cache.
    #[must_use]
    pub fn with_images(images: ImageCache) -> Self {
        let mut browser = Self::new();
        browser.images = Some(RemoteImages::new(images));
        browser
    }

    /// Shared catalog state.
    #[must_use]
    pub const fn store(&self) -> &CatalogStore {
        self.controller.store()
    }

    /// Mutable catalog state.
    pub fn store_mut(&mut self) -> &mut CatalogStore {
        self.controller.store_mut()
    }

    /// Current scroll position.
    #[must_use]
    pub const fn scroll(&self) -> f32 {
        self.controller.scroll()
    }

    /// Saves the grid scroll position.
    pub fn set_scroll(&mut self, scroll: f32) {
        self.controller.set_scroll(scroll);
    }

    /// Independent browser zoom.
    #[must_use]
    pub const fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Sets the independent browser zoom, which is not the capture scale.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.max(0.05);
    }

    /// Tone whose load is currently resolving.
    #[must_use]
    pub const fn pending_tone(&self) -> Option<ToneId> {
        self.pending_tone
    }

    /// Updates the picked card shown as busy; all other cards become inert.
    pub fn set_pending_tone(&mut self, pending: Option<ToneId>) {
        self.pending_tone = pending;
    }

    /// Supplies the latest clean, blurred frame for the picked card snapshot.
    pub fn set_busy_backdrop(&mut self, backdrop: Option<TextureHandle>) {
        self.busy_backdrop = backdrop;
    }

    /// Keeps the busy state off the picked card, because the window now captures that card.
    ///
    /// The window paints a screenshot in the frame after the one that asks for it.
    pub fn hold_busy_overlay(&mut self, held: bool) {
        self.busy_overlay_held = held;
    }

    /// Whether a picked card waits for its capture.
    #[must_use]
    pub const fn busy_card_held(&self) -> bool {
        self.busy_overlay_held && self.pending_tone.is_some()
    }

    /// Whether the signed-out sign-in prompt replaces the results.
    #[must_use]
    pub const fn sign_in_page(&self) -> bool {
        self.sign_in_page
    }

    /// Shows or leaves the signed-out card's sign-in prompt.
    pub fn set_sign_in_page(&mut self, shown: bool) {
        self.sign_in_page = shown;
        if shown {
            self.set_scroll(0.0);
        }
    }

    /// Closes the browser and cancels pending work.
    pub fn close(&mut self) {
        if let Some(images) = &mut self.images {
            images.stop();
        }
        self.pending_tone = None;
        self.sign_in_page = false;
        self.filters.forget_taxonomy_request();
        self.controller.close();
    }

    /// Removes the filter state of the previous session. The store drops its own results.
    pub(crate) fn drop_session_filters(&mut self) {
        self.filters.drop_session_state();
    }

    /// Reopens preserved query/result/scroll state.
    pub fn reopen(&mut self) {
        self.controller.reopen();
    }

    /// Whether the secondary filter row is expanded.
    #[must_use]
    pub const fn filters_expanded(&self) -> bool {
        self.filters.expanded()
    }

    /// Sets whether the secondary filter row is expanded.
    pub fn set_filters_expanded(&mut self, expanded: bool) {
        self.filters.set_expanded(expanded);
    }

    /// Currently open filter menu.
    #[must_use]
    pub const fn filter_menu(&self) -> Option<super::filters::FilterMenu> {
        self.filters.open_menu()
    }

    /// Opens or closes one filter menu.
    pub fn set_filter_menu(&mut self, menu: Option<super::filters::FilterMenu>) {
        self.filters.set_open_menu(menu);
    }

    /// Supplies the session avatar used by the profile chip.
    pub fn set_profile_avatar(&mut self, avatar: TextureHandle) {
        self.filters.set_profile_avatar(avatar);
    }

    /// Supplies decoded artwork for one tone card.
    pub fn set_artwork(&mut self, id: ToneId, texture: TextureHandle) {
        self.artwork.insert(id, texture);
    }

    /// Supplies a decoded creator avatar for one tone card.
    pub fn set_avatar(&mut self, id: ToneId, texture: TextureHandle) {
        self.avatars.insert(id, texture.clone());
        if let Some(name) = self
            .store()
            .result()
            .and_then(|page| page.data.iter().find(|tone| tone.id == id))
            .and_then(|tone| tone.raw["user"]["username"].as_str())
            .map(str::to_owned)
        {
            self.filters.set_creator_avatar(&name, texture);
        }
    }

    /// Whether one tone card has decoded artwork.
    #[must_use]
    pub fn has_artwork(&self, id: ToneId) -> bool {
        self.artwork.contains_key(&id)
    }

    /// Whether one tone card has a decoded creator avatar.
    #[must_use]
    pub fn has_avatar(&self, id: ToneId) -> bool {
        self.avatars.contains_key(&id)
    }

    /// Whether the creator menu has a decoded avatar for one name.
    #[must_use]
    pub fn has_creator_avatar(&self, name: &str) -> bool {
        self.filters.has_creator_avatar(name)
    }

    /// Renders the browser and returns at most one action.
    pub fn show(&mut self, ui: &mut egui::Ui, session: SessionStatus) -> Option<BrowserAction> {
        if !session.can_sign_in() {
            self.sign_in_page = false;
        }
        self.poll_images(ui);
        let zoom = self.zoom;
        let root = ui.available_rect_before_wrap();
        let back =
            egui::Rect::from_min_size(root.min + egui::vec2(32.0, 24.0), egui::vec2(150.0, 22.0));
        let back_response = ui.interact(
            back,
            ui.scope_id().with("close-tone-browser"),
            egui::Sense::click(),
        );
        back_response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, "Close tone browser")
        });
        let painter = ui.painter_at(back);
        let center = egui::pos2(back.left() + 8.0, back.center().y);
        let stroke = egui::Stroke::new(4.0 / 3.0, Color32::WHITE);
        painter.line_segment(
            [
                center + egui::vec2(0.0, 14.0 / 3.0),
                center + egui::vec2(-14.0 / 3.0, 0.0),
            ],
            stroke,
        );
        painter.line_segment(
            [
                center + egui::vec2(-14.0 / 3.0, 0.0),
                center + egui::vec2(0.0, -14.0 / 3.0),
            ],
            stroke,
        );
        painter.line_segment(
            [
                center + egui::vec2(-14.0 / 3.0, 0.0),
                center + egui::vec2(14.0 / 3.0, 0.0),
            ],
            stroke,
        );
        if !self.sign_in_page {
            painter.text(
                egui::pos2(back.left() + 32.0, back.center().y),
                egui::Align2::LEFT_CENTER,
                "SELECT TONE",
                egui::FontId::monospace(16.0),
                Color32::WHITE,
            );
        }
        let mut action = None;
        if back_response.clicked() {
            if self.sign_in_page {
                self.set_sign_in_page(false);
            } else {
                self.close();
                action = Some(BrowserAction::Close);
            }
        }

        let body = egui::Rect::from_min_max(root.min + egui::vec2(0.0, 46.0), root.max);
        let body_layer =
            egui::LayerId::new(ui.layer_id().order, ui.scope_id().with("browser-body"));
        let body_action = if zoom == 1.0 {
            ui.ctx()
                .set_transform_layer(body_layer, egui::emath::TSTransform::IDENTITY);
            self.show_body(ui, body, 1.0, session)
        } else {
            let layout = crate::shell::BrowserLayout::new(
                body.width().round() as i32,
                body.height().round() as i32,
                f64::from(zoom),
            );
            let virtual_body = egui::Rect::from_min_size(
                body.min,
                egui::vec2(layout.content_width as f32, layout.content_height as f32),
            );
            let scaling = layout.scale as f32;
            let transform = egui::emath::TSTransform {
                scaling,
                translation: body.min.to_vec2() * (1.0 - scaling),
            };
            ui.ctx().set_sublayer(ui.layer_id(), body_layer);
            let mut body_ui = ui.new_child(
                egui::UiBuilder::new()
                    .layer_id(body_layer)
                    .max_rect(virtual_body),
            );
            body_ui.set_clip_rect(transform.inverse() * body.intersect(ui.clip_rect()));
            body_ui.ctx().set_transform_layer(body_layer, transform);
            let action = self.show_body(&mut body_ui, virtual_body, 1.0, session);
            body_ui.expand_to_include_rect(virtual_body);
            action
        };
        body_action.or(action)
    }

    fn show_body(
        &mut self,
        ui: &mut egui::Ui,
        body: egui::Rect,
        scale: f32,
        session: SessionStatus,
    ) -> Option<BrowserAction> {
        let mut action = None;
        if self.sign_in_page {
            let (clicked, _) = show_prompt(
                ui,
                body,
                PromptSpec {
                    mark: true,
                    copy: "Sign in to see your tones and discover a zillion new ones.",
                    max_width: 420.0,
                    button: "Sign in or create free account",
                    centered: true,
                },
                scale,
            );
            if clicked {
                action = Some(BrowserAction::SignIn);
            }
            return action;
        }
        let column_width = (960.0 * self.zoom).min(body.width());
        let column_x = body.left() + ((body.width() - column_width) / 2.0).floor();
        let mut y = body.top();
        if session.signed_in() {
            y += 16.0 * scale;
            let search_rect = egui::Rect::from_min_size(
                egui::pos2(column_x, y),
                egui::vec2(column_width, 40.0 * scale),
            );
            ui.painter_at(search_rect).rect_filled(
                search_rect,
                20.0 * scale,
                crate::theme::palette::SURFACE_RAISED,
            );
            ui.painter_at(search_rect).rect_stroke(
                search_rect,
                20.0 * scale,
                egui::Stroke::new(1.0, crate::theme::palette::HAIRLINE),
                egui::StrokeKind::Inside,
            );
            let search_icon = search_rect.left_center() + egui::vec2(16.0 * scale, 0.0);
            // The drawing is 24 units high, so 12 is the middle line of the bar.
            let icon_point = |x: f32, y: f32| {
                search_icon + egui::vec2(x * 0.75 * scale, (y - 12.0) * 0.75 * scale)
            };
            let icon_stroke = egui::Stroke::new(1.5 * scale, crate::theme::palette::TEXT_MUTED);
            ui.painter_at(search_rect).circle_stroke(
                icon_point(11.0, 11.0),
                8.0 * 0.75 * scale,
                icon_stroke,
            );
            ui.painter_at(search_rect).line_segment(
                [icon_point(16.65, 16.65), icon_point(21.0, 21.0)],
                icon_stroke,
            );
            let mut search = self.store().query().text.clone();
            let editor_rect = egui::Rect::from_min_max(
                search_rect.min + egui::vec2(44.0 * scale, 0.0),
                search_rect.max - egui::vec2(12.0 * scale, 0.0),
            );
            let mut search_ui = ui.new_child(egui::UiBuilder::new().max_rect(editor_rect));
            search_ui
                .style_mut()
                .text_styles
                .values_mut()
                .for_each(|font| font.size *= scale);
            search_ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .hint_text(
                            egui::RichText::new("Search…").color(crate::theme::palette::TEXT_MUTED),
                        )
                        .font(egui::FontId::proportional(14.0 * scale))
                        .align(egui::Align2::LEFT_CENTER)
                        .frame(egui::Frame::NONE),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Search tones")
                });
                if !search.is_empty() {
                    let center = search_rect.right_center() - egui::vec2(16.0 * scale, 0.0);
                    let stroke = egui::Stroke::new(1.3 * scale, crate::theme::palette::TEXT_MUTED);
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(-4.0, -4.0) * scale,
                            center + egui::vec2(4.0, 4.0) * scale,
                        ],
                        stroke,
                    );
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(-4.0, 4.0) * scale,
                            center + egui::vec2(4.0, -4.0) * scale,
                        ],
                        stroke,
                    );
                }
                let clear = !search.is_empty()
                    && response
                        .interact_pointer_pos()
                        .is_some_and(|position| position.x >= search_rect.right() - 40.0 * scale);
                if clear {
                    self.store_mut().query_mut().text.clear();
                    self.store_mut().begin_query();
                } else if response.changed() {
                    self.store_mut().query_mut().text = search;
                    self.store_mut().begin_query();
                }
            });
            y += 40.0 * scale;
        }
        y += 16.0 * scale;
        let filters = egui::Rect::from_min_size(
            egui::pos2(column_x, y),
            egui::vec2(column_width, 38.0 * scale),
        );
        let Self {
            controller,
            filters: filter_bar,
            ..
        } = self;
        filter_bar.show_at(ui, filters, controller.store_mut(), session, scale);
        y += 38.0 * scale;
        let result = if self.store().status() == CatalogStatus::Error {
            None
        } else {
            self.store().result().cloned()
        };
        let paginate = result.as_ref().is_some_and(|page| page.total_pages > 1);
        let bottom = body.bottom() - if paginate { 44.0 * scale } else { 0.0 };
        let scroll_rect =
            egui::Rect::from_min_max(egui::pos2(body.left(), y), egui::pos2(body.right(), bottom));
        self.show_grid(
            ui,
            (scroll_rect, column_x, column_width, scale),
            result.clone(),
            &mut action,
            session,
        );
        if paginate {
            self.show_paginator(
                ui,
                body,
                column_x,
                column_width,
                result.as_ref().expect("page"),
                scale,
            );
        }
        action
    }

    fn show_grid(
        &mut self,
        ui: &mut egui::Ui,
        layout: GridLayout,
        result: Option<ronie_core::Page<ronie_core::Tone>>,
        action: &mut Option<BrowserAction>,
        session: SessionStatus,
    ) {
        let (rect, column_x, column_width, scale) = layout;
        let status = self.store().status();
        if status == CatalogStatus::Error {
            let (clicked, _) = show_prompt(
                ui,
                rect.translate(egui::vec2(0.0, 24.0 * scale)),
                PromptSpec {
                    mark: false,
                    copy: "Failed to load tones from TONE3000.",
                    max_width: 340.0,
                    button: "Try again",
                    centered: false,
                },
                scale,
            );
            if clicked {
                self.store_mut().begin_query();
            }
            self.set_scroll(0.0);
            return;
        }
        let has_cards = result.as_ref().is_some_and(|page| !page.data.is_empty());
        let mut grid_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let wheel_multiplier = ui
            .ctx()
            .layer_transform_from_global(ui.layer_id())
            .map_or(1.0, |transform| transform.scaling);
        let output = egui::ScrollArea::vertical()
            .id_salt("tone-browser-grid")
            .wheel_scroll_multiplier(egui::Vec2::splat(wheel_multiplier))
            .vertical_scroll_offset(self.scroll().min(self.max_scroll))
            .show(&mut grid_ui, |ui| {
                let content_top = ui.max_rect().top();
                let mut y = content_top + 24.0 * scale;
                if status == CatalogStatus::Loading {
                    for x in [-12.0 * scale, 0.0, 12.0 * scale] {
                        ui.painter().circle_filled(
                            egui::pos2(column_x + column_width / 2.0 + x, y + 64.0 * scale),
                            4.0 * scale,
                            Color32::WHITE,
                        );
                    }
                }
                if let Some(error) = self.store().error().map(str::to_owned) {
                    ui.painter().text(
                        egui::pos2(column_x, y),
                        egui::Align2::LEFT_TOP,
                        error,
                        egui::FontId::proportional(13.0 * scale),
                        crate::theme::palette::FLAME,
                    );
                    let button = egui::Rect::from_min_size(
                        egui::pos2(column_x, y + 28.0 * scale),
                        egui::vec2(100.0 * scale, 34.0 * scale),
                    );
                    if ui
                        .add_sized(button.size(), egui::Button::new("Try again"))
                        .clicked()
                    {
                        self.store_mut().begin_query();
                    }
                    y += 56.0 * scale;
                }
                let columns = if column_width >= 1052.0 * scale { 3 } else { 2 };
                let card_width =
                    ((column_width - (columns - 1) as f32 * 16.0 * scale) / columns as f32).floor();
                if let Some(page) = result.filter(|page| !page.data.is_empty()) {
                    let mut row_y = y;
                    for tones in page.data.chunks(columns) {
                        let row_height = tones
                            .iter()
                            .map(|tone| tone_card::content_height_for(ui, tone, card_width, scale))
                            .fold(112.0 * scale, f32::max);
                        for (column, tone) in tones.iter().enumerate() {
                            self.request_images(tone);
                            let top = row_y.round();
                            let card = egui::Rect::from_min_size(
                                egui::pos2(
                                    column_x + column as f32 * (card_width + 16.0 * scale),
                                    top,
                                ),
                                egui::vec2(card_width, (row_y + row_height).round() - top),
                            );
                            if let Some(card_action) = tone_card::show_at(
                                ui,
                                card,
                                tone,
                                tone_card::ToneCardPaint {
                                    artwork: self.artwork.get(&tone.id),
                                    avatar_texture: self.avatars.get(&tone.id),
                                    scale,
                                    content_height: row_height,
                                    pending_tone: self.pending_tone,
                                    busy_backdrop: self.busy_backdrop.as_ref(),
                                    busy_overlay: !self.busy_overlay_held,
                                    favorite: session.signed_in().then(|| {
                                        tone.raw["is_favorite"].as_bool().unwrap_or(false)
                                    }),
                                },
                            ) {
                                match card_action {
                                    ToneCardAction::Open => {
                                        if session.can_sign_in() {
                                            self.set_sign_in_page(true);
                                        } else {
                                            *action = Some(BrowserAction::Tone(tone.clone()));
                                        }
                                    }
                                    ToneCardAction::Favorite(id) => {
                                        *action = Some(BrowserAction::Favorite {
                                            tone: id,
                                            favorite: !tone.raw["is_favorite"]
                                                .as_bool()
                                                .unwrap_or(false),
                                        });
                                    }
                                }
                            }
                        }
                        row_y += row_height + 16.0 * scale;
                    }
                    y = row_y - 16.0 * scale;
                } else if status == CatalogStatus::Idle {
                    ui.painter().text(
                        egui::pos2(column_x, y + 32.0 * scale),
                        egui::Align2::LEFT_TOP,
                        if !session.has_service() {
                            "Right-click a tile on the main screen and choose Load File."
                        } else {
                            "Search the tone catalog or preview trending tones."
                        },
                        egui::FontId::proportional(14.0 * scale),
                        crate::theme::palette::TEXT_SECONDARY,
                    );
                } else if status != CatalogStatus::Loading {
                    let copy = if self.store().result_is_trending() {
                        "No trending tones of this type right now. Try another."
                    } else if !self.store().query().text.is_empty() {
                        "No tones match. Try a different search or fewer filters."
                    } else {
                        match self.store().query().profile {
                            crate::services::catalog::Profile::Downloaded => {
                                "Tones you download on TONE3000 will show up here."
                            }
                            crate::services::catalog::Profile::Favorited => {
                                "Tones you favorite on TONE3000 will show up here."
                            }
                            crate::services::catalog::Profile::Created => {
                                "Tones you upload to TONE3000 will show up here."
                            }
                            crate::services::catalog::Profile::None => {
                                "No tones match. Try fewer filters."
                            }
                        }
                    };
                    let line_height = crate::fonts::normal_line_height(13.0) as f32 * scale;
                    ui.painter().text(
                        egui::pos2(column_x + column_width / 2.0, y + 64.0 * scale),
                        egui::Align2::CENTER_TOP,
                        copy,
                        egui::FontId::proportional(13.0 * scale),
                        crate::theme::palette::TEXT_SECONDARY,
                    );
                    y += 128.0 * scale + line_height;
                }
                if session.can_sign_in() && (has_cards || status != CatalogStatus::Loading) {
                    let (clicked, prompt_height) = show_prompt(
                        ui,
                        egui::Rect::from_min_size(
                            egui::pos2(column_x, y),
                            egui::vec2(column_width, 208.0 * scale),
                        ),
                        PromptSpec {
                            mark: true,
                            copy: "Discover a zillion more tones.",
                            max_width: 420.0,
                            button: "Sign in or create free account",
                            centered: false,
                        },
                        scale,
                    );
                    y += prompt_height;
                    if clicked {
                        *action = Some(BrowserAction::SignIn);
                    }
                }
                y += 24.0 * scale;
                self.max_scroll = (y - content_top - rect.height()).max(0.0);
                ui.set_min_height((y - content_top).max(rect.height()));
            });
        let offset = output.state.clamped_offset().y;
        paint_scroll_fades(
            ui,
            output.inner_rect,
            scale,
            offset > 0.0,
            offset + output.inner_rect.height() < output.content_size.y,
        );
        self.set_scroll(offset);
    }

    fn show_paginator(
        &mut self,
        ui: &mut egui::Ui,
        body: egui::Rect,
        column_x: f32,
        column_width: f32,
        page: &ronie_core::Page<ronie_core::Tone>,
        scale: f32,
    ) {
        #[derive(Clone, Copy)]
        enum Item {
            Previous,
            Next,
            Number(u64),
            Ellipsis,
        }
        let text_width = |text: String| {
            ui.ctx().fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(
                        text,
                        egui::FontId::proportional(13.0 * scale),
                        Color32::WHITE,
                    )
                    .size()
                    .x
            })
        };
        let pages = if page.total_pages > 7 {
            if page.page <= 3 {
                (1..=4).chain([0, page.total_pages]).collect::<Vec<_>>()
            } else if page.page.saturating_add(2) >= page.total_pages {
                [1, 0]
                    .into_iter()
                    .chain(page.total_pages - 3..=page.total_pages)
                    .collect()
            } else {
                vec![
                    1,
                    0,
                    page.page - 1,
                    page.page,
                    page.page.saturating_add(1),
                    0,
                    page.total_pages,
                ]
            }
        } else {
            (1..=page.total_pages).collect()
        };
        let mut items = Vec::new();
        let mut total_width = 0.0;
        let mut add = |item, width| {
            items.push((item, width));
            total_width += width + 8.0 * scale;
        };
        if page.page > 1 {
            add(Item::Previous, 28.0 * scale);
        }
        for number in pages {
            if number == 0 {
                add(Item::Ellipsis, text_width("…".to_owned()) + 16.0 * scale);
            } else {
                add(
                    Item::Number(number),
                    text_width(number.to_string()) + 26.0 * scale,
                );
            }
        }
        if page.page < page.total_pages {
            add(Item::Next, 28.0 * scale);
        }
        let y = body.bottom() - 32.0 * scale;
        let mut x = column_x + column_width - total_width + 8.0 * scale;
        for (item, width) in items {
            let height = if matches!(item, Item::Previous | Item::Next) {
                28.0 * scale
            } else {
                25.0 * scale
            };
            let rect = egui::Rect::from_center_size(
                egui::pos2(x + width / 2.0, y + 14.0 * scale),
                egui::vec2(width, height),
            );
            match item {
                Item::Previous | Item::Next => {
                    let direction = if matches!(item, Item::Previous) {
                        1.0
                    } else {
                        -1.0
                    };
                    let stroke = egui::Stroke::new(1.5 * scale, crate::theme::palette::TEXT_MUTED);
                    let center = rect.center();
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(7.0 * direction, 0.0) * scale,
                            center + egui::vec2(-7.0 * direction, 0.0) * scale,
                        ],
                        stroke,
                    );
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(0.0, -7.0) * scale,
                            center + egui::vec2(-7.0 * direction, 0.0),
                        ],
                        stroke,
                    );
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(-7.0 * direction, 0.0),
                            center + egui::vec2(0.0, 7.0) * scale,
                        ],
                        stroke,
                    );
                }
                Item::Ellipsis => {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "…",
                        egui::FontId::proportional(13.0 * scale),
                        crate::theme::palette::TEXT_MUTED,
                    );
                }
                Item::Number(number) => {
                    let current = number == page.page;
                    let color = if current {
                        Color32::WHITE
                    } else {
                        crate::theme::palette::TEXT_MUTED
                    };
                    ui.painter().rect_stroke(
                        rect,
                        6.0 * scale,
                        egui::Stroke::new(
                            1.0,
                            if current {
                                Color32::WHITE
                            } else {
                                crate::theme::palette::HAIRLINE
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        number.to_string(),
                        egui::FontId::proportional(13.0 * scale),
                        color,
                    );
                }
            }
            let target = match item {
                Item::Previous => page.page - 1,
                Item::Next => page.page.saturating_add(1),
                Item::Number(number) => number,
                Item::Ellipsis => page.page,
            };
            let (kind, accessible_name) = match item {
                Item::Previous => ("previous", Some("Previous page".to_owned())),
                Item::Next => ("next", Some("Next page".to_owned())),
                Item::Number(number) => ("number", Some(format!("Page {number}"))),
                Item::Ellipsis => ("ellipsis", None),
            };
            if let Some(accessible_name) = accessible_name {
                let enabled = target != page.page;
                let response = ui.interact(
                    rect,
                    ui.scope_id().with(("browser-page", kind, target)),
                    if enabled {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    },
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, enabled, accessible_name.clone())
                });
                if enabled && response.clicked() {
                    self.store_mut().turn_page(target);
                }
            }
            x += width + 8.0 * scale;
        }
        let pages_rect = egui::Rect::from_min_size(
            egui::pos2(column_x + column_width - total_width + 8.0 * scale, y),
            egui::vec2(total_width - 8.0 * scale, 28.0 * scale),
        );
        let response = ui.allocate_rect(pages_rect, egui::Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Pages"));
    }

    fn request_images(&mut self, tone: &Tone) {
        let Some(images) = &mut self.images else {
            return;
        };
        if let Some(url) = tone.raw["images"]
            .as_array()
            .and_then(|images| images.first())
            .and_then(serde_json::Value::as_str)
        {
            images.request(BrowserImage::Artwork(tone.id), url, ART_SIDE);
        }
        if let Some(url) = tone.raw["user"]["avatar_url"].as_str() {
            images.request(BrowserImage::Avatar(tone.id), url, AVATAR_SIDE);
        }
    }

    /// Asks for the avatars that the open creator menu can show. A menu has few rows.
    fn request_creator_avatars(&mut self) {
        if self.filters.open_menu() != Some(super::filters::FilterMenu::Creators) {
            return;
        }
        let Some(images) = &mut self.images else {
            return;
        };
        let store = self.controller.store();
        let rows = store.taxonomy(Taxonomy::Creators).unwrap_or_default();
        let listed = rows
            .iter()
            .map(|row| row.raw["username"].as_str().unwrap_or(&row.name));
        let selected = store.query().creators.iter().map(String::as_str);
        for name in listed.chain(selected) {
            if let Some(url) = store.creator_avatar(name) {
                images.request(BrowserImage::Creator(name.to_owned()), url, AVATAR_SIDE);
            }
        }
    }

    /// Keeps textures only for the current page, so that a long browse stays bounded. A return
    /// to a page takes its images from the cache.
    fn drop_images_of_other_pages(&mut self) {
        let Some(images) = &mut self.images else {
            return;
        };
        let tones = self
            .controller
            .store()
            .result()
            .iter()
            .flat_map(|page| &page.data)
            .map(|tone| tone.id)
            .collect::<BTreeSet<_>>();
        images.retain(|key| match key {
            BrowserImage::Artwork(id) | BrowserImage::Avatar(id) => tones.contains(id),
            BrowserImage::Creator(_) => true,
        });
        self.artwork.retain(|id, _| tones.contains(id));
        self.avatars.retain(|id, _| tones.contains(id));
    }

    fn poll_images(&mut self, ui: &egui::Ui) {
        self.drop_images_of_other_pages();
        self.request_creator_avatars();
        let Some(images) = &mut self.images else {
            return;
        };
        for (key, texture) in images.poll(ui.ctx()) {
            match key {
                BrowserImage::Artwork(id) => self.set_artwork(id, texture),
                BrowserImage::Avatar(id) => self.set_avatar(id, texture),
                BrowserImage::Creator(name) => {
                    self.filters.set_creator_avatar(&name, texture);
                }
            }
        }
    }
}

struct PromptSpec<'a> {
    mark: bool,
    copy: &'a str,
    max_width: f32,
    button: &'a str,
    centered: bool,
}

fn show_prompt(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    spec: PromptSpec<'_>,
    scale: f32,
) -> (bool, f32) {
    let available = (spec.max_width * scale).min((rect.width() - 48.0 * scale).max(1.0));
    let mut job = egui::text::LayoutJob::simple(
        spec.copy.to_owned(),
        egui::FontId::proportional(14.0 * scale),
        Color32::WHITE,
        available,
    );
    job.halign = egui::Align::Center;
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    let mark_height = if spec.mark { 44.0 * scale } else { 0.0 };
    let content_height = 96.0 * scale
        + mark_height
        + if spec.mark { 16.0 * scale } else { 0.0 }
        + galley.rect.height()
        + 16.0 * scale
        + 36.0 * scale;
    let top = if spec.centered {
        rect.top() + (rect.height() - content_height).max(0.0) / 2.0
    } else {
        rect.top()
    };
    let mut y = top + 48.0 * scale;
    let center_x = rect.center().x;
    if spec.mark {
        super::brand::draw_launcher_icon(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(center_x, y + 22.0 * scale),
                egui::Vec2::splat(44.0 * scale),
            ),
        );
        y += 60.0 * scale;
    }
    let copy_height = galley.rect.height();
    ui.painter()
        .galley(egui::pos2(center_x, y), galley, Color32::WHITE);
    y += copy_height + 16.0 * scale;
    let button_width = if spec.button == "Try again" {
        98.0 * scale
    } else {
        ui.ctx().fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(
                    spec.button.to_owned(),
                    egui::FontId::proportional(14.0 * scale),
                    Color32::WHITE,
                )
                .size()
                .x
        }) + 40.0 * scale
    };
    let button = egui::Rect::from_center_size(
        egui::pos2(center_x, y + 18.0 * scale),
        egui::vec2(button_width, 36.0 * scale),
    );
    let response = ui.interact(
        button,
        ui.scope_id().with(("browser-prompt", spec.button)),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, spec.button.to_owned())
    });
    ui.painter()
        .rect_filled(button, 18.0 * scale, Color32::WHITE);
    ui.painter().text(
        button.center(),
        egui::Align2::CENTER_CENTER,
        spec.button,
        egui::FontId::proportional(14.0 * scale),
        Color32::BLACK,
    );
    (response.clicked(), content_height)
}

fn paint_scroll_fades(ui: &egui::Ui, rect: egui::Rect, scale: f32, above: bool, below: bool) {
    let bands = (32.0 * scale).round().max(1.0) as usize;
    let solid = ((bands as f32) * 0.2).ceil() as usize;
    let painter = ui.painter_at(rect);
    let row_height = 32.0 * scale / bands as f32;
    for index in 0..bands {
        let transition = index.saturating_sub(solid) as f32 / (bands - solid).max(1) as f32;
        if above {
            let alpha = if index < solid {
                1.0
            } else {
                (1.0 - transition).powf(1.5)
            };
            let y = rect.top() + index as f32 * row_height;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), y),
                    egui::vec2(rect.width(), row_height.ceil()),
                ),
                0.0,
                Color32::from_black_alpha((alpha * 255.0).round() as u8),
            );
        }
        if below {
            let alpha = if index < solid {
                1.0
            } else {
                (1.0 - (index - solid) as f32 / (bands - 1 - solid).max(1) as f32).powf(1.5)
            };
            let y = rect.bottom() - (index + 1) as f32 * row_height;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), y),
                    egui::vec2(rect.width(), row_height.ceil()),
                ),
                0.0,
                Color32::from_black_alpha((alpha * 255.0).round() as u8),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{BrowserView, SessionStatus};
    use eframe::egui::{self, Color32, emath::TSTransform};
    use ronie_core::{Page, Tone};

    fn page(rows: usize) -> Page<Tone> {
        Page {
            data: (0..rows)
                .map(|index| {
                    Tone::from_value(serde_json::json!({
                        "id": index,
                        "title": format!("Tone {index}"),
                        "format": "nam",
                        "models": []
                    }))
                    .expect("tone")
                })
                .collect(),
            page: 1,
            page_size: 20,
            total: 60,
            total_pages: 3,
        }
    }

    fn ready_browser(rows: usize) -> (BrowserView, Page<Tone>) {
        let mut browser = BrowserView::new();
        let page = page(rows);
        let generation = browser.store_mut().begin_query();
        assert!(
            browser
                .store_mut()
                .complete_query(generation, page.clone(), false)
        );
        (browser, page)
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 900.0),
            )),
            events,
            ..Default::default()
        }
    }

    /// All text that one frame of a new browser paints.
    fn painted_text(session: SessionStatus) -> String {
        let context = egui::Context::default();
        crate::fonts::Fonts::install(&context);
        let mut browser = BrowserView::new();
        let mut output = context.run_ui(input(Vec::new()), |ui| {
            let _ = browser.show(ui, session);
        });
        output.textures_delta.clear();
        let mut text = String::new();
        for clipped in output.shapes {
            if let egui::Shape::Text(shape) = clipped.shape {
                text.push_str(shape.galley.text());
                text.push('\n');
            }
        }
        text
    }

    #[test]
    fn the_search_icon_sits_on_the_middle_line_of_the_bar() {
        let context = egui::Context::default();
        crate::fonts::Fonts::install(&context);
        let mut browser = BrowserView::new();
        let mut output = context.run_ui(input(Vec::new()), |ui| {
            let _ = browser.show(ui, SessionStatus::SignedIn);
        });
        output.textures_delta.clear();
        let bar = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == crate::theme::palette::SURFACE_RAISED => {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("search bar");
        let lens = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Circle(circle)
                    if circle.stroke.color == crate::theme::palette::TEXT_MUTED =>
                {
                    Some(circle.center)
                }
                _ => None,
            })
            .expect("search icon");
        assert!(
            (lens.y - bar.center().y).abs() <= 1.0,
            "lens at {}, bar middle at {}",
            lens.y,
            bar.center().y
        );
    }

    #[test]
    fn the_idle_page_points_to_a_local_file_when_no_service_exists() {
        let text = painted_text(SessionStatus::NoService);
        assert!(text.contains("Load File"), "{text}");
        assert!(!text.contains("Search the tone catalog"), "{text}");
        assert!(!text.contains("zillion"), "{text}");

        let text = painted_text(SessionStatus::SignedOut);
        assert!(text.contains("Search the tone catalog"), "{text}");
        assert!(text.contains("zillion"), "{text}");
        assert!(!text.contains("Load File"), "{text}");
    }

    #[test]
    fn a_page_with_no_service_leaves_the_sign_in_prompt() {
        let context = egui::Context::default();
        crate::fonts::Fonts::install(&context);
        let mut browser = BrowserView::new();
        browser.set_sign_in_page(true);
        let mut output = context.run_ui(input(Vec::new()), |ui| {
            let _ = browser.show(ui, SessionStatus::NoService);
        });
        output.textures_delta.clear();
        assert!(!browser.sign_in_page());
    }

    #[test]
    fn paginator_click_uses_transformed_widget_coordinates() {
        let context = egui::Context::default();
        let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::unique("zoomed-paginator"));
        let transform = TSTransform {
            translation: egui::vec2(12.0, 28.0),
            scaling: 0.625,
        };
        context.set_transform_layer(layer, transform);
        let (mut browser, page) = ready_browser(0);
        let click = Cell::new(egui::Pos2::ZERO);
        let body = egui::Rect::from_min_size(egui::pos2(40.0, 50.0), egui::vec2(1200.0, 700.0));
        let column_x = 120.0;
        let column_width = 960.0;
        for _ in 0..2 {
            let mut output = context.run_ui(input(Vec::new()), |ui| {
                egui::Area::new(layer.id)
                    .order(layer.order)
                    .fixed_pos(egui::Pos2::ZERO)
                    .default_size(egui::vec2(1600.0, 900.0))
                    .show(ui.ctx(), |ui| {
                        ui.set_min_size(egui::vec2(1600.0, 900.0));
                        let text_width = |text: String| {
                            ui.ctx().fonts_mut(|fonts| {
                                fonts
                                    .layout_no_wrap(
                                        text,
                                        egui::FontId::proportional(13.0),
                                        Color32::WHITE,
                                    )
                                    .size()
                                    .x
                            })
                        };
                        let number_width = |number: u64| text_width(number.to_string()) + 26.0;
                        let total_width = 36.0
                            + (1..=3)
                                .map(|number| number_width(number) + 8.0)
                                .sum::<f32>()
                            + 36.0;
                        let page_two_x = column_x + column_width - total_width
                            + 8.0
                            + 36.0
                            + number_width(1)
                            + 8.0;
                        let page_two = egui::Rect::from_center_size(
                            egui::pos2(page_two_x + number_width(2) / 2.0, body.bottom() - 18.0),
                            egui::vec2(number_width(2), 25.0),
                        );
                        click.set(transform * page_two.center());
                        browser.show_paginator(ui, body, column_x, column_width, &page, 1.0);
                    });
            });
            output.textures_delta.clear();
        }
        for pressed in [true, false] {
            let position = click.get();
            let mut events = vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }];
            if pressed {
                events.insert(0, egui::Event::PointerMoved(position));
            }
            let mut output = context.run_ui(input(events), |ui| {
                egui::Area::new(layer.id)
                    .order(layer.order)
                    .fixed_pos(egui::Pos2::ZERO)
                    .default_size(egui::vec2(1600.0, 900.0))
                    .show(ui.ctx(), |ui| {
                        ui.set_min_size(egui::vec2(1600.0, 900.0));
                        browser.show_paginator(ui, body, column_x, column_width, &page, 1.0);
                    });
            });
            output.textures_delta.clear();
        }
        assert_eq!(browser.store().result().expect("result").page, 2);
    }

    #[test]
    fn a_paginator_survives_the_largest_page_that_a_server_can_send() {
        let context = egui::Context::default();
        let (mut browser, mut page) = ready_browser(1);
        page.page = u64::MAX;
        page.total_pages = u64::MAX;
        let body = egui::Rect::from_min_size(egui::pos2(40.0, 50.0), egui::vec2(1200.0, 700.0));
        let mut output = context.run_ui(input(Vec::new()), |ui| {
            browser.show_paginator(ui, body, 120.0, 960.0, &page, 1.0);
        });
        output.textures_delta.clear();
    }

    fn physical_grid_wheel_offset(zoom: f32) -> f32 {
        let context = egui::Context::default();
        let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::unique("zoomed-grid-wheel"));
        let transform = if zoom == 1.0 {
            TSTransform::IDENTITY
        } else {
            TSTransform {
                translation: egui::vec2(12.0, 28.0),
                scaling: 1.0 / zoom,
            }
        };
        context.set_transform_layer(layer, transform);
        let (mut browser, page) = ready_browser(20);
        browser.set_zoom(zoom);
        let grid = egui::Rect::from_min_size(egui::pos2(100.0, 150.0), egui::vec2(1000.0, 300.0));
        let pointer = transform * grid.center();
        for _ in 0..2 {
            let mut output = context.run_ui(input(Vec::new()), |ui| {
                egui::Area::new(layer.id)
                    .order(layer.order)
                    .fixed_pos(egui::Pos2::ZERO)
                    .default_size(egui::vec2(1600.0, 900.0))
                    .show(ui.ctx(), |ui| {
                        ui.set_min_size(egui::vec2(1600.0, 900.0));
                        let mut action = None;
                        browser.show_grid(
                            ui,
                            (grid, 120.0, 960.0, 1.0),
                            Some(page.clone()),
                            &mut action,
                            SessionStatus::SignedIn,
                        );
                    });
            });
            output.textures_delta.clear();
        }
        let mut output = context.run_ui(
            input(vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -40.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ui| {
                egui::Area::new(layer.id)
                    .order(layer.order)
                    .fixed_pos(egui::Pos2::ZERO)
                    .default_size(egui::vec2(1600.0, 900.0))
                    .show(ui.ctx(), |ui| {
                        ui.set_min_size(egui::vec2(1600.0, 900.0));
                        let mut action = None;
                        browser.show_grid(
                            ui,
                            (grid, 120.0, 960.0, 1.0),
                            Some(page.clone()),
                            &mut action,
                            SessionStatus::SignedIn,
                        );
                    });
            },
        );
        output.textures_delta.clear();
        browser.scroll() * transform.scaling
    }

    #[test]
    fn grid_wheel_has_equivalent_physical_travel_across_zoom() {
        let unzoomed = physical_grid_wheel_offset(1.0);
        let zoomed = physical_grid_wheel_offset(1.6);
        assert!(unzoomed > 0.0, "the baseline wheel event must scroll");
        assert!(
            (unzoomed - zoomed).abs() < 0.001,
            "physical wheel travel differed: 1x={unzoomed}, 1.6x={zoomed}"
        );
    }

    #[test]
    fn grid_leaves_unconsumed_wheel_for_its_parent() {
        let context = egui::Context::default();
        let layer =
            egui::LayerId::new(egui::Order::Middle, egui::Id::unique("grid-wheel-chaining"));
        let transform = TSTransform {
            translation: egui::vec2(12.0, 28.0),
            scaling: 0.625,
        };
        context.set_transform_layer(layer, transform);
        let (mut browser, page) = ready_browser(0);
        let grid = egui::Rect::from_min_size(egui::pos2(100.0, 150.0), egui::vec2(1000.0, 300.0));
        let pointer = transform * grid.center();
        let parent_delta = Cell::new(0.0);
        for events in [
            Vec::new(),
            Vec::new(),
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -40.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        ] {
            let mut output = context.run_ui(input(events), |ui| {
                egui::Area::new(layer.id)
                    .order(layer.order)
                    .fixed_pos(egui::Pos2::ZERO)
                    .default_size(egui::vec2(1600.0, 900.0))
                    .show(ui.ctx(), |ui| {
                        ui.set_min_size(egui::vec2(1600.0, 900.0));
                        let output = egui::ScrollArea::vertical()
                            .id_salt("parent-grid-wheel")
                            .max_height(500.0)
                            .show(ui, |ui| {
                                let mut action = None;
                                browser.show_grid(
                                    ui,
                                    (grid, 120.0, 960.0, 1.0),
                                    Some(page.clone()),
                                    &mut action,
                                    SessionStatus::SignedIn,
                                );
                                ui.set_min_height(2_000.0);
                            });
                        assert_eq!(output.content_size.y, 2_000.0);
                        parent_delta.set(ui.input(|input| input.smooth_scroll_delta.y));
                    });
            });
            output.textures_delta.clear();
        }
        assert_eq!(browser.scroll(), 0.0);
        assert!(parent_delta.get() < 0.0);
    }
}
