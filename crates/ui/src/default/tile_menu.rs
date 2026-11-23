//! Context-menu items shared by tone and insert tiles.

/// One context-menu action label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileMenuItem {
    /// Copy the block.
    Copy,
    /// Paste the clipboard.
    Paste,
    /// Load one local file.
    LoadFile,
    /// Load a folder of models.
    LoadFolder,
    /// Remove the block.
    Remove,
}

/// Returns menu items appropriate to a tone or insert tile.
#[must_use]
pub fn items(is_insert: bool, can_paste: bool) -> Vec<TileMenuItem> {
    let _ = can_paste;
    let mut items = Vec::new();
    if is_insert {
        items.push(TileMenuItem::Paste);
    } else {
        items.push(TileMenuItem::Copy);
    }
    items.push(TileMenuItem::LoadFile);
    items.push(TileMenuItem::LoadFolder);
    if !is_insert {
        items.push(TileMenuItem::Remove);
    }
    items
}
