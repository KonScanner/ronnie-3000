//! The tones that the player loaded lately, newest first. The Stage add menu shows them as
//! the library.

use ronnie_core::{
    ModelSource, Tone, ToneModel,
    gear::{GearCategory, identify_tone},
};

/// The tones that were loaded lately, newest first.
#[derive(Debug, Clone, Default)]
pub struct RecentTones {
    tones: Vec<Tone>,
}

impl RecentTones {
    /// The machine setting that holds the list.
    pub const KEY: &'static str = "stage.recent_tones";
    /// The most tones that the list keeps.
    pub const MAX: usize = 24;

    /// Reads a list that [`Self::to_text`] wrote. Text that is not such a list gives an empty one.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(text) else {
            return Self::default();
        };
        let tones = values
            .into_iter()
            .filter_map(|value| Tone::from_value(value).ok())
            .take(Self::MAX)
            .collect();
        Self { tones }
    }

    /// The list as a JSON array of the raw tone objects.
    #[must_use]
    pub fn to_text(&self) -> String {
        let raws: Vec<&serde_json::Value> = self.tones.iter().map(|tone| &tone.raw).collect();
        serde_json::to_string(&raws).unwrap_or_else(|_| "[]".to_owned())
    }

    /// Puts a tone first. The same catalog tone, or the same local file, moves up instead of
    /// appearing twice.
    pub fn remember(&mut self, tone: Tone) {
        self.tones.retain(|kept| !same_tone(kept, &tone));
        self.tones.insert(0, tone);
        self.tones.truncate(Self::MAX);
    }

    /// All tones, newest first.
    #[must_use]
    pub fn tones(&self) -> &[Tone] {
        &self.tones
    }

    /// The tones of one gear category, newest first.
    #[must_use]
    pub fn in_category(&self, category: GearCategory) -> Vec<&Tone> {
        self.tones
            .iter()
            .filter(|tone| category_of(tone) == category)
            .collect()
    }

    /// How many tones one gear category holds.
    #[must_use]
    pub fn count(&self, category: GearCategory) -> usize {
        self.in_category(category).len()
    }
}

fn category_of(tone: &Tone) -> GearCategory {
    let (_, identity) = identify_tone(tone, tone.models.first());
    identity.family.category()
}

fn stash_name(model: Option<&ToneModel>) -> Option<&str> {
    match model?.source.as_ref()? {
        ModelSource::Local { stash_name } => Some(stash_name),
        ModelSource::Catalog { .. } => None,
    }
}

/// Whether two tones are the same catalog tone, or the same local file.
#[must_use]
pub fn same_tone(a: &Tone, b: &Tone) -> bool {
    match (stash_name(a.models.first()), stash_name(b.models.first())) {
        (Some(x), Some(y)) => a.title == b.title && x == y,
        (None, None) => a.id == b.id,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog(id: i64, gear: &str) -> Tone {
        Tone::from_value(json!({
            "id": id, "title": format!("Tone {id}"), "format": "nam", "gear": gear,
            "models": [{"id": id * 10, "name": "m",
                "model_url": format!("https://fixtures.invalid/m/{id}.nam")}]
        }))
        .expect("tone")
    }

    fn local(title: &str, file: &str) -> Tone {
        // A file URL names an absolute path of this system, so a Windows path needs a drive.
        let path = format!("/stash/{file}");
        let url = if cfg!(windows) {
            format!("file:///C:{path}")
        } else {
            format!("file://{path}")
        };
        Tone::from_value(json!({
            "id": 0, "title": title, "format": "nam", "gear": "amp",
            "models": [{"id": 1, "name": "m", "model_url": url}]
        }))
        .expect("tone")
    }

    #[test]
    fn remember_puts_the_newest_first() {
        let mut recent = RecentTones::default();
        recent.remember(catalog(1, "amp"));
        recent.remember(catalog(2, "amp"));
        let ids: Vec<i64> = recent.tones().iter().map(|tone| tone.id.0).collect();
        assert_eq!(ids, [2, 1]);
    }

    #[test]
    fn a_catalog_tone_is_kept_once_by_id() {
        let mut recent = RecentTones::default();
        recent.remember(catalog(1, "amp"));
        recent.remember(catalog(2, "amp"));
        recent.remember(catalog(1, "amp"));
        let ids: Vec<i64> = recent.tones().iter().map(|tone| tone.id.0).collect();
        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn a_local_tone_is_kept_once_by_file() {
        let mut recent = RecentTones::default();
        recent.remember(local("Mine", "a.nam"));
        recent.remember(local("Mine", "b.nam"));
        recent.remember(local("Mine", "a.nam"));
        assert_eq!(recent.tones().len(), 2);
        assert_eq!(stash_name(recent.tones()[0].models.first()), Some("a.nam"));
    }

    #[test]
    fn the_list_keeps_twenty_four() {
        let mut recent = RecentTones::default();
        for id in 1..=30 {
            recent.remember(catalog(id, "amp"));
        }
        assert_eq!(recent.tones().len(), RecentTones::MAX);
        assert_eq!(recent.tones()[0].id.0, 30);
    }

    #[test]
    fn bad_text_gives_an_empty_library() {
        assert!(RecentTones::from_text("not json").tones().is_empty());
        assert!(RecentTones::from_text("{}").tones().is_empty());
    }

    #[test]
    fn text_with_thirty_tones_keeps_twenty_four() {
        let raws: Vec<serde_json::Value> = (1..=30).map(|id| catalog(id, "amp").raw).collect();
        let text = serde_json::to_string(&raws).expect("text");
        let back = RecentTones::from_text(&text);
        let ids: Vec<i64> = back.tones().iter().map(|tone| tone.id.0).collect();
        assert_eq!(ids, (1..=24).collect::<Vec<_>>());
    }

    #[test]
    fn text_keeps_the_valid_tones_among_invalid_ones() {
        let text = serde_json::to_string(&json!([
            catalog(1, "amp").raw,
            "not a tone",
            {"id": 9, "title": "Broken", "models": [{"id": 90, "model_url": "not a url"}]},
            catalog(2, "pedal").raw,
            42,
        ]))
        .expect("text");
        let back = RecentTones::from_text(&text);
        let ids: Vec<i64> = back.tones().iter().map(|tone| tone.id.0).collect();
        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn text_round_trips() {
        let mut recent = RecentTones::default();
        recent.remember(catalog(1, "amp"));
        recent.remember(catalog(2, "pedal"));
        let back = RecentTones::from_text(&recent.to_text());
        let pairs: Vec<(i64, &str)> = back
            .tones()
            .iter()
            .map(|tone| (tone.id.0, tone.title.as_str()))
            .collect();
        assert_eq!(pairs, [(2, "Tone 2"), (1, "Tone 1")]);
    }

    #[test]
    fn tones_sort_into_gear_categories() {
        let mut recent = RecentTones::default();
        recent.remember(catalog(1, "amp"));
        recent.remember(catalog(2, "pedal"));
        recent.remember(catalog(3, "pedal"));
        assert_eq!(recent.count(GearCategory::Amp), 1);
        assert_eq!(recent.count(GearCategory::Pedal), 2);
        assert_eq!(recent.in_category(GearCategory::Pedal)[0].id.0, 3);
    }
}
