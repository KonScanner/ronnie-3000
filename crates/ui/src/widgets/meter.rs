//! Quantized level, clip, CPU, LED, and pill state.

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::MeterSnapshot;

/// Shared 30 Hz meter store with 0.5 dB quantization and clip latching.
#[derive(Debug)]
pub struct MeterStore {
    levels: BTreeMap<String, f32>,
    clips: BTreeSet<String>,
    cpu_seeded: bool,
    cpu_ema: f32,
    cpu_percent: f32,
    last_cpu_publish_ms: Option<u64>,
}

impl Default for MeterStore {
    fn default() -> Self {
        Self {
            levels: BTreeMap::new(),
            clips: BTreeSet::new(),
            cpu_seeded: false,
            cpu_ema: 0.0,
            cpu_percent: 0.0,
            last_cpu_publish_ms: None,
        }
    }
}

impl MeterStore {
    /// Applies a complete backend snapshot.
    pub fn apply(&mut self, snapshot: &MeterSnapshot, now_ms: u64) {
        self.update_main("input", snapshot.input_db[0].max(snapshot.input_db[1]));
        self.update_main("output", snapshot.output_db[0].max(snapshot.output_db[1]));
        for block in &snapshot.blocks {
            self.levels.insert(
                format!("block:{}:in", block.block_id.0),
                block.input_db.max(-60.0),
            );
            self.levels.insert(
                format!("block:{}:out", block.block_id.0),
                block.output_db.max(-60.0),
            );
        }
        self.apply_cpu(snapshot.cpu, now_ms);
    }

    /// Updates the combined and per-channel views of one main meter.
    pub fn update_main(&mut self, id: &str, raw: f32) {
        let raw = if raw.is_finite() { raw } else { -60.0 };
        let level = (raw.max(-60.0) * 2.0).round() / 2.0;
        for id in [id.to_owned(), format!("{id}:l"), format!("{id}:r")] {
            self.levels.insert(id.clone(), level);
            if raw >= 0.0 {
                self.clips.insert(id);
            }
        }
    }

    /// Current level or the -60 dB floor.
    #[must_use]
    pub fn level(&self, id: &str) -> f32 {
        self.levels.get(id).copied().unwrap_or(-60.0)
    }

    /// Whether a main or block clip is latched.
    #[must_use]
    pub fn clipped(&self, id: &str) -> bool {
        self.clips.contains(id)
    }

    /// Clears the combined and L/R clip views together.
    pub fn clear_clip(&mut self, id: &str) {
        self.clips.remove(id);
        self.clips.remove(&format!("{id}:l"));
        self.clips.remove(&format!("{id}:r"));
    }

    /// Applies the CPU EMA and 400 ms publication interval.
    pub fn apply_cpu(&mut self, raw: f32, now_ms: u64) {
        let sample = if raw.is_finite() { raw.max(0.0) } else { 0.0 };
        if self.cpu_seeded {
            self.cpu_ema += (sample - self.cpu_ema) * 0.15;
        } else {
            self.cpu_seeded = true;
            self.cpu_ema = sample;
        }
        if self
            .last_cpu_publish_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < 400)
        {
            return;
        }
        self.last_cpu_publish_ms = Some(now_ms);
        self.cpu_percent = (self.cpu_ema * 1000.0).round() / 10.0;
    }

    /// Published CPU percentage.
    #[must_use]
    pub fn cpu_percent(&self) -> f32 {
        self.cpu_percent
    }
}

/// Repaint-dirty meter widget.
#[derive(Debug, Default)]
pub struct Meter {
    level: f32,
    clipped: bool,
    dirty: bool,
}

impl Meter {
    /// Updates level and clip state and records whether a repaint is needed.
    pub fn set(&mut self, level: f32, clipped: bool) {
        let level = level.max(-60.0);
        if self.level != level || self.clipped != clipped {
            self.level = level;
            self.clipped = clipped;
            self.dirty = true;
        }
    }

    /// Current level.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Current clip state.
    #[must_use]
    pub fn clipped(&self) -> bool {
        self.clipped
    }

    /// Takes the repaint-dirty flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

/// Boolean LED state with change tracking.
#[derive(Debug, Default)]
pub struct Led {
    active: bool,
    dirty: bool,
}

impl Led {
    /// Sets state and records changes only.
    pub fn set(&mut self, active: bool) {
        if self.active != active {
            self.active = active;
            self.dirty = true;
        }
    }

    /// Whether lit.
    #[must_use]
    pub fn active(&self) -> bool {
        self.active
    }

    /// Takes the repaint-dirty flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

/// Text pill with active state.
#[derive(Debug)]
pub struct Pill {
    label: String,
    active: bool,
}

impl Pill {
    /// Creates an inactive pill.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            active: false,
        }
    }

    /// Label text.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether active.
    #[must_use]
    pub fn active(&self) -> bool {
        self.active
    }

    /// Toggles active state.
    pub fn toggle(&mut self) {
        self.active = !self.active;
    }
}
