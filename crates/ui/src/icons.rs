//! Neutral icon identities and per-editor immutable asset caching.

use std::collections::BTreeMap;

/// Icons shared by chrome and controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IconId {
    /// Back navigation.
    Back,
    /// Close or dismiss.
    Close,
    /// Add or insert.
    Plus,
    /// Save preset.
    Save,
    /// Undo.
    Undo,
    /// Redo.
    Redo,
    /// Tuner.
    Tuner,
    /// Menu.
    Menu,
    /// Account.
    Account,
    /// Search.
    Search,
}

/// Cached immutable icon source.
#[derive(Debug, Clone)]
pub struct Icon {
    /// Source identity.
    pub id: IconId,
    /// Neutral SVG source with no product mark.
    pub svg: &'static str,
}

/// Per-editor icon cache. Cloning the editor never reparses an icon.
#[derive(Debug, Default)]
pub struct IconCache {
    entries: BTreeMap<IconId, Icon>,
}

impl IconCache {
    /// Returns one cached icon and inserts it on first use.
    #[must_use]
    pub fn get(&mut self, id: IconId) -> &Icon {
        self.entries.entry(id).or_insert_with(|| Icon {
            id,
            svg: source(id),
        })
    }
}

fn source(id: IconId) -> &'static str {
    match id {
        IconId::Back => {
            "<svg viewBox=\"0 0 24 24\"><path d=\"m12 19-7-7 7-7M5 12h14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></svg>"
        }
        IconId::Close => {
            "<svg viewBox=\"0 0 24 24\"><path d=\"M18 6 6 18M6 6l12 12\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\"/></svg>"
        }
        IconId::Plus => {
            "<svg viewBox=\"0 0 24 24\"><path d=\"M12 5v14M5 12h14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\"/></svg>"
        }
        IconId::Save => {
            "<svg viewBox=\"0 0 24 24\"><path d=\"M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2ZM7 3v6h10V3M7 21v-8h10v8\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linejoin=\"round\"/></svg>"
        }
        IconId::Undo => {
            "<svg viewBox=\"0 0 24 24\"><path d=\"M3 10h10a7 7 0 0 1 0 14M3 10l6-6M3 10l6 6\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></svg>"
        }
        IconId::Redo => {
            "<svg viewBox=\"0 0 24 24\"><path d=\"M21 10H11a7 7 0 0 0 0 14m10-14-6-6m6 6-6 6\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></svg>"
        }
        IconId::Tuner => {
            "<svg viewBox=\"0 0 24 24\"><path d=\"M2 12h4l3-9 4 18 3-9h6\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></svg>"
        }
        IconId::Menu => {
            "<svg viewBox=\"0 0 24 24\"><path d=\"M4 6h16M4 12h16M4 18h16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\"/></svg>"
        }
        IconId::Account => {
            "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"8\" r=\"4\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\"/><path d=\"M4 21a8 8 0 0 1 16 0\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\"/></svg>"
        }
        IconId::Search => {
            "<svg viewBox=\"0 0 24 24\"><circle cx=\"11\" cy=\"11\" r=\"7\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\"/><path d=\"m20 20-4-4\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\"/></svg>"
        }
    }
}
