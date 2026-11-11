//! Tone, chain, and session compatibility tests.

use std::time::Duration;

use ronie_core::{
    BlockId, ChainBlock, ChainBranch, ChainSide, ChainSnapshot, InputMode, JuceTree, JuceValue,
    MidiSource, ModelId, ModelSource, SessionSnapshot, Tone, ToneError, references_model,
};
use serde_json::{Value, json};

/// A file URL of an absolute path on this system. A Windows path needs a drive.
fn file_url(path: &str) -> String {
    if cfg!(windows) {
        format!("file:///C:{path}")
    } else {
        format!("file://{path}")
    }
}

fn catalog_tone() -> Tone {
    Tone::from_value(json!({
        "id": 101,
        "title": "Fixture amplifier",
        "format": "nam",
        "models": [
            {
                "id": 1101,
                "name": "Clean",
                "model_url": "https://fixtures.local/models/1101.nam",
                "future_model_field": {"keep": true}
            },
            {
                "id": 1102,
                "name": "Drive",
                "model_url": "https://fixtures.local/models/1102.nam"
            }
        ],
        "makes": [{"id": 11, "name": "Fixture Make"}],
        "future_tone_field": {"nested": [1, 2, 3]}
    }))
    .expect("catalog fixture should parse")
}

fn local_tone() -> Tone {
    Tone::from_value(json!({
        "id": 0,
        "local": true,
        "title": "Local pack",
        "format": "nam",
        "models": [
            {"id": 11, "name": "One", "model_url": file_url("/tmp/hash-1.nam")},
            {"id": 12, "name": "Two", "model_url": file_url("/tmp/hash%20two-2.nam")}
        ]
    }))
    .expect("local fixture should parse")
}

#[test]
fn raw_tone_json_round_trip_preserves_every_field() {
    let input = json!({
        "id": 101,
        "title": "Fixture amplifier",
        "format": "nam",
        "models": [{
            "id": 1101,
            "name": "Clean",
            "model_url": "https://fixtures.local/models/1101.nam",
            "size": "standard"
        }],
        "license": "cc0",
        "unknown": {"array": [true, null, {"unicode": "καλώς"}]}
    });

    let tone = Tone::from_value(input.clone()).expect("fixture should parse");
    let encoded = tone.to_json().expect("raw JSON should encode");
    let round_trip: Value = serde_json::from_str(&encoded).expect("encoded JSON should parse");

    assert_eq!(round_trip, input);
    assert_eq!(tone.raw, input);
}

#[test]
fn local_storage_retains_all_models_and_decodes_stash_names() {
    let stored = local_tone()
        .for_storage(ModelId(11))
        .expect("local storage should retain its active model");

    assert_eq!(stored.models.len(), 2);
    assert_eq!(stored.raw["models"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        stored.models[1].source,
        Some(ModelSource::Local {
            stash_name: "hash two-2.nam".into()
        })
    );
}

#[test]
fn catalog_storage_retains_only_the_active_model() {
    let stored = catalog_tone()
        .for_storage(ModelId(1102))
        .expect("active catalog model should exist");

    assert_eq!(stored.models.len(), 1);
    assert_eq!(stored.models[0].id, ModelId(1102));
    assert_eq!(stored.raw["models"].as_array().map(Vec::len), Some(1));
    assert_eq!(stored.raw["models"][0]["id"], 1102);
    assert_eq!(
        stored.raw["future_tone_field"],
        json!({"nested": [1, 2, 3]})
    );
}

#[test]
fn catalog_storage_rejects_an_unknown_active_model() {
    let result = catalog_tone().for_storage(ModelId(9999));

    assert!(matches!(
        result,
        Err(ToneError::MissingActiveModel(ModelId(9999)))
    ));
}

#[test]
fn local_model_ids_match_independent_fnv_vectors() {
    let cases: [(&[u8], i64); 3] = [
        (b"", 1_160_856_134),
        (b"a", 113_493_085),
        (b"hello", 344_390_646),
    ];

    for (bytes, expected) in cases {
        let id = ModelId::from_local_bytes(bytes);
        assert_eq!(id.0, expected);
        assert!(id.0 > 0);
    }
}

#[test]
fn model_references_follow_local_and_catalog_storage_rules() {
    let mut tone = catalog_tone();
    let mut block = ChainBlock::tone("block-1", tone.clone(), ModelId(1101));

    assert!(references_model(&block, ModelId(1101)));
    assert!(!references_model(&block, ModelId(1102)));
    assert!(!references_model(&block, ModelId(9999)));

    tone.local = true;
    block.tone = Some(tone);
    assert!(references_model(&block, ModelId(1102)));
}

#[test]
fn unknown_input_mode_falls_back_to_stereo() {
    assert_eq!(InputMode::from_persisted("future-mode"), InputMode::Stereo);
    assert_eq!(InputMode::from_persisted("dual"), InputMode::DualMono);
    assert_eq!(ChainSide::from_persisted("future-side"), ChainSide::Left);
}

#[test]
fn a_new_insert_slot_never_takes_the_identity_of_a_moved_block() {
    let mut snapshot = ChainSnapshot::empty();
    snapshot.stereo_enabled = true;
    snapshot.normalize();
    let moved = snapshot.left.remove(0);
    snapshot.right.insert(0, moved);
    let last = snapshot.left.remove(snapshot.left.len() - 1);
    snapshot.right.insert(0, last);
    for _ in 0..2 {
        snapshot.stereo_enabled = !snapshot.stereo_enabled;
        snapshot.normalize();
        let mut ids: Vec<_> = snapshot
            .left
            .iter()
            .chain(&snapshot.right)
            .map(|block| block.id.0.clone())
            .collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "identities must be unique: {ids:?}");
    }
}

#[test]
fn chain_snapshot_normalizes_lanes_and_preserves_fixture_state() {
    let snapshot = ChainSnapshot::from_ui_value(json!({
        "revision": 7,
        "stereoEnabled": true,
        "activeSide": "right",
        "stereoInput": true,
        "stereoOutput": false,
        "dualMonoActive": true,
        "chain": [{
            "blockId": "blk-1",
            "kind": "tone",
            "tone": catalog_tone().raw,
            "activeModelId": 1101,
            "params": {
                "enabled": true,
                "normalize": true,
                "inputGain": 0.5,
                "outputGain": 0.5,
                "mix": 1,
                "eq": {
                    "enabled": false,
                    "pre": false,
                    "bands": [
                        {"type": "lowcut", "freqHz": 20, "gainDb": 0, "q": 0.71},
                        {"type": "bell", "freqHz": 150, "gainDb": 0, "q": 1},
                        {"type": "bell", "freqHz": 500, "gainDb": 0, "q": 1},
                        {"type": "bell", "freqHz": 1500, "gainDb": 0, "q": 1},
                        {"type": "bell", "freqHz": 4000, "gainDb": 0, "q": 1},
                        {"type": "highshelf", "freqHz": 8000, "gainDb": 0, "q": 0.71}
                    ]
                }
            }
        }],
        "branch": {"side": "left", "afterBlockId": "blk-1"}
    }))
    .expect("official chain shape should parse");

    assert_eq!(snapshot.revision, 7);
    assert_eq!(snapshot.left.len(), 3);
    assert_eq!(snapshot.right.len(), 3);
    assert!(snapshot.left.iter().any(|block| block.is_insert));
    assert!(snapshot.right.iter().any(|block| block.is_insert));
    assert_eq!(snapshot.left[0].id.0, "blk-1");
    assert_eq!(snapshot.left[0].params.eq.bands.len(), 6);
    assert_eq!(snapshot.active_side, ChainSide::Right);
    assert_eq!(snapshot.gesture_settle, Duration::from_millis(400));
    assert_eq!(
        snapshot
            .branch
            .expect("branch should survive")
            .after_block_id
            .0,
        "blk-1"
    );
}

#[test]
fn session_snapshot_uses_reference_defaults_and_retains_session_fields() {
    let session = SessionSnapshot::from_ui_value(json!({
        "inputMode": "dual",
        "activePresetId": "user:lead",
        "activePresetName": "Lead",
        "midiMappings": [
            {"targetId": "inputGain", "source": "cc", "number": 20}
        ]
    }))
    .expect("session values should parse");

    assert_eq!(session.input_mode, InputMode::DualMono);
    assert_eq!(session.editor_scale, 1.0);
    assert_eq!(session.editor_extra_height, 36);
    assert_eq!(session.midi_mappings.len(), 1);
    assert_eq!(session.midi_mappings[0].source, MidiSource::Cc);
    assert_eq!(
        session.active_preset.expect("preset should survive").id,
        "user:lead"
    );
    assert_eq!(session.chain.left.len(), 5);
}

#[test]
fn cloned_snapshots_do_not_share_mutable_processing_state() {
    let mut original = ChainSnapshot::empty();
    original.left[0] = ChainBlock::tone("block-1", catalog_tone(), ModelId(1101));
    let cloned = original.clone();

    original.left[0].params.input_gain = 0.75;
    original.left[0]
        .tone
        .as_mut()
        .expect("tone should exist")
        .raw["future_tone_field"] = json!("changed");

    assert_eq!(cloned.left[0].params.input_gain, 0.5);
    assert_eq!(
        cloned.left[0].tone.as_ref().expect("tone should exist").raw["future_tone_field"],
        json!({"nested": [1, 2, 3]})
    );
}

fn tone_block(id: &str) -> ChainBlock {
    ChainBlock::tone(id, catalog_tone(), ModelId(1101))
}

fn lane_ids(lane: &[ChainBlock]) -> Vec<(&str, bool)> {
    lane.iter()
        .map(|block| (block.id.0.as_str(), block.is_insert))
        .collect()
}

#[test]
fn a_lane_puts_its_tones_first_and_keeps_one_open_slot_after_them() {
    let mut snapshot = ChainSnapshot::empty();
    snapshot.left = vec![
        ChainBlock::insert("left-insert-1"),
        tone_block("a"),
        ChainBlock::insert("left-insert-3"),
        tone_block("b"),
        ChainBlock::insert("left-insert-5"),
        ChainBlock::insert("left-insert-6"),
        ChainBlock::insert("left-insert-7"),
    ];
    snapshot.normalize();
    assert_eq!(
        lane_ids(&snapshot.left),
        vec![
            ("a", false),
            ("b", false),
            ("left-insert-1", true),
            ("left-insert-3", true),
            ("left-insert-5", true),
        ],
        "a mono lane keeps five slots, and an open slot keeps its identity"
    );

    snapshot.left = (0..6)
        .map(|index| tone_block(&format!("t{index}")))
        .collect();
    snapshot.normalize();
    assert_eq!(snapshot.left.len(), 7);
    assert!(snapshot.left[6].is_insert);
    assert!(snapshot.left[..6].iter().all(|block| !block.is_insert));
}

#[test]
fn a_stereo_lane_keeps_three_slots() {
    let mut snapshot = ChainSnapshot::empty();
    snapshot.stereo_enabled = true;
    snapshot.right = vec![tone_block("r")];
    snapshot.normalize();
    assert_eq!(snapshot.left.len(), 3);
    assert_eq!(snapshot.right.len(), 3);
    assert_eq!(snapshot.right[0].id.0, "r");
}

#[test]
fn a_saved_chain_with_gaps_between_tones_loads_without_the_gaps() {
    let tone = |id: &str| {
        json!({
            "blockId": id,
            "kind": "tone",
            "tone": catalog_tone().raw,
            "activeModelId": 1101
        })
    };
    let insert = |id: &str| json!({"blockId": id, "kind": "insert"});
    let snapshot = ChainSnapshot::from_ui_value(json!({
        "chain": [insert("i1"), tone("a"), insert("i2"), insert("i3"), tone("b"), insert("i4")],
    }))
    .expect("an old chain parses");
    assert_eq!(
        lane_ids(&snapshot.left),
        vec![
            ("a", false),
            ("b", false),
            ("i1", true),
            ("i2", true),
            ("i3", true)
        ]
    );

    let juce_block = |id: &str, kind: &str| JuceTree {
        identifier: "ChainBlock".to_owned(),
        properties: vec![
            ("id".into(), JuceValue::String(id.into())),
            ("type".into(), JuceValue::String(kind.into())),
            (
                "toneJson".into(),
                JuceValue::String(catalog_tone().to_json().expect("tone JSON")),
            ),
            ("activeModelId".into(), JuceValue::Int64(1101)),
        ],
        children: Vec::new(),
    };
    let tree = JuceTree {
        identifier: "ChainSnapshot".to_owned(),
        properties: Vec::new(),
        children: vec![JuceTree {
            identifier: "ChainBlocks".to_owned(),
            properties: Vec::new(),
            children: vec![
                juce_block("i1", "INSERT"),
                juce_block("a", "NAM"),
                juce_block("i2", "INSERT"),
                juce_block("b", "NAM"),
            ],
        }],
    };
    let snapshot = ChainSnapshot::from_juce_tree(&tree).expect("an old preset chain parses");
    assert_eq!(snapshot.left[0].id.0, "a");
    assert_eq!(snapshot.left[1].id.0, "b");
    assert!(snapshot.left[2..].iter().all(|block| block.is_insert));
}

#[test]
fn a_removed_tone_takes_its_slot_and_a_branch_after_it_moves_back() {
    let mut snapshot = ChainSnapshot::empty();
    snapshot.stereo_enabled = true;
    snapshot.left = vec![tone_block("a"), tone_block("b"), tone_block("c")];
    snapshot.branch = Some(ChainBranch {
        side: ChainSide::Left,
        after_block_id: BlockId("b".to_owned()),
    });
    snapshot.normalize();

    assert!(snapshot.remove_tone(&BlockId("b".to_owned())));
    assert_eq!(lane_ids(&snapshot.left)[..2], [("a", false), ("c", false)]);
    assert_eq!(snapshot.left.len(), 3);
    assert_eq!(
        snapshot.branch.as_ref().expect("branch").after_block_id.0,
        "a"
    );

    assert!(snapshot.remove_tone(&BlockId("a".to_owned())));
    assert_eq!(snapshot.branch, None, "no tone is left before the split");

    let open = snapshot.left[1].id.clone();
    let before = snapshot.clone();
    assert!(!snapshot.remove_tone(&open), "an open slot is not a tone");
    assert_eq!(snapshot, before);
}
