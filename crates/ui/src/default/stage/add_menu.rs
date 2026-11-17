//! The black menu that the + button opens: the gear categories, the lane that receives the
//! block, and the ways to load a file or open the tone browser.

use eframe::egui::{self, Align, Color32, FontFamily, FontId, Layout, Rect, RichText, Stroke};
use ronie_core::{BlockId, ChainSide, ModelKind, Tone, ToneId, gear::GearCategory};

use crate::default::Placement;
use crate::services::{
    catalog::{CatalogStatus, CatalogStore},
    recent_tones::{RecentTones, same_tone},
};

use super::super::{
    tile::{GearGlyph, draw_glyph, tone_glyph},
    tone_card::model_count,
};

const MENU_WIDTH: f32 = 380.0;
const PADDING: f32 = 8.0;
const HEAD: f32 = 34.0;
const ROW: f32 = 44.0;
const LINK: f32 = 36.0;
/// A tone row of the results.
const TONE_ROW: f32 = 40.0;
/// The search field and its margins.
const SEARCH: f32 = 4.0 + 30.0 + 2.0;
/// A section title such as YOUR LIBRARY.
const SECTION: f32 = 28.0;
/// A muted line in place of rows.
const NOTE: f32 = 28.0;
/// The sign-in box and its margins.
const SIGN_IN_BOX: f32 = 2.0 + 62.0 + 6.0;
/// The hairline between the results and the link below them.
const DIVIDER: f32 = 13.0;
/// The most library rows and TONE3000 rows that the results show.
const LIBRARY_ROWS: usize = 3;
const REMOTE_ROWS: usize = 4;
const GAP_BELOW_ANCHOR: f32 = 6.0;
/// The gap between a stage slot and the menu that it opened beside it.
const GAP_BESIDE_ANCHOR: f32 = 10.0;
/// The longest block title that the header of a slot's menu prints whole.
const PLACE_TITLE_CHARS: usize = 18;

const BACKGROUND: Color32 = Color32::from_rgb(0x0b, 0x0b, 0x0b);
pub(super) const TEXT: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
pub(super) const MUTED: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x87);
const SOFT: Color32 = Color32::from_rgb(0xbd, 0xbd, 0xbd);
/// A row or an item under the pointer on a black surface: the menus and the utility bar.
pub(super) const HOVER: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x1d);
pub(super) const HAIRLINE: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
const FIELD: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
const FIELD_BORDER: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2e);

/// What the menu shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuView {
    /// The gear categories and the two links.
    Categories,
    /// The tones of one category.
    Results(GearCategory),
}

/// What the player did in the menu.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum AddMenuAction {
    /// Open the tone browser, narrowed to a category and the search text when there is one.
    OpenBrowser(Option<GearCategory>),
    /// Pick a local model file.
    LoadFile,
    /// Add a tone to the chosen lane.
    Add(Tone),
    /// Start the sign-in.
    SignIn,
}

/// The menu's own state; it lives in egui temp data between frames.
#[derive(Debug, Clone)]
pub(super) struct AddMenu {
    pub(super) open: bool,
    pub(super) view: MenuView,
    pub(super) lane: ChainSide,
    pub(super) search: String,
    /// Whether the search field takes the keyboard at the next frame.
    focus_search: bool,
    /// Whether a tone that the menu added is still downloading, so the menu stays open.
    pub(super) adding: bool,
    /// Where an added block lands in its lane.
    pub(super) placement: Placement,
    /// The stage slot that opened the menu beside it, or `None` for the + button.
    pub(super) anchor: Option<Rect>,
    /// The kind of gear of the slot that opened the menu.
    pub(super) slot: Option<GearCategory>,
    /// The block whose tone the menu replaces, or `None` for an add.
    pub(super) replacing: Option<BlockId>,
    /// Whether the REPLACE button of the block page opened the menu, so that the menu follows
    /// that button when the page lays out again.
    pub(super) on_page_button: bool,
    /// Whether the menu opened on this frame from a click elsewhere, such as a row of the strip's
    /// block menu, so that this click does not count as a click outside the menu.
    pub(super) fresh: bool,
}

impl Default for AddMenu {
    fn default() -> Self {
        Self {
            open: false,
            view: MenuView::Categories,
            lane: ChainSide::Left,
            search: String::new(),
            focus_search: false,
            adding: false,
            placement: Placement::End,
            anchor: None,
            slot: None,
            replacing: None,
            on_page_button: false,
            fresh: false,
        }
    }
}

impl AddMenu {
    fn id() -> egui::Id {
        egui::Id::unique("stage-add-menu")
    }

    pub(super) fn load(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp::<Self>(Self::id()))
            .unwrap_or_default()
    }

    pub(super) fn store(&self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(Self::id(), self.clone()));
    }

    /// Opens the menu on its categories, for one lane.
    pub(super) fn open_for(&mut self, lane: ChainSide) {
        self.open = true;
        self.view = MenuView::Categories;
        self.lane = lane;
        self.search.clear();
        self.adding = false;
        self.placement = Placement::End;
        self.anchor = None;
        self.slot = None;
        self.replacing = None;
        self.on_page_button = false;
    }

    /// Opens the menu beside the stage slot `anchor` on the tones of `category`, for a block that
    /// lands at `placement` in `lane`.
    pub(super) fn open_at(
        &mut self,
        category: GearCategory,
        placement: Placement,
        anchor: Rect,
        lane: ChainSide,
    ) {
        self.open = true;
        self.lane = lane;
        self.adding = false;
        self.placement = placement;
        self.anchor = Some(anchor);
        self.slot = Some(category);
        self.replacing = None;
        self.on_page_button = false;
        self.show_results(category);
    }

    /// Opens the menu under `anchor` to replace the tone of block `block_id`: on the tones of
    /// `category`, or on the categories for a block of no known gear.
    pub(super) fn open_replace(
        &mut self,
        block_id: BlockId,
        category: Option<GearCategory>,
        anchor: Rect,
    ) {
        self.open = true;
        self.adding = false;
        self.placement = Placement::End;
        self.anchor = Some(anchor);
        self.slot = None;
        self.replacing = Some(block_id);
        self.on_page_button = false;
        match category {
            Some(category) => self.show_results(category),
            None => {
                self.view = MenuView::Categories;
                self.search.clear();
            }
        }
    }

    /// The block whose tone the open menu replaces.
    pub(super) fn replaced_block(&self) -> Option<&BlockId> {
        self.replacing.as_ref().filter(|_| self.open)
    }

    /// The slot whose menu is open, by its kind of gear and its placement, which the stage
    /// draws solid.
    pub(super) fn open_slot(&self) -> Option<(GearCategory, Placement)> {
        self.slot
            .filter(|_| self.open)
            .map(|category| (category, self.placement.clone()))
    }

    /// Whether the + button opened the menu.
    pub(super) fn opened_by_plus(&self) -> bool {
        self.open && self.anchor.is_none()
    }

    /// Shows the tones of one category, with an empty search that takes the keyboard.
    fn show_results(&mut self, category: GearCategory) {
        self.view = MenuView::Results(category);
        self.search.clear();
        self.focus_search = true;
    }
}

/// What the menu needs to know about the rest of the app.
pub(super) struct MenuData<'a> {
    pub(super) stereo: bool,
    pub(super) library: &'a RecentTones,
    /// The tones of the chain, in both lanes.
    pub(super) in_chain: Vec<&'a Tone>,
    pub(super) signed_in: bool,
    /// Whether a catalog tone can load, or the player signs in first.
    pub(super) catalog_downloads: bool,
    /// The TONE3000 results of the category on show.
    pub(super) results: Option<&'a CatalogStore>,
    /// The tone that a load is fetching now.
    pub(super) loading: Option<ToneId>,
    /// Where a slot's menu puts the block, such as "after OD808".
    pub(super) place: Option<String>,
    /// The tone of the block that a replace swaps out, or `None` for an add.
    pub(super) replacing: Option<&'a Tone>,
}

/// What the header of a slot's menu says about where the block goes: "after" or "before" and
/// the title of the block beside it, shortened with an ellipsis.
pub(super) fn place_text(word: &str, title: &str) -> String {
    let title = if title.chars().count() > PLACE_TITLE_CHARS {
        let kept: String = title.chars().take(PLACE_TITLE_CHARS - 1).collect();
        format!("{}\u{2026}", kept.trim_end())
    } else {
        title.to_owned()
    };
    format!("{word} {title}")
}

/// Where the caret of the menu sits: on its top edge at an x, or on its left or right edge at
/// a y.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Caret {
    Top(f32),
    Left(f32),
    Right(f32),
}

/// The menu's rect and caret: under the + button, or beside a stage slot (right of it when it
/// fits in the window, else left of it), level with the slot inside the stage.
fn place_menu(anchor: Rect, beside: bool, height: f32, screen: Rect, stage: Rect) -> (Rect, Caret) {
    if !beside {
        let left = (anchor.center().x - MENU_WIDTH / 2.0)
            .clamp(screen.left() + 4.0, screen.right() - MENU_WIDTH - 4.0);
        let rect = Rect::from_min_size(
            egui::pos2(left, anchor.bottom() + GAP_BELOW_ANCHOR),
            egui::vec2(MENU_WIDTH, height),
        );
        let tip = anchor
            .center()
            .x
            .clamp(rect.left() + 20.0, rect.right() - 20.0);
        return (rect, Caret::Top(tip));
    }
    let right_side = anchor.right() + GAP_BESIDE_ANCHOR + MENU_WIDTH <= screen.right() - 4.0;
    let left = if right_side {
        anchor.right() + GAP_BESIDE_ANCHOR
    } else {
        (anchor.left() - GAP_BESIDE_ANCHOR - MENU_WIDTH).max(screen.left() + 4.0)
    };
    let top = (anchor.center().y - height / 2.0)
        .min(stage.bottom() - height)
        .max(stage.top());
    let rect = Rect::from_min_size(egui::pos2(left, top), egui::vec2(MENU_WIDTH, height));
    let tip = anchor.center().y.clamp(
        rect.top() + 20.0,
        (rect.bottom() - 20.0).max(rect.top() + 20.0),
    );
    let caret = if right_side {
        Caret::Left(tip)
    } else {
        Caret::Right(tip)
    };
    (rect, caret)
}

const CATEGORIES: [(GearCategory, GearGlyph, &str, &str); 6] = [
    (
        GearCategory::Pedal,
        GearGlyph::Pedal,
        "PEDAL",
        "Overdrive, distortion, fuzz, boost, compressor",
    ),
    (
        GearCategory::Amp,
        GearGlyph::Amp,
        "AMP",
        "Heads and combos, no cab",
    ),
    (
        GearCategory::FullRig,
        GearGlyph::FullRig,
        "AMP + CAB",
        "A whole rig in one capture",
    ),
    (
        GearCategory::Cab,
        GearGlyph::Cabinet,
        "CAB",
        "Impulse responses, mic positions",
    ),
    (
        GearCategory::Outboard,
        GearGlyph::Outboard,
        "OUTBOARD",
        "Preamps, rack EQ and compressors",
    ),
    (
        GearCategory::Space,
        GearGlyph::Space,
        "SPACE",
        "Rooms and reverbs",
    ),
];

fn category_name(category: GearCategory) -> &'static str {
    CATEGORIES
        .iter()
        .find(|row| row.0 == category)
        .map_or("", |row| row.2)
}

/// The tones of one category in a sentence, such as "amps".
const fn plural(category: GearCategory) -> &'static str {
    match category {
        GearCategory::Pedal => "pedals",
        GearCategory::Amp => "amps",
        GearCategory::FullRig => "rigs",
        GearCategory::Cab => "cabs",
        GearCategory::Outboard => "outboard gear",
        GearCategory::Space => "spaces",
        GearCategory::Unknown => "tones",
    }
}

/// A count with a comma between each group of three digits.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(digit);
    }
    output
}

/// What the right end of a tone row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowState {
    Add,
    /// A catalog tone that loads only after a sign-in.
    SignIn,
    InChain,
    Downloading,
    /// A tone that replaces the block's tone.
    Swap,
    /// The tone of the block that the menu replaces, which a click leaves alone.
    Current,
}

/// The rows that the results view shows, chosen once for the height and the drawing.
struct Results<'a> {
    category: GearCategory,
    library: Vec<&'a Tone>,
    remote: Vec<&'a Tone>,
    /// A line in place of the TONE3000 rows: loading, a failure, or no match.
    remote_note: Option<String>,
}

impl<'a> Results<'a> {
    fn new(category: GearCategory, search: &str, data: &MenuData<'a>) -> Self {
        let needle = search.trim().to_lowercase();
        let library = data
            .library
            .in_category(category)
            .into_iter()
            .filter(|tone| needle.is_empty() || tone.title.to_lowercase().contains(&needle))
            .take(LIBRARY_ROWS)
            .collect();
        let mut remote = Vec::new();
        let mut remote_note = None;
        if data.signed_in {
            let store = data.results;
            if let Some(page) = store.and_then(CatalogStore::result) {
                remote.extend(page.data.iter().take(REMOTE_ROWS));
            }
            if remote.is_empty() {
                remote_note = Some(match store.map(CatalogStore::status) {
                    Some(CatalogStatus::Error) => store
                        .and_then(CatalogStore::error)
                        .unwrap_or("The search failed.")
                        .to_owned(),
                    Some(CatalogStatus::Ready) => {
                        format!("No {} on TONE3000 match.", plural(category))
                    }
                    _ => "Loading\u{2026}".to_owned(),
                });
            }
        }
        Self {
            category,
            library,
            remote,
            remote_note,
        }
    }

    fn height(&self, signed_in: bool) -> f32 {
        let library = if self.library.is_empty() {
            NOTE
        } else {
            TONE_ROW * self.library.len() as f32
        };
        let remote = if !signed_in {
            SIGN_IN_BOX
        } else if self.remote_note.is_some() {
            NOTE
        } else {
            TONE_ROW * self.remote.len() as f32
        };
        SEARCH + SECTION + library + SECTION + remote + DIVIDER + LINK
    }

    fn state_of(tone: &Tone, data: &MenuData<'_>) -> RowState {
        if data.loading.is_some_and(|id| id == tone.id && !tone.local) {
            RowState::Downloading
        } else if data
            .replacing
            .is_some_and(|current| same_tone(current, tone))
        {
            RowState::Current
        } else if !tone.local && !data.catalog_downloads {
            RowState::SignIn
        } else if data.replacing.is_some() {
            RowState::Swap
        } else if data.in_chain.iter().any(|held| same_tone(held, tone)) {
            RowState::InChain
        } else {
            RowState::Add
        }
    }
}

/// The line under a library tone: where it came from.
fn library_line(tone: &Tone) -> String {
    if tone.local {
        let extension = if tone.kind == ModelKind::Ir {
            "wav"
        } else {
            "nam"
        };
        return format!("Local .{extension} file");
    }
    let count = model_count(tone);
    let noun = if count == 1 { "capture" } else { "captures" };
    format!("Downloaded \u{b7} {count} {noun}")
}

/// The line under a TONE3000 tone: its creator and its downloads.
fn remote_line(tone: &Tone) -> String {
    let user = &tone.raw["user"];
    let creator = user["display_name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .or_else(|| user["username"].as_str())
        .unwrap_or("Unknown");
    let count = tone.raw["downloads_count"].as_u64().unwrap_or(0);
    format!("{creator} \u{b7} {} \u{2193}", downloads(count))
}

/// A download count as the menu prints it: whole up to 999, then in thousands or millions with
/// one decimal below a hundred, such as "3.2k", "12.4k" and "124k".
fn downloads(value: u64) -> String {
    let (scaled, unit) = match value {
        0..1_000 => return value.to_string(),
        1_000..1_000_000 => (value as f64 / 1_000.0, "k"),
        _ => (value as f64 / 1_000_000.0, "M"),
    };
    let text = if scaled < 100.0 {
        format!("{scaled:.1}")
    } else {
        format!("{scaled:.0}")
    };
    let text = text.strip_suffix(".0").unwrap_or(&text);
    format!("{text}{unit}")
}

pub(super) fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub(super) fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("arimo-bold".into()))
}

/// Puts one line of text in `rect`, at its left or right edge and centred in height.
pub(super) fn text_in(ui: &mut egui::Ui, rect: Rect, text: RichText, right: bool) {
    let layout = if right {
        Layout::right_to_left(Align::Center)
    } else {
        Layout::left_to_right(Align::Center)
    };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
    child.add(egui::Label::new(text).selectable(false).truncate());
}

/// Lays out one line of text for the painter.
fn galley_of(ui: &egui::Ui, text: RichText) -> std::sync::Arc<egui::Galley> {
    egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::FontSelection::Default,
    )
}

pub(super) fn tracked(text: &str, font: FontId, color: Color32, em: f32) -> RichText {
    let size = font.size;
    RichText::new(text)
        .font(font)
        .color(color)
        .extra_letter_spacing(size * em)
}

pub(super) fn menu_item(ui: &egui::Ui, rect: Rect, id: &str, label: &str) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(id), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, true, label));
    response
}

fn chevron(painter: &egui::Painter, center: egui::Pos2, left: bool, color: Color32) {
    let (a, b) = if left { (2.0, -2.0) } else { (-2.0, 2.0) };
    painter.line(
        vec![
            center + egui::vec2(a, -4.0),
            center + egui::vec2(b, 0.0),
            center + egui::vec2(a, 4.0),
        ],
        Stroke::new(1.5, color),
    );
}

/// Draws the menu under the + button `anchor`, or beside the stage slot of a slot's menu inside
/// `stage`, and returns what the player chose. The menu closes itself on Escape and on a click
/// outside it and outside its anchor.
pub(super) fn show(
    ui: &mut egui::Ui,
    plus: Rect,
    stage: Rect,
    menu: &mut AddMenu,
    data: &MenuData<'_>,
) -> Option<AddMenuAction> {
    let results = match menu.view {
        MenuView::Categories => None,
        MenuView::Results(category) => Some(Results::new(category, &menu.search, data)),
    };
    let height = PADDING * 2.0
        + HEAD
        + results
            .as_ref()
            .map_or(4.0 + ROW * 6.0 + 13.0 + LINK * 2.0, |results| {
                results.height(data.signed_in)
            });
    let screen = ui.ctx().content_rect();
    let anchor = menu.anchor.unwrap_or(plus);
    // A replace opens under its button, as the + button's menu does.
    let beside = menu.anchor.is_some() && menu.replacing.is_none();
    let (rect, caret) = place_menu(anchor, beside, height, screen, stage);
    let mut action = None;
    egui::Area::new(egui::Id::unique("stage-add-menu-area"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ui.ctx(), |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            action = draw(ui, rect, caret, menu, data, results.as_ref());
        });

    let (escape, clicked_at) = ui.input(|input| {
        (
            input.key_pressed(egui::Key::Escape),
            input
                .pointer
                .primary_clicked()
                .then(|| input.pointer.latest_pos())
                .flatten(),
        )
    });
    let fresh = std::mem::take(&mut menu.fresh);
    let outside =
        !fresh && clicked_at.is_some_and(|pos| !rect.contains(pos) && !anchor.contains(pos));
    if escape || outside {
        menu.open = false;
    }
    action
}

/// Paints the black board of a menu: its two soft shadows, its fill, and its caret, outlined
/// when the look asks for it, so that it stands out from a black window.
pub(super) fn paint_board(painter: &egui::Painter, rect: Rect, caret: Caret) {
    let look = crate::theme::Look::get(painter.ctx());
    // The two soft shadows of the board: a wide one far below, and a close one.
    for (offset, blur, alpha) in [(18, 40, 71), (2, 6, 51)] {
        painter.add(
            egui::Shadow {
                offset: [0, offset],
                blur,
                spread: 0,
                color: Color32::from_black_alpha(alpha),
            }
            .as_shape(rect, 8.0),
        );
    }
    let outline = look
        .menu_outline
        .map_or(Stroke::NONE, |color| Stroke::new(1.0, color));
    painter.rect(rect, 8.0, look.menu, outline, egui::StrokeKind::Inside);
    let points = match caret {
        Caret::Top(x) => [
            egui::pos2(x - 6.0, rect.top() + 0.5),
            egui::pos2(x, rect.top() - 6.0),
            egui::pos2(x + 6.0, rect.top() + 0.5),
        ],
        Caret::Left(y) => [
            egui::pos2(rect.left() + 0.5, y + 6.0),
            egui::pos2(rect.left() - 6.0, y),
            egui::pos2(rect.left() + 0.5, y - 6.0),
        ],
        Caret::Right(y) => [
            egui::pos2(rect.right() - 0.5, y - 6.0),
            egui::pos2(rect.right() + 6.0, y),
            egui::pos2(rect.right() - 0.5, y + 6.0),
        ],
    };
    painter.add(egui::Shape::convex_polygon(
        points.to_vec(),
        look.menu,
        Stroke::NONE,
    ));
    if outline != Stroke::NONE {
        // The caret's two outer edges continue the outline; its base stays open into the board.
        painter.line_segment([points[0], points[1]], outline);
        painter.line_segment([points[1], points[2]], outline);
    }
}

fn draw(
    ui: &mut egui::Ui,
    rect: Rect,
    caret: Caret,
    menu: &mut AddMenu,
    data: &MenuData<'_>,
    results: Option<&Results<'_>>,
) -> Option<AddMenuAction> {
    let painter = ui.painter().clone();
    paint_board(&painter, rect, caret);

    let inner = rect.shrink(PADDING);
    let head = Rect::from_min_size(inner.min, egui::vec2(inner.width(), HEAD));
    let mut action = None;

    match menu.view {
        MenuView::Categories => {
            text_in(
                ui,
                Rect::from_min_max(head.min + egui::vec2(14.0, 0.0), head.max),
                tracked(
                    if menu.replacing.is_some() {
                        "REPLACE BLOCK"
                    } else {
                        "ADD BLOCK"
                    },
                    bold(10.0),
                    TEXT,
                    0.14,
                ),
                false,
            );
        }
        MenuView::Results(category) => {
            let title = if menu.replacing.is_some() {
                format!("REPLACE \u{b7} {}", category_name(category))
            } else {
                category_name(category).to_owned()
            };
            let width = galley_of(ui, tracked(&title, bold(10.0), TEXT, 0.14))
                .size()
                .x;
            let back =
                Rect::from_min_size(head.min, egui::vec2((34.0 + width + 8.0).max(120.0), HEAD));
            let response = ui.interact(back, ui.scope_id().with("back"), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Back to categories")
            });
            chevron(
                &painter,
                back.left_center() + egui::vec2(20.0, 0.0),
                true,
                TEXT,
            );
            text_in(
                ui,
                Rect::from_min_max(back.min + egui::vec2(34.0, 0.0), back.max),
                tracked(&title, bold(10.0), TEXT, 0.14),
                false,
            );
            if response.clicked() {
                menu.view = MenuView::Categories;
            }
        }
    }
    lane_picker(ui, head, menu, data);

    if let Some(results) = results
        && menu.view == MenuView::Results(results.category)
    {
        let body = Rect::from_min_max(egui::pos2(inner.left(), head.bottom()), inner.max);
        return draw_results(ui, body, menu, data, results);
    }
    if menu.view == MenuView::Categories {
        let mut top = head.bottom() + 4.0;
        for (category, glyph, name, hint) in CATEGORIES {
            let row = Rect::from_min_size(
                egui::pos2(inner.left(), top),
                egui::vec2(inner.width(), ROW),
            );
            top += ROW;
            let response = menu_item(ui, row, name, name);
            if response.hovered() {
                painter.rect_filled(row, 6.0, HOVER);
            }
            let icon = Rect::from_center_size(
                egui::pos2(row.left() + 14.0 + 13.0, row.center().y),
                egui::Vec2::splat(26.0),
            );
            draw_glyph(&painter, icon, glyph, TEXT);
            let text_left = row.left() + 52.0;
            let text_right = row.right() - 76.0;
            text_in(
                ui,
                Rect::from_min_max(
                    egui::pos2(text_left, row.center().y - 14.5),
                    egui::pos2(text_right, row.center().y),
                ),
                tracked(name, bold(11.0), TEXT, 0.08),
                false,
            );
            text_in(
                ui,
                Rect::from_min_max(
                    egui::pos2(text_left, row.center().y),
                    egui::pos2(text_right, row.center().y + 14.5),
                ),
                RichText::new(hint).font(sans(11.0)).color(MUTED),
                false,
            );
            let count = data.library.count(category);
            text_in(
                ui,
                Rect::from_min_max(
                    egui::pos2(text_right, row.top()),
                    egui::pos2(row.right() - 38.0, row.bottom()),
                ),
                RichText::new(if count == 0 {
                    "\u{2014}".to_owned()
                } else {
                    count.to_string()
                })
                .font(FontId::monospace(10.0))
                .color(MUTED),
                true,
            );
            chevron(
                &painter,
                egui::pos2(row.right() - 20.0, row.center().y),
                false,
                MUTED,
            );
            if response.clicked() {
                menu.show_results(category);
            }
        }
        painter.hline(
            (inner.left() + 6.0)..=(inner.right() - 6.0),
            top + 6.5,
            Stroke::new(1.0, HAIRLINE),
        );
        top += 13.0;
        for (index, (label, hint, mono)) in [
            ("LOAD A FILE\u{2026}", ".nam \u{b7} .wav", true),
            ("OPEN THE TONE BROWSER", "filters, sort, pages", false),
        ]
        .into_iter()
        .enumerate()
        {
            let row = Rect::from_min_size(
                egui::pos2(inner.left(), top),
                egui::vec2(inner.width(), LINK),
            );
            top += LINK;
            let response = menu_item(ui, row, label, label);
            if response.hovered() {
                painter.rect_filled(row, 6.0, HOVER);
            }
            let icon = Rect::from_center_size(
                egui::pos2(row.left() + 14.0 + 7.0, row.center().y),
                egui::Vec2::splat(14.0),
            );
            link_icon(&painter, icon, index == 0);
            text_in(
                ui,
                Rect::from_min_max(
                    egui::pos2(row.left() + 14.0 + 14.0 + 10.0, row.top()),
                    egui::pos2(row.right() - 130.0, row.bottom()),
                ),
                tracked(label, bold(11.0), TEXT, 0.06),
                false,
            );
            text_in(
                ui,
                Rect::from_min_max(
                    egui::pos2(row.right() - 130.0, row.top()),
                    egui::pos2(row.right() - 14.0, row.bottom()),
                ),
                RichText::new(hint)
                    .font(if mono {
                        FontId::monospace(10.0)
                    } else {
                        sans(10.0)
                    })
                    .color(MUTED),
                true,
            );
            if response.clicked() {
                action = Some(if index == 0 {
                    AddMenuAction::LoadFile
                } else {
                    AddMenuAction::OpenBrowser(None)
                });
            }
        }
    }
    action
}

/// The lane choice in the head row, or the line that says where a block goes: in mono, and in
/// a slot's menu, whose lane is the slot's.
fn lane_picker(ui: &mut egui::Ui, head: Rect, menu: &mut AddMenu, data: &MenuData<'_>) {
    let right = head.right() - 6.0;
    let place = if menu.replacing.is_some() {
        Some("keeps trims, EQ and place")
    } else if menu.anchor.is_some() || !data.stereo {
        data.place.as_deref()
    } else {
        None
    };
    if !data.stereo || menu.anchor.is_some() {
        text_in(
            ui,
            Rect::from_min_max(
                egui::pos2(right - 200.0, head.top()),
                egui::pos2(right, head.bottom()),
            ),
            RichText::new(place.unwrap_or(if menu.view == MenuView::Categories {
                // The browser and file links here place by type, as a pedal may land before the amp.
                "placed by type"
            } else {
                "to the end of the chain"
            }))
            .font(sans(10.0))
            .color(MUTED),
            true,
        );
        return;
    }
    const SEGMENT: f32 = 64.0;
    let track = Rect::from_center_size(
        egui::pos2(right - (SEGMENT + 2.0), head.center().y),
        egui::vec2(SEGMENT * 2.0 + 4.0, 22.0),
    );
    let painter = ui.painter().clone();
    painter.rect_filled(track, 4.0, HOVER);
    text_in(
        ui,
        Rect::from_min_max(
            egui::pos2(track.left() - 28.0, head.top()),
            egui::pos2(track.left() - 8.0, head.bottom()),
        ),
        RichText::new("to").font(sans(10.0)).color(MUTED),
        true,
    );
    for (index, (side, label)) in [(ChainSide::Left, "LANE A"), (ChainSide::Right, "LANE B")]
        .into_iter()
        .enumerate()
    {
        let segment = Rect::from_min_size(
            track.min + egui::vec2(2.0 + SEGMENT * index as f32, 2.0),
            egui::vec2(SEGMENT, 18.0),
        );
        let chosen = menu.lane == side;
        let response = ui.interact(segment, ui.scope_id().with(label), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::RadioButton, true, chosen, label)
        });
        if chosen {
            painter.rect_filled(segment, 3.0, TEXT);
        }
        let galley = galley_of(
            ui,
            tracked(
                label,
                bold(10.0),
                if chosen { BACKGROUND } else { SOFT },
                0.08,
            ),
        );
        painter.galley(segment.center() - galley.size() / 2.0, galley, BACKGROUND);
        if response.clicked() {
            menu.lane = side;
        }
    }
}

/// The small outline icon of a link row: a page, or a magnifier.
fn link_icon(painter: &egui::Painter, rect: Rect, page: bool) {
    let stroke = Stroke::new(1.5, TEXT);
    if page {
        // A page with a folded corner, on the 24-point grid of the board's icon.
        let point = |x: f32, y: f32| rect.min + egui::vec2(x, y) * (rect.width() / 24.0);
        painter.add(egui::Shape::closed_line(
            vec![
                point(4.0, 3.0),
                point(14.0, 3.0),
                point(20.0, 9.0),
                point(20.0, 21.0),
                point(4.0, 21.0),
            ],
            stroke,
        ));
        painter.line(
            vec![point(14.0, 3.0), point(14.0, 9.0), point(20.0, 9.0)],
            stroke,
        );
    } else {
        let lens = rect.center() - egui::vec2(1.5, 1.5);
        painter.circle_stroke(lens, 4.5, stroke);
        painter.line_segment(
            [
                lens + egui::vec2(3.2, 3.2),
                rect.right_bottom() - egui::vec2(1.0, 1.0),
            ],
            stroke,
        );
    }
}

/// Draws the search field, the library, TONE3000, and the link to the browser, from the top of
/// `body`.
fn draw_results(
    ui: &mut egui::Ui,
    body: Rect,
    menu: &mut AddMenu,
    data: &MenuData<'_>,
    results: &Results<'_>,
) -> Option<AddMenuAction> {
    let painter = ui.painter().clone();
    let mut action = None;
    let mut top = body.top();
    let noun = plural(results.category);

    let field = Rect::from_min_size(
        egui::pos2(body.left() + 6.0, top + 4.0),
        egui::vec2(body.width() - 12.0, 30.0),
    );
    top += SEARCH;
    let hint = if data.signed_in {
        format!("Search {noun} in your library and on TONE3000")
    } else {
        format!("Search {noun} in your library")
    };
    search_field(ui, field, menu, &hint);

    top = section(ui, body, top, "YOUR LIBRARY", None);
    if results.library.is_empty() {
        let note = if menu.search.trim().is_empty() {
            "Nothing yet. Tones you load show up here."
        } else {
            "No tone in your library matches."
        };
        top = note_line(ui, body, top, note);
    }
    for (index, tone) in results.library.iter().enumerate() {
        let row = Rect::from_min_size(
            egui::pos2(body.left(), top),
            egui::vec2(body.width(), TONE_ROW),
        );
        top += TONE_ROW;
        let state = Results::state_of(tone, data);
        if tone_row(
            ui,
            &painter,
            row,
            ("library", index),
            tone,
            &library_line(tone),
            state,
        ) {
            action = Some(row_action(tone, state));
        }
    }

    top = section(
        ui,
        body,
        top,
        "TONE3000",
        (data.signed_in && menu.search.trim().is_empty()).then_some("TRENDING"),
    );
    if data.signed_in {
        if let Some(note) = &results.remote_note {
            top = note_line(ui, body, top, note);
        }
        for (index, tone) in results.remote.iter().enumerate() {
            let row = Rect::from_min_size(
                egui::pos2(body.left(), top),
                egui::vec2(body.width(), TONE_ROW),
            );
            top += TONE_ROW;
            let state = Results::state_of(tone, data);
            if tone_row(
                ui,
                &painter,
                row,
                ("remote", index),
                tone,
                &remote_line(tone),
                state,
            ) {
                action = Some(row_action(tone, state));
            }
        }
    } else {
        let card = Rect::from_min_size(
            egui::pos2(body.left() + 6.0, top + 2.0),
            egui::vec2(body.width() - 12.0, 62.0),
        );
        top += SIGN_IN_BOX;
        if sign_in_box(ui, &painter, card) {
            action = Some(AddMenuAction::SignIn);
        }
    }

    painter.hline(
        (body.left() + 6.0)..=(body.right() - 6.0),
        top + 6.5,
        Stroke::new(1.0, HAIRLINE),
    );
    top += DIVIDER;
    let label = if !data.signed_in {
        "OPEN THE TONE BROWSER".to_owned()
    } else if let Some(page) = data.results.and_then(CatalogStore::result) {
        format!(
            "SHOW ALL {} {} IN THE TONE BROWSER",
            grouped(page.total),
            noun.to_uppercase()
        )
    } else {
        format!("SHOW ALL {} IN THE TONE BROWSER", noun.to_uppercase())
    };
    let row = Rect::from_min_size(egui::pos2(body.left(), top), egui::vec2(body.width(), LINK));
    let response = menu_item(ui, row, "show-all", &label);
    if response.hovered() {
        painter.rect_filled(row, 6.0, HOVER);
    }
    text_in(
        ui,
        Rect::from_min_max(
            egui::pos2(row.left() + 14.0, row.top()),
            egui::pos2(row.right() - 34.0, row.bottom()),
        ),
        tracked(&label, bold(11.0), TEXT, 0.06),
        false,
    );
    let arrow = egui::pos2(row.right() - 20.0, row.center().y);
    painter.hline(
        (arrow.x - 6.0)..=(arrow.x + 5.0),
        arrow.y,
        Stroke::new(1.5, TEXT),
    );
    chevron(&painter, arrow + egui::vec2(3.0, 0.0), false, TEXT);
    if response.clicked() {
        action = Some(AddMenuAction::OpenBrowser(Some(results.category)));
    }
    action
}

/// The dark search field. Its text is the search of the menu.
fn search_field(ui: &mut egui::Ui, field: Rect, menu: &mut AddMenu, hint: &str) {
    let painter = ui.painter().clone();
    painter.rect(
        field,
        5.0,
        FIELD,
        Stroke::new(1.0, FIELD_BORDER),
        egui::StrokeKind::Inside,
    );
    let lens = field.left_center() + egui::vec2(15.0, -1.0);
    let stroke = Stroke::new(1.5, MUTED);
    painter.circle_stroke(lens, 4.0, stroke);
    painter.line_segment(
        [lens + egui::vec2(3.0, 3.0), lens + egui::vec2(6.0, 6.0)],
        stroke,
    );
    let editor = Rect::from_min_max(
        field.min + egui::vec2(31.0, 0.0),
        field.max - egui::vec2(8.0, 0.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(editor)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.visuals_mut().text_cursor.stroke.color = TEXT;
    child.visuals_mut().extreme_bg_color = FIELD;
    let response = child.add(
        egui::TextEdit::singleline(&mut menu.search)
            .id(egui::Id::unique("stage-add-search"))
            .hint_text(RichText::new(hint).color(MUTED))
            .font(sans(12.0))
            .text_color(TEXT)
            .frame(egui::Frame::NONE)
            .desired_width(editor.width()),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, hint));
    if std::mem::take(&mut menu.focus_search) {
        response.request_focus();
    }
}

/// A small section title such as YOUR LIBRARY, with an optional note at its right. Returns the
/// top of what follows.
fn section(ui: &mut egui::Ui, body: Rect, top: f32, title: &str, note: Option<&str>) -> f32 {
    let line = Rect::from_min_max(
        egui::pos2(body.left() + 14.0, top + 10.0),
        egui::pos2(body.right() - 14.0, top + SECTION - 6.0),
    );
    text_in(ui, line, tracked(title, bold(9.5), MUTED, 0.14), false);
    if let Some(note) = note {
        text_in(ui, line, tracked(note, sans(9.5), MUTED, 0.04), true);
    }
    top + SECTION
}

/// A muted line in place of rows. Returns the top of what follows.
fn note_line(ui: &mut egui::Ui, body: Rect, top: f32, text: &str) -> f32 {
    text_in(
        ui,
        Rect::from_min_max(
            egui::pos2(body.left() + 14.0, top),
            egui::pos2(body.right() - 14.0, top + NOTE),
        ),
        RichText::new(text).font(sans(11.0)).color(MUTED),
        false,
    );
    top + NOTE
}

/// What a click on a tone row asks for.
fn row_action(tone: &Tone, state: RowState) -> AddMenuAction {
    if state == RowState::SignIn {
        AddMenuAction::SignIn
    } else {
        AddMenuAction::Add(tone.clone())
    }
}

/// One tone row: its icon, title, line, and what a click does. Returns whether it was clicked
/// to add the tone, or to sign in first.
fn tone_row(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    row: Rect,
    id: (&str, usize),
    tone: &Tone,
    line: &str,
    state: RowState,
) -> bool {
    let response = menu_item(ui, row, &format!("{}-{}", id.0, id.1), &tone.title);
    if response.hovered() && state != RowState::Current {
        painter.rect_filled(row, 6.0, HOVER);
    }
    let icon = Rect::from_center_size(
        egui::pos2(row.left() + 14.0 + 12.0, row.center().y),
        egui::Vec2::splat(24.0),
    );
    draw_glyph(painter, icon, tone_glyph(tone, tone.models.first()), TEXT);
    let text_left = row.left() + 14.0 + 24.0 + 12.0;
    let text_right = row.right() - 14.0 - 92.0;
    text_in(
        ui,
        Rect::from_min_max(
            egui::pos2(text_left, row.center().y - 14.0),
            egui::pos2(text_right, row.center().y),
        ),
        RichText::new(&tone.title).font(sans(12.0)).color(TEXT),
        false,
    );
    text_in(
        ui,
        Rect::from_min_max(
            egui::pos2(text_left, row.center().y),
            egui::pos2(text_right, row.center().y + 13.0),
        ),
        RichText::new(line).font(sans(10.5)).color(MUTED),
        false,
    );
    let accent = crate::theme::Look::get(ui.ctx()).accent;
    let (label, color) = match state {
        RowState::Add if response.hovered() => ("ADD", accent),
        RowState::Add => ("ADD", MUTED),
        RowState::SignIn if response.hovered() => ("SIGN IN", accent),
        RowState::SignIn => ("SIGN IN", MUTED),
        RowState::InChain => ("IN CHAIN", MUTED),
        RowState::Downloading => ("DOWNLOADING", MUTED),
        RowState::Swap if response.hovered() => ("SWAP", accent),
        RowState::Swap => ("SWAP", MUTED),
        RowState::Current => ("CURRENT", MUTED),
    };
    text_in(
        ui,
        Rect::from_min_max(
            egui::pos2(text_right, row.top()),
            egui::pos2(row.right() - 14.0, row.bottom()),
        ),
        tracked(label, bold(10.0), color, 0.08),
        true,
    );
    if state == RowState::Downloading {
        // The load reports no progress, so a short piece of the bar slides along it.
        let track = Rect::from_min_max(
            egui::pos2(row.left() + 14.0, row.bottom() - 5.0),
            egui::pos2(row.right() - 14.0, row.bottom() - 3.0),
        );
        painter.rect_filled(track, 1.0, HAIRLINE);
        let piece = track.width() * 0.3;
        let phase = (ui.input(|input| input.time) * 0.8).fract() as f32;
        let left = track.left() - piece + (track.width() + piece) * phase;
        painter.with_clip_rect(track).rect_filled(
            Rect::from_min_size(egui::pos2(left, track.top()), egui::vec2(piece, 2.0)),
            1.0,
            accent,
        );
    }
    response.clicked() && !matches!(state, RowState::Downloading | RowState::Current)
}

/// The box that takes the place of the TONE3000 rows for a signed-out user. Returns whether
/// its button was clicked.
fn sign_in_box(ui: &mut egui::Ui, painter: &egui::Painter, card: Rect) -> bool {
    painter.rect_stroke(
        card,
        6.0,
        Stroke::new(1.0, HAIRLINE),
        egui::StrokeKind::Inside,
    );
    let label = galley_of(ui, tracked("SIGN IN", bold(10.0), BACKGROUND, 0.08));
    let width = label.size().x + 28.0;
    let button = Rect::from_min_size(
        egui::pos2(card.right() - 14.0 - width, card.center().y - 14.0),
        egui::vec2(width, 28.0),
    );
    let text = Rect::from_min_max(
        egui::pos2(card.left() + 14.0, card.top() + 8.0),
        egui::pos2(button.left() - 12.0, card.bottom() - 8.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.add(
        egui::Label::new(
            RichText::new("Sign in to search and download from the TONE3000 community library.")
                .font(sans(11.0))
                .color(SOFT),
        )
        .selectable(false)
        .wrap(),
    );
    let response = ui.interact(button, ui.scope_id().with("sign-in"), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "SIGN IN"));
    painter.rect_filled(button, 5.0, TEXT);
    painter.galley(button.center() - label.size() / 2.0, label, BACKGROUND);
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::{downloads, place_text};

    #[test]
    fn a_long_title_shortens_to_eighteen_characters() {
        assert_eq!(place_text("after", "OD808"), "after OD808");
        assert_eq!(
            place_text("after", "Exactly 18 chars!!"),
            "after Exactly 18 chars!!"
        );
        assert_eq!(
            place_text("before", "'02 Vox AC30/6 Top Boost"),
            "before '02 Vox AC30/6 To\u{2026}"
        );
    }

    #[test]
    fn downloads_read_in_thousands_from_one_thousand() {
        for (value, text) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1k"),
            (3_200, "3.2k"),
            (8_149, "8.1k"),
            (12_400, "12.4k"),
            (99_960, "100k"),
            (124_000, "124k"),
            (1_260_000, "1.3M"),
        ] {
            assert_eq!(downloads(value), text, "{value}");
        }
    }
}
