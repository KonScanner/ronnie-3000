//! Preset editing, browsing, and optimistic accepted-edit state.

use std::sync::Arc;

use ronie_core::{ActivePreset, PresetInfo};

use crate::backend::{UiAction, UiBackend, UiError};

/// Preset controller backed only by host-neutral actions.
pub struct PresetController {
    backend: Arc<dyn UiBackend>,
    presets: Vec<PresetInfo>,
    active: Option<ActivePreset>,
    draft: String,
    search: String,
    reordering: bool,
    show_pc: bool,
    save_error: Option<String>,
    last_edit_ms: Option<u64>,
    last_revision: Option<u64>,
}

impl PresetController {
    /// Creates a controller from an existing preset list and backend session state.
    #[must_use]
    pub fn new(backend: Arc<dyn UiBackend>, presets: Vec<PresetInfo>) -> Self {
        let active = backend.chain_state(0).session.active_preset.clone();
        Self {
            backend,
            presets,
            active,
            draft: String::new(),
            search: String::new(),
            reordering: false,
            show_pc: false,
            save_error: None,
            last_edit_ms: None,
            last_revision: None,
        }
    }

    /// Visible presets in store order.
    #[must_use]
    pub fn presets(&self) -> &[PresetInfo] {
        &self.presets
    }

    /// Current case-insensitive preset search.
    #[must_use]
    pub fn search(&self) -> &str {
        &self.search
    }

    /// Replaces the preset search without changing storage.
    pub fn set_search(&mut self, search: impl Into<String>) {
        self.search = search.into();
    }

    /// Whether rows show reorder controls.
    #[must_use]
    pub const fn reordering(&self) -> bool {
        self.reordering
    }

    /// Toggles the reorder controls.
    pub fn toggle_reordering(&mut self) {
        self.reordering = !self.reordering;
    }

    /// Active preset display name.
    #[must_use]
    pub fn active_preset_name(&self) -> Option<&str> {
        self.active.as_ref().map(|preset| preset.name.as_str())
    }

    /// Current save draft.
    #[must_use]
    pub fn draft(&self) -> &str {
        &self.draft
    }

    /// Replaces the save draft without touching storage.
    pub fn set_draft(&mut self, draft: impl Into<String>) {
        self.draft = draft.into();
        self.save_error = None;
    }

    /// Latest save error shown inside the still-open editor.
    #[must_use]
    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    /// Saves or updates a same-name user preset.
    ///
    /// # Errors
    /// Returns validation or storage errors and leaves the editor open.
    pub fn save(&mut self) -> Result<(), UiError> {
        let name = self.draft.trim().to_owned();
        if name.is_empty() {
            let error = UiError::InvalidAction("preset name is empty".to_owned());
            self.save_error = Some(error.to_string());
            return Err(error);
        }
        if let Err(error) = self
            .backend
            .dispatch(UiAction::SavePreset { name: name.clone() })
        {
            self.save_error = Some(error.to_string());
            return Err(error);
        }
        self.save_error = None;
        if let Some(existing) = self
            .presets
            .iter_mut()
            .find(|preset| !preset.factory && preset.name.eq_ignore_ascii_case(&name))
        {
            existing.name = name;
        } else {
            let id = self
                .backend
                .chain_state(0)
                .session
                .active_preset
                .as_ref()
                .map(|preset| preset.id.clone())
                .unwrap_or_else(|| format!("user:{}", name.to_lowercase().replace(' ', "-")));
            self.presets.push(PresetInfo {
                id,
                name,
                factory: false,
            });
        }
        self.active = self.backend.chain_state(0).session.active_preset.clone();
        Ok(())
    }

    /// Loads a preset by ID.
    ///
    /// # Errors
    /// Returns an unknown-preset or storage error.
    pub fn load_preset(&mut self, id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::LoadPreset { id: id.to_owned() })?;
        self.active = self
            .presets
            .iter()
            .find(|preset| preset.id == id)
            .map(|preset| ActivePreset {
                id: preset.id.clone(),
                name: preset.name.clone(),
            })
            .or_else(|| self.backend.chain_state(0).session.active_preset.clone());
        Ok(())
    }

    /// Renames a user preset after backend acceptance.
    ///
    /// # Errors
    /// Returns validation, missing-target, or storage errors.
    pub fn rename_preset(&mut self, id: &str, name: &str) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::RenamePreset {
            id: id.to_owned(),
            name: name.to_owned(),
        })?;
        if let Some(preset) = self.presets.iter_mut().find(|preset| preset.id == id) {
            preset.name = name.to_owned();
        }
        self.active = self.backend.chain_state(0).session.active_preset.clone();
        Ok(())
    }

    /// Deletes a user preset after backend acceptance.
    ///
    /// # Errors
    /// Returns missing-target or storage errors.
    pub fn delete_preset(&mut self, id: &str) -> Result<(), UiError> {
        self.backend
            .dispatch(UiAction::DeletePreset { id: id.to_owned() })?;
        self.presets.retain(|preset| preset.id != id);
        self.active = self.backend.chain_state(0).session.active_preset.clone();
        Ok(())
    }

    /// Reorders within one factory or user section after backend acceptance.
    ///
    /// # Errors
    /// Returns an invalid-move or storage error.
    pub fn move_preset(&mut self, id: &str, delta: i32) -> Result<(), UiError> {
        self.backend.dispatch(UiAction::MovePreset {
            id: id.to_owned(),
            delta,
        })?;
        let Some(index) = self.presets.iter().position(|preset| preset.id == id) else {
            return Ok(());
        };
        let factory = self.presets[index].factory;
        let target =
            (index as i32 + delta).clamp(0, self.presets.len().saturating_sub(1) as i32) as usize;
        if self.presets[target].factory != factory {
            return Ok(());
        }
        let preset = self.presets.remove(index);
        self.presets.insert(target, preset);
        Ok(())
    }

    /// Loads the next preset, wrapping to the first when none is active.
    ///
    /// # Errors
    /// Returns a backend load error.
    pub fn next_preset(&mut self) -> Result<(), UiError> {
        self.step(1)
    }

    /// Loads the previous preset, wrapping to the last when none is active.
    ///
    /// # Errors
    /// Returns a backend load error.
    pub fn previous_preset(&mut self) -> Result<(), UiError> {
        self.step(-1)
    }

    fn step(&mut self, direction: i32) -> Result<(), UiError> {
        if self.presets.is_empty() {
            return Ok(());
        }
        let current = self.active.as_ref().and_then(|active| {
            self.presets
                .iter()
                .position(|preset| preset.id == active.id)
        });
        let next = match current {
            Some(index) => {
                (index as i32 + direction).rem_euclid(self.presets.len() as i32) as usize
            }
            None if direction > 0 => 0,
            None => self.presets.len() - 1,
        };
        let id = self.presets[next].id.clone();
        self.load_preset(&id)
    }

    /// Changes MIDI program-change badge visibility.
    pub fn set_show_pc(&mut self, show: bool) {
        self.show_pc = show;
    }

    /// One-based three-digit PC badge, or `None` when hidden.
    #[must_use]
    pub fn pc_badge(&self, index: usize) -> Option<String> {
        self.show_pc.then(|| format!("{:03}", index + 1))
    }

    /// Coalesces revisions for 400 ms while one edit remains active.
    #[must_use]
    pub fn coalesce_edit(&mut self, now_ms: u64, revision: u64) -> u64 {
        let Some(last_ms) = self.last_edit_ms else {
            self.last_edit_ms = Some(now_ms);
            self.last_revision = Some(revision);
            return revision;
        };
        if now_ms.saturating_sub(last_ms) < 400 {
            self.last_revision.unwrap_or(revision)
        } else {
            self.last_edit_ms = Some(now_ms);
            self.last_revision = Some(revision);
            revision
        }
    }
}
