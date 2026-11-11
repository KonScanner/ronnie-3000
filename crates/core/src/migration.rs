use crate::{JuceTree, JuceValue};

/// Migrates legacy parameter IDs on an owned tree and removes stale duplicates.
/// Returns the number of renamed children.
#[must_use]
pub fn migrate_param_ids(tree: &mut JuceTree, property: &str) -> usize {
    let mut renamed = 0_usize;
    let mut index = 0_usize;
    while index < tree.children.len() {
        let legacy = match tree.children[index]
            .properties
            .iter()
            .find(|(name, _)| name == property)
            .map(|(_, value)| value)
        {
            Some(JuceValue::String(value)) => value.clone(),
            Some(JuceValue::Int(value)) => value.to_string(),
            Some(JuceValue::Int64(value)) => value.to_string(),
            _ => String::new(),
        };
        let current = current_param_id(&legacy).to_owned();
        if current == legacy {
            index += 1;
            continue;
        }
        if let Some(stale) = tree.children.iter().position(|child| {
            child.properties.iter().any(|(name, value)| {
                name == property && matches!(value, JuceValue::String(text) if text == &current)
            })
        }) {
            tree.children.remove(stale);
            if stale < index {
                index -= 1;
            }
        }
        if let Some(value) = tree.children[index]
            .properties
            .iter_mut()
            .find(|(name, _)| name == property)
            .map(|(_, value)| value)
        {
            *value = JuceValue::String(current);
        }
        renamed += 1;
        index += 1;
    }
    renamed
}

/// Returns the current parameter ID for a legacy spelling.
#[must_use]
pub fn current_param_id(id: &str) -> &str {
    match id {
        "transposeEnabled" => "pitchEnabled",
        "transposeSemitones" => "pitchSemitones",
        "transposeStep" => "pitchStep",
        "transposeTonality" => "pitchTonality",
        "transposeWindow" => "pitchWindow",
        other => other,
    }
}
