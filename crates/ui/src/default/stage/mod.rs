//! The Stage layout: a light header with the chain as gear icons, a stage for the selected
//! block, a light global panel, and a black utility bar.
//!
//! It reuses the state owners of the reference layout (`ChromeController`, `ChainGallery`,
//! `BlockDetail`, `Faceplate`) and only places them differently.

use crate::theme::Look;
mod add_menu;
mod block_menu;
mod header;
mod utility;

use eframe::egui;

use super::header::{ChromeController, Placement, show_faceplate, show_overlay, show_stage};
use crate::services::load_flow::LoadPhase;

/// Header height in design points.
pub(super) const HEADER_HEIGHT: f32 = 64.0;
/// Utility bar height in design points; it takes the slot of the hint strip.
pub(super) const UTILITY_HEIGHT: f32 = 36.0;

/// Draws the whole Stage window below any banner.
pub(super) fn show(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &crate::backend::ChainViewState,
    now_ms: i64,
) {
    let look = chrome.stage_look(ui.ctx());
    Look::install(ui.ctx(), look);
    let header_bounds = egui::Rect::from_min_size(
        ui.available_rect_before_wrap().min,
        egui::vec2(ui.available_width(), HEADER_HEIGHT),
    );
    // Fill only what is left below any banner, so that the banner stays visible.
    ui.painter()
        .rect_filled(ui.available_rect_before_wrap(), 0.0, look.background);

    egui::Panel::top("stage-header")
        .exact_size(HEADER_HEIGHT)
        .frame(egui::Frame::NONE.fill(look.background))
        .show(ui, |ui| header::show_header(ui, chrome, state));
    show_block_menu(ui, chrome, state);
    show_overlay(ui, chrome);
    egui::Panel::bottom("stage-utility")
        .exact_size(UTILITY_HEIGHT)
        .frame(egui::Frame::NONE.fill(look.utility))
        .show(ui, |ui| utility::show_utility(ui, chrome, state));
    chrome.faceplate_mut().set_show_lanes(true);
    show_faceplate(ui, chrome, look.panel);
    if let Some(stereo) = chrome.faceplate_mut().take_lanes_request() {
        let _ = chrome.set_stereo(stereo);
    }
    // What is left between the header and the global panel: the stage, which holds a slot's menu.
    let stage = ui.available_rect_before_wrap();
    if let Some(stops) = look.spotlight {
        paint_spotlight(&ui.painter().with_clip_rect(stage), stage, stops);
    }

    delete_on_key(ui.ctx(), chrome);
    // Settle selection first, so that the stage decides on this frame's state and never falls
    // through to the reference chain gallery.
    chrome.sync_detail();
    chrome.sync_browser();
    let mut focus = Focus::load(ui.ctx());
    focus.observe(chrome, state);
    let stage_free = !chrome.tuner_visible()
        && !chrome.auth_active()
        && chrome.browser_mut().is_none()
        && chrome.detail_mut().is_none()
        && matches!(
            chrome.gallery().selection(),
            super::chain::GallerySelection::None
        );
    if stage_free {
        // The pass-top snapshot can still hold the block that Delete just removed, so choosing
        // the next block from it would leave the stage with no page and fall through to the
        // reference chain gallery for a frame.
        let after = chrome.backend().chain_state(0);
        match focus.next_block(&after) {
            Some(id) => {
                chrome.gallery_mut().activate(&id);
                chrome.sync_detail();
            }
            None => {
                focus.store(ui.ctx());
                show_empty(ui, chrome, state, look);
                show_add_menu(ui, chrome, state, header_bounds, stage);
                return;
            }
        }
    }
    focus.store(ui.ctx());
    if let Some(detail) = chrome.detail_mut() {
        detail.set_back_link(false);
        detail.set_page(super::block::BlockPage::Stage);
        let (before, after) = pedal_run(state, detail.block_id());
        detail.set_stage_neighbours(before, after);
        let slots = stage_slots(state, detail.block_id());
        detail.set_stage_slots(slots);
        let menu = add_menu::AddMenu::load(ui.ctx());
        detail.set_open_slot(menu.open_slot());
        detail.set_replace_open(menu.replaced_block() == Some(detail.block_id()));
    }
    show_stage(ui, chrome, now_ms);
    if let Some(id) = chrome
        .detail_mut()
        .and_then(super::block::BlockDetail::take_select_request)
    {
        chrome.gallery_mut().activate(&id.0);
        chrome.sync_detail();
        ui.ctx().request_repaint();
    }
    if let Some(request) = chrome
        .detail_mut()
        .and_then(super::block::BlockDetail::take_slot_request)
    {
        open_slot_menu(ui.ctx(), state, request);
    }
    let page_replace = chrome.detail_mut().and_then(|detail| {
        let anchor = detail.take_replace_request()?;
        Some((detail.block_id().clone(), anchor))
    });
    if let Some((id, anchor)) = &page_replace {
        chrome.open_replace(id.clone(), *anchor);
    }
    if let Some((id, anchor)) = chrome.take_replace_request() {
        let on_page = page_replace.is_some_and(|(page, _)| page == id);
        open_replace_menu(ui.ctx(), state, id, anchor, on_page);
    }
    follow_slot(ui.ctx(), chrome);
    follow_replace(ui.ctx(), chrome, state);
    show_add_menu(ui, chrome, state, header_bounds, stage);
}

/// Paints the radial light of the stage, like the CSS `ellipse 70% 75% at 50% 45%`: an
/// ellipse whose horizontal radius is 70 % of the stage width and whose vertical radius is 75 %
/// of its height, centred at 45 % of its height. It runs from `stops[0]` at the centre to
/// `stops[1]` at 70 % of the way and `stops[2]` at the edge, which also fills the rest of
/// `stage`.
fn paint_spotlight(painter: &egui::Painter, stage: egui::Rect, stops: [egui::Color32; 3]) {
    const SEGMENTS: u32 = 96;
    painter.rect_filled(stage, 0.0, stops[2]);
    let center = egui::pos2(stage.center().x, stage.top() + stage.height() * 0.45);
    let radii = egui::vec2(stage.width() * 0.7, stage.height() * 0.75);
    let mut mesh = egui::Mesh::default();
    // Vertex 0 is the centre; each ring after it has SEGMENTS vertices.
    mesh.colored_vertex(center, stops[0]);
    for (fraction, color) in [(0.7, stops[1]), (1.0, stops[2])] {
        for segment in 0..SEGMENTS {
            let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let offset = egui::vec2(angle.cos() * radii.x, angle.sin() * radii.y) * fraction;
            mesh.colored_vertex(center + offset, color);
        }
    }
    // A fan from the centre to the first ring, then a band from the first ring to the edge.
    for segment in 0..SEGMENTS {
        let next = (segment + 1) % SEGMENTS;
        mesh.add_triangle(0, 1 + segment, 1 + next);
    }
    let (inner, outer) = (1, 1 + SEGMENTS);
    for segment in 0..SEGMENTS {
        let next = (segment + 1) % SEGMENTS;
        mesh.add_triangle(inner + segment, outer + segment, outer + next);
        mesh.add_triangle(inner + segment, outer + next, inner + next);
    }
    painter.add(mesh);
}

/// Opens the add menu beside the slot that the player clicked, or closes it when it is that
/// slot's menu already.
fn open_slot_menu(
    ctx: &egui::Context,
    state: &crate::backend::ChainViewState,
    request: super::block::SlotRequest,
) {
    let mut menu = add_menu::AddMenu::load(ctx);
    if menu.open_slot() == Some((request.category, request.placement.clone())) {
        menu.open = false;
    } else {
        let lane = match &request.placement {
            Placement::After(id) | Placement::Before(id) => header::lane_of(state, Some(id)),
            Placement::End | Placement::Smart => ronnie_core::ChainSide::Left,
        };
        menu.open_at(request.category, request.placement, request.anchor, lane);
    }
    menu.store(ctx);
    ctx.request_repaint();
}

/// Opens the add menu under `anchor` to replace the tone of block `id`, on the block's kind of
/// gear, or closes it when it is that block's replace menu already.
fn open_replace_menu(
    ctx: &egui::Context,
    state: &crate::backend::ChainViewState,
    id: ronnie_core::BlockId,
    anchor: egui::Rect,
    on_page: bool,
) {
    let mut menu = add_menu::AddMenu::load(ctx);
    if menu.replaced_block() == Some(&id) {
        menu.open = false;
    } else {
        let category = state
            .chain
            .left
            .iter()
            .chain(&state.chain.right)
            .find(|block| block.id == id)
            .and_then(gear_of);
        menu.open_replace(id, category, anchor);
        menu.on_page_button = on_page;
        // The click that asked for the menu may land where it opens, such as on a row of the
        // block menu, and must not close it again.
        menu.fresh = true;
    }
    menu.store(ctx);
    ctx.request_repaint();
}

/// Draws the block menu of the chain strip when it is open and carries out its row.
fn show_block_menu(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &crate::backend::ChainViewState,
) {
    let mut menu = block_menu::BlockMenu::load(ui.ctx());
    if !menu.is_open() {
        return;
    }
    let block = |id: &ronnie_core::BlockId| {
        state
            .chain
            .left
            .iter()
            .chain(&state.chain.right)
            .find(|block| block.id == *id && !block.is_insert)
    };
    menu.retain(|id| block(id).is_some());
    let chosen = block_menu::show(ui, &mut menu);
    menu.store(ui.ctx());
    let Some((id, anchor, action)) = chosen else {
        return;
    };
    match action {
        block_menu::BlockMenuAction::Bypass => {
            if let Some(block) = block(&id) {
                let _ = chrome
                    .gallery_mut()
                    .set_enabled(&id.0, !block.params.enabled);
            }
        }
        block_menu::BlockMenuAction::Replace => chrome.open_replace(id, anchor),
        block_menu::BlockMenuAction::Duplicate => {
            let _ = chrome.gallery_mut().duplicate(&id.0);
        }
        block_menu::BlockMenuAction::Remove => remove_block(chrome, &id),
    }
    ui.ctx().request_repaint();
}

/// Removes block `id` as REMOVE of its page does: through the page when the block is open, so
/// that the stage moves on to a neighbour, else from the chain.
fn remove_block(chrome: &mut ChromeController, id: &ronnie_core::BlockId) {
    match chrome.detail_mut().filter(|detail| detail.block_id() == id) {
        Some(detail) => {
            let _ = detail.remove();
        }
        None => {
            let _ = chrome.gallery_mut().remove(&id.0);
        }
    }
}

/// Delete or Backspace removes the open block, unless a text field takes the keys or a menu,
/// popup, overlay or other surface is open.
fn delete_on_key(ctx: &egui::Context, chrome: &mut ChromeController) {
    let busy = ctx.text_edit_focused()
        || egui::Popup::is_any_open(ctx)
        || add_menu::AddMenu::load(ctx).open
        || block_menu::BlockMenu::load(ctx).is_open()
        || chrome.overlay() != super::overlays::Overlay::None
        || chrome.tuner_visible()
        || chrome.auth_active()
        || chrome.browser_mut().is_some();
    if busy {
        return;
    }
    let pressed = ctx.input_mut(|input| {
        let delete = input.consume_key(egui::Modifiers::NONE, egui::Key::Delete);
        let backspace = input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace);
        delete || backspace
    });
    if !pressed {
        return;
    }
    let selected = match chrome.gallery().selection() {
        super::chain::GallerySelection::BlockDetail(id) => id.clone(),
        _ => return,
    };
    remove_block(chrome, &selected);
    ctx.request_repaint();
}

/// The kind of gear that a block models, when it is known.
fn gear_of(block: &ronnie_core::ChainBlock) -> Option<ronnie_core::gear::GearCategory> {
    super::skins::gear_category(block)
        .filter(|category| *category != ronnie_core::gear::GearCategory::Unknown)
}

/// Keeps a replace menu under the REPLACE button that opened it when the page lays out again,
/// and closes it when its block is gone.
fn follow_replace(
    ctx: &egui::Context,
    chrome: &mut ChromeController,
    state: &crate::backend::ChainViewState,
) {
    let mut menu = add_menu::AddMenu::load(ctx);
    let Some(id) = menu.replaced_block().cloned() else {
        return;
    };
    let exists = state
        .chain
        .left
        .iter()
        .chain(&state.chain.right)
        .any(|block| block.id == id && !block.is_insert);
    if !exists {
        menu.open = false;
        menu.store(ctx);
        ctx.request_repaint();
        return;
    }
    if !menu.on_page_button {
        return;
    }
    let drawn = chrome
        .detail_mut()
        .filter(|detail| *detail.block_id() == id)
        .and_then(|detail| detail.replace_rect());
    if let Some(rect) = drawn
        && menu.anchor != Some(rect)
    {
        menu.anchor = Some(rect);
        menu.store(ctx);
    }
}

/// Keeps a slot's menu beside its slot when the stage lays out again, such as after a resize,
/// and closes it when its slot left the stage, such as when another block opened. Another slot
/// of the same gear places the block elsewhere, so the menu follows only its own placement.
fn follow_slot(ctx: &egui::Context, chrome: &mut ChromeController) {
    let mut menu = add_menu::AddMenu::load(ctx);
    let Some((category, placement)) = menu.open_slot() else {
        return;
    };
    let drawn = chrome
        .detail_mut()
        .and_then(|detail| detail.slot_rect(category, &placement));
    match drawn {
        Some(rect) if menu.anchor == Some(rect) => return,
        Some(rect) => menu.anchor = Some(rect),
        None => {
            menu.open = false;
            if let Some(detail) = chrome.detail_mut() {
                detail.set_open_slot(None);
            }
            ctx.request_repaint();
        }
    }
    menu.store(ctx);
}

/// What the header of a slot's menu says about where the block lands, such as "after OD808".
fn place_of_slot(state: &crate::backend::ChainViewState, placement: &Placement) -> Option<String> {
    let (word, id) = match placement {
        Placement::After(id) => ("after", id),
        Placement::Before(id) => ("before", id),
        Placement::End | Placement::Smart => return None,
    };
    let block = state
        .chain
        .left
        .iter()
        .chain(&state.chain.right)
        .find(|block| &block.id == id)?;
    Some(add_menu::place_text(word, &block.tone.as_ref()?.title))
}

/// Where the menu's tone goes: the slot's place, and for the + menu, in the lane it shows, the
/// place of the chosen category.
fn menu_placement(state: &crate::backend::ChainViewState, menu: &add_menu::AddMenu) -> Placement {
    match (&menu.placement, menu.view) {
        (Placement::End, add_menu::MenuView::Results(category)) if menu.replacing.is_none() => {
            let lane = match menu.lane {
                ronnie_core::ChainSide::Left => &state.chain.left,
                ronnie_core::ChainSide::Right => &state.chain.right,
            };
            super::header::smart_placement(lane, category, None)
        }
        (placement, _) => placement.clone(),
    }
}

/// Draws the add menu when it is open and carries out what the player chose.
fn show_add_menu(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &crate::backend::ChainViewState,
    header_bounds: egui::Rect,
    stage: egui::Rect,
) {
    let mut menu = add_menu::AddMenu::load(ui.ctx());
    if menu.adding && chrome.load_flow_phase() != LoadPhase::Loading {
        // The add from this menu ended: a landed tone closes the menu, and a failure leaves it.
        menu.adding = false;
        if chrome.load_flow_phase() == LoadPhase::Ready {
            menu.open = false;
        }
        menu.store(ui.ctx());
    }
    if !menu.open {
        chrome.close_add_results();
        return;
    }
    if !state.chain.stereo_enabled {
        // Mono has one lane; a lane B choice from before the switch no longer exists.
        menu.lane = ronnie_core::ChainSide::Left;
    }
    let data = add_menu::MenuData {
        stereo: state.chain.stereo_enabled,
        library: chrome.recent_tones(),
        in_chain: state
            .chain
            .left
            .iter()
            .chain(&state.chain.right)
            .filter(|block| !block.is_insert)
            .filter_map(|block| block.tone.as_ref())
            .collect(),
        signed_in: chrome.session().signed_in(),
        catalog_downloads: chrome.can_download_catalog_tones(),
        results: chrome.add_store(),
        loading: chrome.loading_tone(),
        place: place_of_slot(state, &menu_placement(state, &menu)),
        replacing: menu.replacing.as_ref().and_then(|id| {
            state
                .chain
                .left
                .iter()
                .chain(&state.chain.right)
                .find(|block| block.id == *id)
                .and_then(|block| block.tone.as_ref())
        }),
    };
    // A downloading row spins, so the menu must repaint until its tone lands.
    let loading = data.loading.is_some();
    let plus = header::plus_rect(header_bounds);
    let action = add_menu::show(ui, plus, stage, &mut menu, &data);
    if let Some(action) = action {
        if let Some(id) = menu.replacing.clone() {
            replace_from_menu(chrome, &mut menu, &id.0, action);
        } else {
            add_from_menu(chrome, &mut menu, state, action);
        }
    }
    match menu.view {
        add_menu::MenuView::Results(category) if menu.open => {
            chrome.open_add_results(category, &menu.search);
        }
        _ => chrome.close_add_results(),
    }
    menu.store(ui.ctx());
    // The row of a downloading tone spins, so the menu repaints until it lands. Otherwise the
    // app's 30 Hz cadence decides when the next frame runs.
    if menu.adding || loading {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }
}

/// Carries out what the player chose in the menu of an add.
fn add_from_menu(
    chrome: &mut ChromeController,
    menu: &mut add_menu::AddMenu,
    state: &crate::backend::ChainViewState,
    action: add_menu::AddMenuAction,
) {
    let insert = header::insert_of(state, menu.lane);
    // A menu that no slot opened places the tone by its kind of gear, when it lands.
    let placement = match &menu.placement {
        Placement::End => Placement::Smart,
        slot => slot.clone(),
    };
    match action {
        add_menu::AddMenuAction::OpenBrowser(category) => {
            menu.open = false;
            if let Some(id) = insert {
                chrome.open_browser_for(&id, category, &menu.search, placement);
            }
        }
        add_menu::AddMenuAction::LoadFile => {
            menu.open = false;
            let picked = rfd::FileDialog::new()
                .set_title("Open a Ronnie model")
                .add_filter("Audio models", &["nam", "wav"])
                .pick_file();
            if let (Some(path), Some(id)) = (picked, insert) {
                let _ = chrome.drop_file_into(&id, &path, placement);
            }
        }
        add_menu::AddMenuAction::Add(tone) => {
            if let Some(id) = insert {
                chrome.add_tone(menu.lane, &id, tone, placement);
            }
            // A download keeps the menu open, so that its row shows the load, and a failed
            // load keeps it open beside its toast, so that the player can try again.
            menu.adding = chrome.load_flow_phase() == LoadPhase::Loading;
            menu.open = chrome.load_flow_phase() != LoadPhase::Ready;
        }
        add_menu::AddMenuAction::SignIn => {
            menu.open = false;
            let _ = chrome.sign_in();
        }
    }
}

/// Carries out what the player chose in a replace menu, for block `id`.
fn replace_from_menu(
    chrome: &mut ChromeController,
    menu: &mut add_menu::AddMenu,
    id: &str,
    action: add_menu::AddMenuAction,
) {
    match action {
        add_menu::AddMenuAction::OpenBrowser(category) => {
            menu.open = false;
            chrome.open_replace_browser(id, category, &menu.search);
        }
        add_menu::AddMenuAction::LoadFile => {
            menu.open = false;
            let picked = rfd::FileDialog::new()
                .set_title("Open a Ronnie model")
                .add_filter("Audio models", &["nam", "wav"])
                .pick_file();
            if let Some(path) = picked {
                let _ = chrome.replace_with_file(id, &path);
            }
        }
        add_menu::AddMenuAction::Add(tone) => {
            chrome.replace_tone(id, tone);
            // As with an add, a download keeps the menu open on its row, and a failure keeps it
            // open beside its toast.
            menu.adding = chrome.load_flow_phase() == LoadPhase::Loading;
            menu.open = chrome.load_flow_phase() != LoadPhase::Ready;
        }
        add_menu::AddMenuAction::SignIn => {
            menu.open = false;
            let _ = chrome.sign_in();
        }
    }
}

/// The stage of a chain with no tone blocks: a prompt and a button that opens the browser.
fn show_empty(
    ui: &mut egui::Ui,
    chrome: &mut ChromeController,
    state: &crate::backend::ChainViewState,
    look: Look,
) {
    let area = ui.available_rect_before_wrap();
    let painter = ui.painter().clone();
    ui.put(
        egui::Rect::from_center_size(
            area.center() - egui::vec2(0.0, 28.0),
            egui::vec2(400.0, 28.0),
        ),
        egui::Label::new(
            egui::RichText::new("Your chain is empty")
                .font(egui::FontId::new(
                    18.0,
                    egui::FontFamily::Name("arimo-bold".into()),
                ))
                .color(look.text),
        )
        .selectable(false),
    );
    let button = egui::Rect::from_center_size(
        area.center() + egui::vec2(0.0, 16.0),
        egui::vec2(180.0, 36.0),
    );
    let response = ui.interact(
        button,
        ui.scope_id().with("stage-first-block"),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, "Add your first block")
    });
    painter.rect_filled(
        button,
        6.0,
        if response.hovered() {
            look.accent
        } else {
            look.solid
        },
    );
    painter.text(
        button.center(),
        egui::Align2::CENTER_CENTER,
        "Add a block",
        egui::FontId::new(13.0, egui::FontFamily::Name("arimo-bold".into())),
        look.background,
    );
    if response.clicked() {
        let insert = state
            .chain
            .left
            .iter()
            .find(|block| block.is_insert)
            .map(|block| block.id.0.clone());
        if let Some(id) = insert {
            chrome.gallery_mut().activate(&id);
        }
    }
}

/// Which block the stage showed, so that it can choose the right block after a change.
#[derive(Clone, Default)]
struct Focus {
    /// The block on the stage, and its lane and position among the tone blocks of that lane.
    shown: Option<(String, bool, usize)>,
    /// The slot that the tone browser fills, so that the loaded block opens.
    pending: Option<String>,
    /// The preset the stage last saw, so that a preset switch opens its first block.
    preset: Option<String>,
}

impl Focus {
    fn id() -> egui::Id {
        egui::Id::unique("stage-focus")
    }

    fn load(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp::<Self>(Self::id()))
            .unwrap_or_default()
    }

    fn store(&self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(Self::id(), self.clone()));
    }

    /// Records the current selection and notices a preset switch.
    fn observe(&mut self, chrome: &ChromeController, state: &crate::backend::ChainViewState) {
        let preset = chrome.active_preset_name().map(str::to_owned);
        if preset != self.preset {
            if self.preset.is_some() || self.shown.is_some() {
                self.shown = None;
                self.pending = None;
            }
            self.preset = preset;
        }
        match chrome.gallery().selection() {
            super::chain::GallerySelection::BlockDetail(id) => {
                if let Some(place) = place_of(state, &id.0) {
                    self.shown = Some((id.0.clone(), place.0, place.1));
                }
            }
            super::chain::GallerySelection::SelectTone {
                target_block_id, ..
            } => {
                self.pending = Some(target_block_id.0.clone());
            }
            super::chain::GallerySelection::None => {}
        }
    }

    /// The block to open on a free stage: the block just loaded, else the neighbour of the block
    /// that went away, else the first block (lane A, then lane B).
    fn next_block(&mut self, state: &crate::backend::ChainViewState) -> Option<String> {
        if let Some(id) = self.pending.take()
            && place_of(state, &id).is_some()
        {
            return Some(id);
        }
        if let Some((id, right, index)) = &self.shown {
            if place_of(state, id).is_some() {
                return Some(id.clone());
            }
            let lane = tones(state, *right);
            if !lane.is_empty() {
                return Some(lane[(*index).min(lane.len() - 1)].clone());
            }
        }
        tones(state, false)
            .into_iter()
            .next()
            .or_else(|| tones(state, true).into_iter().next())
    }
}

/// The ids of the tone blocks of one lane, in order.
fn tones(state: &crate::backend::ChainViewState, right: bool) -> Vec<String> {
    let lane = if right {
        &state.chain.right
    } else {
        &state.chain.left
    };
    lane.iter()
        .filter(|block| !block.is_insert)
        .map(|block| block.id.0.clone())
        .collect()
}

/// The lane (right or not) and the position among that lane's tone blocks of block `id`.
fn place_of(state: &crate::backend::ChainViewState, id: &str) -> Option<(bool, usize)> {
    for right in [false, true] {
        if let Some(index) = tones(state, right).iter().position(|other| other == id) {
            return Some((right, index));
        }
    }
    None
}

/// The pedals right before and right after `id` in its lane, when `id` is a pedal: the
/// unbroken run of tone blocks around it whose render is a pedal.
pub(super) fn pedal_run(
    state: &crate::backend::ChainViewState,
    id: &ronnie_core::BlockId,
) -> (Vec<ronnie_core::ChainBlock>, Vec<ronnie_core::ChainBlock>) {
    let is_pedal = |block: &ronnie_core::ChainBlock| {
        !block.is_insert
            && super::skins::skin_for(block)
                .is_some_and(|skin| skin.category == ronnie_core::gear::GearCategory::Pedal)
    };
    for lane in [&state.chain.left, &state.chain.right] {
        let Some(index) = lane.iter().position(|block| &block.id == id) else {
            continue;
        };
        if !is_pedal(&lane[index]) {
            return (Vec::new(), Vec::new());
        }
        let mut before: Vec<_> = lane[..index]
            .iter()
            .rev()
            .take_while(|block| is_pedal(block))
            .cloned()
            .collect();
        before.reverse();
        let after = lane[index + 1..]
            .iter()
            .take_while(|block| is_pedal(block))
            .cloned()
            .collect();
        return (before, after);
    }
    (Vec::new(), Vec::new())
}

/// Where the stage of block `id` offers a slot: a pedal after the last pedal of a run. Other
/// gear is added from the + menu, which places it by its kind.
pub(super) fn stage_slots(
    state: &crate::backend::ChainViewState,
    id: &ronnie_core::BlockId,
) -> super::block::StageSlots {
    use ronnie_core::gear::GearCategory::Pedal;
    let category = |block: &ronnie_core::ChainBlock| {
        if block.is_insert {
            return None;
        }
        super::skins::skin_for(block).map(|skin| skin.category)
    };
    for lane in [&state.chain.left, &state.chain.right] {
        let Some(index) = lane.iter().position(|block| &block.id == id) else {
            continue;
        };
        let mut slots = super::block::StageSlots::default();
        if category(&lane[index]) == Some(Pedal) {
            let last = lane[index..]
                .iter()
                .take_while(|block| category(block) == Some(Pedal))
                .last()
                .unwrap_or(&lane[index]);
            slots.pedal_after = Some(last.id.clone());
        }
        return slots;
    }
    super::block::StageSlots::default()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use eframe::egui;
    use ronnie_core::BlockId;

    use crate::{
        backend::MockBackend,
        default::{ChromeController, GallerySelection},
        driver::UiDriver,
        shell::ViewMode,
        testbed::{UiScenario, load_scenarios},
    };

    fn scenario(id: &str) -> UiScenario {
        load_scenarios(include_str!("../../../../../fixtures/ui-scenarios.json"))
            .expect("scenarios")
            .into_iter()
            .find(|scenario| scenario.id == id)
            .expect("scenario exists")
    }

    fn stage_driver(id: &str) -> (UiDriver, Arc<MockBackend>) {
        let backend = Arc::new(MockBackend::from_scenario(&scenario(id)));
        let mut chrome = ChromeController::new(backend.clone(), Vec::new(), false, true);
        chrome.set_view_mode(ViewMode::RonnieFlow);
        let mut driver = UiDriver::new(chrome, (*backend).clone());
        driver.settle();
        (driver, backend)
    }

    fn key_event(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn delete_opens_the_neighbour_without_a_frame_of_the_reference_gallery() {
        let (mut driver, _backend) = stage_driver("main-mono");
        // One pass only: the pass in which the key removes the open block. Settling would hide
        // the frame that falls through to the reference chain gallery.
        driver.play(vec![key_event(egui::Key::Delete, true)]);

        assert!(
            !driver.is_visible("Add Tone"),
            "the reference chain gallery must not draw inside the Stage: {:?}",
            driver.visible_labels()
        );
        assert!(
            driver.chrome().detail().is_some(),
            "the neighbour is on the stage in the same pass"
        );
        assert!(
            matches!(
                driver.chrome().gallery().selection(),
                GallerySelection::BlockDetail(id) if *id == BlockId("blk-2".to_owned())
            ),
            "the neighbour opens: {:?}",
            driver.chrome().gallery().selection()
        );
    }

    #[test]
    fn an_open_add_menu_does_not_force_an_immediate_repaint() {
        let (mut driver, _backend) = stage_driver("main-mono");
        let _ = driver.click("Add a block").expect("the + button");
        // The menu fades in over its first frames; only after that is an idle pass free of
        // animation.
        for _ in 0..12 {
            driver.settle();
        }
        // One idle pass of the open menu: nothing on it animates, so the app's 30 Hz cadence
        // must decide when the next frame runs.
        driver.play(Vec::new());
        assert!(
            !driver.context().has_requested_repaint(),
            "the open menu asked for a display-refresh repaint: {:?}",
            driver
                .context()
                .repaint_causes()
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        );
    }
}
