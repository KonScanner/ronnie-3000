//! Converts private official UI scenarios into neutral public fixtures.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use ronie_ui::{backend::UiDrive, testbed::official_drives};
use serde_json::{Value, json};

fn neutral_text(text: &str) -> String {
    let product_lower = ["to", "ne3000"].join("");
    let product_title = ["To", "ne3000"].join("");
    let product_upper = ["TO", "NE3000"].join("");
    text.replace(&product_upper, "Reference")
        .replace(&product_title, "Reference")
        .replace(&format!("{product_lower}.com"), "fixtures.invalid")
        .replace("t3k-fixtures.local", "fixtures.invalid")
        .replace(&product_lower, "reference")
        .replace("t3k", "reference")
}

fn neutral(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (neutral_text(key), neutral(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(neutral).collect()),
        Value::String(text) => Value::String(neutral_text(text)),
        other => other.clone(),
    }
}

fn drive_value(drive: &UiDrive) -> Value {
    match drive {
        UiDrive::Wait { ms } => json!({"kind": "wait", "ms": ms}),
        UiDrive::Click { target } => json!({"kind": "click", "target": target}),
        UiDrive::Fill { target, text } => json!({"kind": "fill", "target": target, "text": text}),
        UiDrive::Submit { target, text } => {
            json!({"kind": "submit", "target": target, "text": text})
        }
        UiDrive::Scroll { target, position } => {
            json!({"kind": "scroll", "target": target, "position": position})
        }
        UiDrive::OpenSettings => json!({"kind": "openSettings"}),
        UiDrive::OpenSystemSettings => json!({"kind": "openSystemSettings"}),
        UiDrive::ScrollSettings { heading } => json!({"kind": "scrollSettings", "target": heading}),
        UiDrive::Unhover => json!({"kind": "unhover"}),
        UiDrive::Hover { target } => json!({"kind": "hover", "target": target}),
        UiDrive::ClearFilter { target } => json!({"kind": "clearFilter", "target": target}),
    }
}

/// The ID of one scenario or manifest row.
fn row_id(row: &Value) -> Option<&str> {
    row["id"].as_str()
}

/// The official scenarios, and after them each local scenario of `existing`.
///
/// A local scenario has an ID that no official scenario has, such as a scenario of a Ronnie
/// feature. It keeps its order, and it goes after the last scenario of its section. A local
/// scenario of a section that no other scenario has goes at the end. An official scenario
/// replaces an existing scenario with its ID.
fn merge_scenarios(official: Vec<Value>, existing: &Value) -> Vec<Value> {
    let ids = official
        .iter()
        .filter_map(row_id)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let mut merged = official;
    for local in existing["scenarios"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row_id(row).is_none_or(|id| !ids.contains(id)))
    {
        let after = merged
            .iter()
            .rposition(|row| row["section"] == local["section"])
            .map_or(merged.len(), |index| index + 1);
        merged.insert(after, local.clone());
    }
    merged
}

/// The official manifest rows and each local row of `existing`, in the order of `scenarios`.
///
/// The manifest keeps each other top-level field of `existing`, such as the capture count of
/// a suite. A row with an ID that no scenario has goes at the end.
fn merge_manifest(official: Vec<Value>, existing: &Value, scenarios: &[Value]) -> Value {
    let ids = official
        .iter()
        .filter_map(row_id)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let mut rows = official;
    rows.extend(
        existing["scenarios"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row_id(row).is_none_or(|id| !ids.contains(id)))
            .cloned(),
    );
    let order = |row: &Value| {
        row_id(row)
            .and_then(|id| {
                scenarios
                    .iter()
                    .position(|scenario| row_id(scenario) == Some(id))
            })
            .unwrap_or(scenarios.len())
    };
    // The sort is stable, so rows with no scenario keep their order at the end.
    rows.sort_by_key(order);
    let mut manifest = existing.as_object().cloned().unwrap_or_default();
    manifest.insert("schema_version".to_owned(), json!(1));
    manifest.insert("scenarios".to_owned(), Value::Array(rows));
    Value::Object(manifest)
}

/// Reads one fixture file, or an empty object when the file does not exist.
fn read_fixture(path: &Path) -> eyre::Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error.into()),
    }
}

fn main() -> eyre::Result<()> {
    let input = std::env::var("RONIE_REFERENCE_UI_SCENARIOS").map_err(|_| {
        eyre::eyre!("set RONIE_REFERENCE_UI_SCENARIOS to the official scenarios JSON")
    })?;
    let source = fs::read_to_string(input)?;
    let root: Value = serde_json::from_str(&source)?;
    let entries = root["scenarios"]
        .as_array()
        .ok_or_else(|| eyre::eyre!("official scenarios must be an array"))?;
    let mut scenarios = Vec::with_capacity(entries.len());
    let mut visual = Vec::with_capacity(entries.len());
    let mut counts = BTreeMap::new();
    for entry in entries {
        let id = entry["id"]
            .as_str()
            .ok_or_else(|| eyre::eyre!("scenario has no ID"))?;
        let section = entry["section"]
            .as_str()
            .ok_or_else(|| eyre::eyre!("scenario has no section"))?;
        *counts.entry(section.to_owned()).or_insert(0_usize) += 1;
        let drives = official_drives(id);
        scenarios.push(json!({
            "id": id,
            "section": section,
            "state": neutral(entry),
            "drives": drives.iter().map(drive_value).collect::<Vec<_>>(),
            "viewport": {"width": 1024, "height": 578, "scale": 2.0},
        }));
        visual.push(json!({
            "id": id,
            "global_mismatch_max": 0.019,
            "worst_tile_64_max": 0.24,
            "backend_effect_required": true,
            "visual_sha256": null,
            "status": "unverified",
        }));
    }
    if scenarios.len() != 103 {
        return Err(eyre::eyre!(
            "expected 103 scenarios, got {}",
            scenarios.len()
        ));
    }
    let expected = [
        ("main", 16),
        ("loading", 14),
        ("banners", 10),
        ("browser", 37),
        ("settings", 11),
        ("chrome", 15),
    ];
    for (section, count) in expected {
        if counts.get(section) != Some(&count) {
            return Err(eyre::eyre!("section {section} has the wrong count"));
        }
    }
    let scenarios_path = Path::new("fixtures/ui-scenarios.json");
    let manifest_path = Path::new("fixtures/ui-visual-manifest.json");
    // The fixtures can also hold local scenarios, and an export must keep them.
    let scenarios = merge_scenarios(scenarios, &read_fixture(scenarios_path)?);
    let local = scenarios.len() - 103;
    let manifest = merge_manifest(visual, &read_fixture(manifest_path)?, &scenarios);
    let public = serde_json::to_string_pretty(&json!({
        "schema_version": 1,
        "scenarios": scenarios,
    }))?;
    let manifest = serde_json::to_string_pretty(&manifest)?;
    let restricted = ["to", "ne3000"].join("");
    for content in [&public, &manifest] {
        if content.to_lowercase().contains(&restricted) || content.contains("t3k-fixtures.local") {
            return Err(eyre::eyre!("neutralization left restricted content"));
        }
    }
    fs::write(scenarios_path, public)?;
    fs::write(manifest_path, manifest)?;
    println!(
        "Exported 103 neutral scenarios and 103 unresolved visual rows, and kept {local} local scenarios"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(id: &str, section: &str) -> Value {
        json!({"id": id, "section": section, "state": {}})
    }

    fn row(id: &str) -> Value {
        json!({"id": id, "status": "unverified"})
    }

    fn ids(rows: &[Value]) -> Vec<&str> {
        rows.iter().filter_map(row_id).collect()
    }

    #[test]
    fn local_scenarios_stay_after_the_official_scenarios_of_their_section() {
        let official = vec![
            scenario("main-a", "main"),
            scenario("main-b", "main"),
            scenario("chrome-a", "chrome"),
        ];
        let existing = json!({"scenarios": [
            scenario("main-a", "main"),
            json!({"id": "main-b", "section": "main", "state": {"old": true}}),
            scenario("main-local", "main"),
            scenario("chrome-a", "chrome"),
            scenario("chrome-local-1", "chrome"),
            scenario("chrome-local-2", "chrome"),
            scenario("extra-local", "extra"),
        ]});

        let merged = merge_scenarios(official, &existing);

        assert_eq!(
            ids(&merged),
            [
                "main-a",
                "main-b",
                "main-local",
                "chrome-a",
                "chrome-local-1",
                "chrome-local-2",
                "extra-local",
            ]
        );
        assert_eq!(merged[1]["state"], json!({}), "the official scenario wins");
    }

    #[test]
    fn no_existing_fixture_gives_the_official_scenarios() {
        let merged = merge_scenarios(vec![scenario("main-a", "main")], &json!({}));
        assert_eq!(ids(&merged), ["main-a"]);
    }

    #[test]
    fn the_manifest_keeps_local_rows_and_its_other_fields() {
        let scenarios = [
            scenario("main-a", "main"),
            scenario("main-local", "main"),
            scenario("chrome-a", "chrome"),
        ];
        let existing = json!({
            "suite": "reference-default",
            "required_capture_count": 103,
            "schema_version": 1,
            "scenarios": [
                row("main-a"),
                row("chrome-a"),
                json!({"id": "main-local", "status": "local"}),
                row("gone-local"),
            ],
        });

        let manifest = merge_manifest(vec![row("main-a"), row("chrome-a")], &existing, &scenarios);

        assert_eq!(manifest["suite"], "reference-default");
        assert_eq!(manifest["required_capture_count"], 103);
        assert_eq!(manifest["schema_version"], 1);
        let rows = manifest["scenarios"].as_array().expect("rows");
        assert_eq!(
            ids(rows),
            ["main-a", "main-local", "chrome-a", "gone-local"]
        );
        assert_eq!(rows[1]["status"], "local", "a local row stays as it is");
    }
}
