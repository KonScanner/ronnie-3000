//! Local standalone user-scenario acceptance tests.

use std::path::Path;

use ronie_app::standalone_backend::{StandaloneBackend, standalone_processor};
use ronie_core::{
    JuceCodec, JuceValue, ModelByteCache, PREFERENCES_SETTINGS, PresetStore, RestoreCaches,
    StateCodec,
    params::{ParameterCatalog, ParameterId, ParameterValue},
    restore_offline,
};
use ronie_ui::backend::{ChainFaultKind, GesturePhase, UiAction, UiBackend, UiError};

fn try_backend(root: &Path) -> Result<StandaloneBackend, UiError> {
    StandaloneBackend::dummy(
        standalone_processor(root)?,
        &root.join(PREFERENCES_SETTINGS),
    )
}

fn backend(root: &Path) -> StandaloneBackend {
    try_backend(root).expect("standalone processor backend")
}

fn wav() -> Vec<u8> {
    let samples = [0_i16, 1000, -1000, 0];
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36_u32 + samples.len() as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[test]
fn saved_preset_reloads_after_a_processor_restart() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let backend = backend(root);
    backend
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.75),
            GesturePhase::End,
        )
        .expect("edit input level");
    backend
        .dispatch(UiAction::SavePreset {
            name: "Restart Lead".to_owned(),
        })
        .expect("save preset");

    let store = PresetStore::new(root.join("presets"), root.join("system"));
    let presets = store.list().expect("list saved presets");
    assert_eq!(
        presets.len(),
        1,
        "the saved preset must reach the disk store"
    );
    drop(backend);

    let restarted = crate::backend(root);
    restarted
        .dispatch(UiAction::LoadPreset {
            id: presets[0].id.clone(),
        })
        .expect("reload preset after restart");
    assert_eq!(
        restarted.parameter(ParameterId("inputLevel")),
        Some(ParameterValue(0.75))
    );
    assert_eq!(
        restarted
            .chain_state(0)
            .session
            .active_preset
            .clone()
            .map(|preset| preset.name),
        Some("Restart Lead".to_owned())
    );
}

#[test]
fn local_rig_restores_offline_and_preserves_future_fields_on_update() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let inputs = root.join("inputs");
    std::fs::create_dir(&inputs).expect("input directory");
    let nam = ronie_oracle::synthetic::synthetic_a2_json(8).expect("synthetic NAM");
    let wav = wav();
    std::fs::write(inputs.join("amp.nam"), &nam).expect("NAM fixture");
    std::fs::write(inputs.join("cab.wav"), &wav).expect("WAV fixture");

    let backend = backend(root);
    let inserts = backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .filter(|block| block.is_insert)
        .take(2)
        .map(|block| block.id.0.clone())
        .collect::<Vec<_>>();
    for (insert, path) in inserts
        .iter()
        .zip([inputs.join("amp.nam"), inputs.join("cab.wav")])
    {
        backend
            .dispatch(UiAction::DropFile {
                target_insert_id: ronie_core::BlockId(insert.clone()),
                path,
            })
            .expect("load local model");
    }
    let loaded = backend.chain_state(0).chain;
    let amp = loaded
        .left
        .iter()
        .find(|block| block.id.0 == inserts[0])
        .expect("amplifier block");
    let mut params = amp.params.clone();
    params.eq.enabled = true;
    params.eq.bands = vec![ronie_core::EqBand {
        kind: ronie_core::EqBandType::Bell,
        freq_hz: 1250.0,
        gain_db: 3.5,
        q: 0.8,
    }];
    backend
        .dispatch(UiAction::SetEq {
            block_id: amp.id.clone(),
            params: params.eq,
            gesture: GesturePhase::End,
        })
        .expect("edit EQ");
    backend
        .set_parameter(
            ParameterId("toneTreble"),
            ParameterValue(6.0),
            GesturePhase::End,
        )
        .expect("edit faceplate");
    backend
        .dispatch(UiAction::SavePreset {
            name: "Local Rig".to_owned(),
        })
        .expect("save local rig");

    let store = PresetStore::new(root.join("presets"), root.join("system"));
    let id = store.list().expect("preset list")[0].id.clone();
    let mut document = store.load(&id).expect("saved local rig");
    let mut state_bytes = b"T3KB".to_vec();
    state_bytes.extend_from_slice(
        &JuceCodec::default()
            .write_tree(&document)
            .expect("preset state bytes"),
    );
    let state = StateCodec::new()
        .decode_session(&state_bytes)
        .expect("offline preset document");
    let cache = ModelByteCache::new(16 * 1024 * 1024);
    let plan = restore_offline(
        &state,
        RestoreCaches {
            bytes: &cache,
            current: None,
        },
    )
    .expect("offline restore plan");
    assert_eq!(plan.jobs.len(), 2);
    assert!(plan.jobs.iter().all(|job| job.bytes.is_some()));

    document.properties.push((
        "futureRootField".into(),
        JuceValue::String("retained".into()),
    ));
    let snapshot = document
        .children
        .iter_mut()
        .find(|child| child.identifier == "ChainSnapshot")
        .expect("chain snapshot");
    snapshot
        .properties
        .push(("futureSnapshotField".into(), JuceValue::Int(9)));
    let block = snapshot
        .children
        .iter_mut()
        .find(|child| child.identifier == "ChainBlocks")
        .expect("left lane")
        .children
        .first_mut()
        .expect("first block");
    block
        .properties
        .push(("futureBlockField".into(), JuceValue::Int(7)));
    store
        .save("Local Rig", document)
        .expect("inject future fields");

    drop(backend);
    std::fs::remove_dir_all(&inputs).expect("remove original model inputs");
    std::fs::remove_dir_all(root.join("stash")).expect("remove local stash");

    let restarted = crate::backend(root);
    restarted
        .dispatch(UiAction::LoadPreset { id: id.clone() })
        .expect("load local rig after restart");
    let restored = restarted.chain_state(0).chain;
    let restored_amp = restored
        .left
        .iter()
        .find(|block| block.id.0 == inserts[0])
        .expect("restored amplifier");
    assert_eq!(restored_amp.params.eq.bands.len(), 1);
    assert_eq!(restored_amp.params.eq.bands[0].freq_hz, 1250.0);
    assert_eq!(restored_amp.params.eq.bands[0].gain_db, 3.5);
    assert_eq!(
        restarted.parameter(ParameterId("toneTreble")),
        Some(ParameterValue(6.0))
    );
    let rendered = restarted
        .render_dummy(&[0.25; 256])
        .expect("render restored NAM and WAV through the production RtChain");
    assert_eq!(rendered.len(), 512);
    assert!(rendered.iter().all(|sample| sample.is_finite()));
    assert!(rendered.iter().any(|sample| sample.abs() > 1.0e-6));

    restarted
        .dispatch(UiAction::SavePreset {
            name: "Local Rig".to_owned(),
        })
        .expect("update local rig without source or stash files");
    let updated = store.load(&id).expect("updated local rig");
    assert!(
        updated
            .properties
            .iter()
            .any(|(name, _)| name == "futureRootField")
    );
    let snapshot = updated
        .children
        .iter()
        .find(|child| child.identifier == "ChainSnapshot")
        .expect("updated chain snapshot");
    assert!(
        snapshot
            .properties
            .iter()
            .any(|(name, _)| name == "futureSnapshotField")
    );
    let block = snapshot
        .children
        .iter()
        .find(|child| child.identifier == "ChainBlocks")
        .expect("updated left lane")
        .children
        .first()
        .expect("updated first block");
    assert!(
        block
            .properties
            .iter()
            .any(|(name, _)| name == "futureBlockField")
    );
    let mut embedded = Vec::new();
    for lane in snapshot
        .children
        .iter()
        .filter(|lane| matches!(lane.identifier.as_str(), "ChainBlocks" | "RightChainBlocks"))
    {
        for block in &lane.children {
            if let Some(cache) = block
                .children
                .iter()
                .find(|child| child.identifier == "ModelCache")
            {
                embedded.extend(cache.children.iter().filter_map(|cached| {
                    cached.properties.iter().find_map(|(name, value)| {
                        if name == "data"
                            && let JuceValue::Binary(bytes) = value
                        {
                            return Some(bytes.clone());
                        }
                        None
                    })
                }));
            }
        }
    }
    assert_eq!(embedded.len(), 2);
    assert!(embedded.contains(&nam));
    assert!(embedded.contains(&wav));
}

#[test]
fn standalone_session_restores_parameters_midi_and_active_preset_after_close() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let expected_input = {
        let backend = backend(root);
        backend
            .set_parameter(
                ParameterId("inputLevel"),
                ParameterValue(0.8),
                GesturePhase::End,
            )
            .expect("edit session parameter");
        backend
            .dispatch(UiAction::Midi(ronie_ui::backend::MidiAction::SetChannel(7)))
            .expect("set MIDI channel");
        backend
            .dispatch(UiAction::Midi(ronie_ui::backend::MidiAction::SetMapping {
                target_id: "inputLevel".to_owned(),
                source: ronie_core::MidiSource::Cc,
                number: 20,
            }))
            .expect("map MIDI controller");
        backend
            .dispatch(UiAction::SavePreset {
                name: "Session Rig".to_owned(),
            })
            .expect("save active preset");
        backend
            .parameter(ParameterId("inputLevel"))
            .expect("parameter before close")
    };
    assert!(root.join("session.state").is_file());

    let restarted = crate::backend(root);
    assert_eq!(
        restarted.parameter(ParameterId("inputLevel")),
        Some(expected_input)
    );
    let midi = restarted.midi_map();
    assert_eq!(midi.channel, 7);
    assert_eq!(midi.mappings.len(), 1);
    assert_eq!(midi.mappings[0].target_id, "inputLevel");
    assert_eq!(midi.mappings[0].number, 20);
    assert_eq!(
        restarted
            .chain_state(0)
            .session
            .active_preset
            .clone()
            .map(|preset| preset.name),
        Some("Session Rig".to_owned())
    );
}

#[test]
fn legacy_preset_ids_keep_the_latest_edit_across_restarts() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let initial = backend(root);
    initial
        .set_parameter(
            ParameterId("pitchEnabled"),
            ParameterValue(0.0),
            GesturePhase::End,
        )
        .expect("disable pitch before saving legacy preset");
    initial
        .dispatch(UiAction::SavePreset {
            name: "Legacy Pitch".to_owned(),
        })
        .expect("save current preset");

    let store = PresetStore::new(root.join("presets"), root.join("system"));
    let id = store.list().expect("preset list")[0].id.clone();
    let mut legacy = store.load(&id).expect("current preset document");
    legacy.properties.push((
        "futureLegacyRoot".into(),
        JuceValue::String("retained".into()),
    ));
    let params = legacy
        .children
        .iter_mut()
        .find(|child| child.identifier == "Params")
        .expect("preset parameters");
    let pitch = params
        .children
        .iter_mut()
        .find(|param| {
            param.properties.iter().any(|(name, value)| {
                name == "id" && matches!(value, JuceValue::String(id) if id == "pitchEnabled")
            })
        })
        .expect("pitch parameter");
    pitch.properties.retain(|(name, _)| name != "id");
    pitch
        .properties
        .push(("id".into(), JuceValue::String("transposeEnabled".into())));
    pitch.properties.push((
        "futureLegacyParam".into(),
        JuceValue::String("retained".into()),
    ));
    store
        .save("Legacy Pitch", legacy)
        .expect("store legacy parameter spelling");
    drop(initial);

    for expected in [1.0, 0.0, 1.0] {
        let editing = backend(root);
        editing
            .dispatch(UiAction::LoadPreset { id: id.clone() })
            .expect("load legacy preset");
        editing
            .set_parameter(
                ParameterId("pitchEnabled"),
                ParameterValue(expected),
                GesturePhase::End,
            )
            .expect("edit migrated pitch parameter");
        editing
            .dispatch(UiAction::SavePreset {
                name: "Legacy Pitch".to_owned(),
            })
            .expect("save edited legacy preset");
        drop(editing);

        let restarted = backend(root);
        restarted
            .dispatch(UiAction::LoadPreset { id: id.clone() })
            .expect("reload edited legacy preset");
        assert_eq!(
            restarted.parameter(ParameterId("pitchEnabled")),
            Some(ParameterValue(expected))
        );
        drop(restarted);

        let updated = store.load(&id).expect("updated legacy preset");
        assert!(
            updated
                .properties
                .iter()
                .any(|(name, _)| name == "futureLegacyRoot")
        );
        let params = updated
            .children
            .iter()
            .find(|child| child.identifier == "Params")
            .expect("updated parameters");
        let pitch = params
            .children
            .iter()
            .find(|param| {
                param.properties.iter().any(|(name, value)| {
                    name == "id" && matches!(value, JuceValue::String(id) if id == "pitchEnabled")
                })
            })
            .expect("migrated pitch parameter");
        assert!(
            pitch
                .properties
                .iter()
                .any(|(name, _)| name == "futureLegacyParam")
        );
        assert!(!params.children.iter().any(|param| {
            param.properties.iter().any(|(name, value)| {
                name == "id" && matches!(value, JuceValue::String(id) if id == "transposeEnabled")
            })
        }));
    }
}

#[test]
fn preset_updates_strip_known_non_preset_parameters_and_preserve_current_scopes() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let initial = backend(root);
    initial
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.25),
            GesturePhase::End,
        )
        .expect("edit preset parameter");
    initial
        .set_parameter(
            ParameterId("osFactor"),
            ParameterValue(2.0),
            GesturePhase::End,
        )
        .expect("edit machine default");
    initial
        .dispatch(UiAction::PersistMachineDefault(ParameterId("osFactor")))
        .expect("persist machine default");
    initial
        .set_parameter(
            ParameterId("targetLoudness"),
            ParameterValue(-30.0),
            GesturePhase::End,
        )
        .expect("edit session-only parameter");
    initial
        .dispatch(UiAction::SavePreset {
            name: "Scoped Rig".to_owned(),
        })
        .expect("save scoped preset");

    let store = PresetStore::new(root.join("presets"), root.join("system"));
    let id = store.list().expect("preset list")[0].id.clone();
    let mut document = store.load(&id).expect("scoped preset");
    let params = document
        .children
        .iter_mut()
        .find(|child| child.identifier == "Params")
        .expect("preset parameters");
    for (id, value) in [("osFactor", 0.0), ("targetLoudness", -60.0)] {
        params.children.push(ronie_core::JuceTree {
            identifier: "Param".into(),
            properties: vec![
                ("id".into(), JuceValue::String(id.into())),
                ("value".into(), JuceValue::Double(value)),
            ],
            children: Vec::new(),
        });
    }
    params.children.push(ronie_core::JuceTree {
        identifier: "Param".into(),
        properties: vec![
            ("id".into(), JuceValue::String("futureParameter".into())),
            ("value".into(), JuceValue::Double(42.0)),
        ],
        children: Vec::new(),
    });
    params.children.push(ronie_core::JuceTree {
        identifier: "FutureParamsNode".into(),
        properties: vec![("retained".into(), JuceValue::Bool(true))],
        children: Vec::new(),
    });
    store
        .save("Scoped Rig", document)
        .expect("inject scoped and future parameters");
    drop(initial);

    let restarted = backend(root);
    assert_eq!(
        restarted.parameter(ParameterId("osFactor")),
        Some(ParameterValue(2.0))
    );
    assert_eq!(
        restarted.parameter(ParameterId("targetLoudness")),
        Some(ParameterValue(-30.0))
    );
    restarted
        .dispatch(UiAction::LoadPreset { id: id.clone() })
        .expect("load scoped preset");
    assert_eq!(
        restarted.parameter(ParameterId("inputLevel")),
        Some(ParameterValue(0.25))
    );
    assert_eq!(
        restarted.parameter(ParameterId("osFactor")),
        Some(ParameterValue(2.0))
    );
    assert_eq!(
        restarted.parameter(ParameterId("targetLoudness")),
        Some(ParameterValue(-30.0))
    );
    restarted
        .dispatch(UiAction::SavePreset {
            name: "Scoped Rig".to_owned(),
        })
        .expect("update scoped preset");

    let updated = store.load(&id).expect("updated scoped preset");
    let params = updated
        .children
        .iter()
        .find(|child| child.identifier == "Params")
        .expect("updated parameters");
    for id in ["osFactor", "targetLoudness"] {
        assert!(
            !params.children.iter().any(|param| {
                param.properties.iter().any(|(name, value)| {
                    name == "id" && matches!(value, JuceValue::String(actual) if actual == id)
                })
            }),
            "known non-preset parameter {id} must be removed"
        );
    }
    assert!(params.children.iter().any(|param| {
        param.properties.iter().any(|(name, value)| {
            name == "id" && matches!(value, JuceValue::String(id) if id == "futureParameter")
        })
    }));
    assert!(
        params
            .children
            .iter()
            .any(|child| child.identifier == "FutureParamsNode")
    );
}

#[test]
fn clamped_preset_move_is_a_revision_free_no_op() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let backend = backend(root);
    backend
        .dispatch(UiAction::SavePreset {
            name: "Only Preset".to_owned(),
        })
        .expect("save preset");
    let id = PresetStore::new(root.join("presets"), root.join("system"))
        .list()
        .expect("preset list")[0]
        .id
        .clone();
    let before = backend.chain_state(0).revision;
    let receipt = backend
        .dispatch(UiAction::MovePreset { id, delta: -100 })
        .expect("clamped move is an accepted no-op");
    assert_eq!(receipt.revision, before);
    assert_eq!(backend.chain_state(0).revision, before);
}

fn nam_only_rig(root: &Path) -> (PresetStore, String) {
    let inputs = root.join("inputs");
    std::fs::create_dir_all(&inputs).expect("input directory");
    let nam = ronie_oracle::synthetic::synthetic_a2_json(8).expect("synthetic NAM");
    std::fs::write(inputs.join("amp.nam"), nam).expect("NAM fixture");
    let backend = backend(root);
    let insert = backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|block| block.is_insert)
        .expect("insert slot")
        .id
        .0
        .clone();
    backend
        .dispatch(UiAction::DropFile {
            target_insert_id: ronie_core::BlockId(insert),
            path: inputs.join("amp.nam"),
        })
        .expect("load local NAM");
    backend
        .dispatch(UiAction::SavePreset {
            name: "Broken Rig".to_owned(),
        })
        .expect("save NAM preset");
    drop(backend);
    let store = PresetStore::new(root.join("presets"), root.join("system"));
    let id = store.list().expect("preset list")[0].id.clone();
    (store, id)
}

const BAD_MODEL: &[u8] = b"not a NAM model";

fn corrupt_first_cached_model(document: &mut ronie_core::JuceTree) {
    replace_first_cached_model(document, BAD_MODEL);
}

/// Replaces the first embedded model and returns the bytes that it held.
fn replace_first_cached_model(document: &mut ronie_core::JuceTree, bytes: &[u8]) -> Vec<u8> {
    let snapshot = document
        .children
        .iter_mut()
        .find(|child| child.identifier == "ChainSnapshot")
        .expect("chain snapshot");
    for lane in &mut snapshot.children {
        for block in &mut lane.children {
            let Some(cache) = block
                .children
                .iter_mut()
                .find(|child| child.identifier == "ModelCache")
            else {
                continue;
            };
            for cached in &mut cache.children {
                for (name, value) in &mut cached.properties {
                    if name == "data" {
                        let previous = std::mem::replace(value, JuceValue::Binary(bytes.to_vec()));
                        let JuceValue::Binary(previous) = previous else {
                            panic!("embedded model is not binary");
                        };
                        return previous;
                    }
                }
            }
        }
    }
    panic!("fixture has no cached model");
}

fn encode_state(document: &ronie_core::JuceTree) -> Vec<u8> {
    let mut bytes = b"T3KB".to_vec();
    bytes.extend_from_slice(
        &JuceCodec::default()
            .write_tree(document)
            .expect("session bytes"),
    );
    bytes
}

/// A preset load compiles no model, so a model that cannot compile does not refuse the preset.
/// The chain worker compiles the preset, runs the block as bypass, and shows its fault.
#[test]
fn a_preset_with_an_embedded_model_that_cannot_compile_runs_the_block_as_bypass() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let (store, id) = nam_only_rig(root);
    let mut document = store.load(&id).expect("valid NAM preset");
    corrupt_first_cached_model(&mut document);
    store
        .save("Broken Rig", document)
        .expect("store corrupt embedded model");
    // No local copy can repair the block.
    std::fs::remove_dir_all(root.join("stash")).expect("remove local stash");

    let backend = backend(root);
    assert!(backend.chain_faults().is_empty(), "the saved session runs");
    backend
        .dispatch(UiAction::LoadPreset { id: id.clone() })
        .expect("a model that cannot compile does not refuse the preset");

    // A dummy stream waits for the compile of each edit, so the fault shows with no settle.
    let block = tone_block(&backend);
    assert!(block.tone.is_some(), "the session keeps the tone");
    assert_eq!(
        backend
            .chain_state(0)
            .session
            .active_preset
            .clone()
            .map(|preset| preset.id),
        Some(id)
    );
    let status = backend.audio_status().expect("status");
    assert_eq!(status.unavailable_blocks, std::slice::from_ref(&block.id));
    assert_eq!(status.chain_error, None);
    let faults = backend.chain_faults();
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(faults[0].kind, ChainFaultKind::ModelRejected);
    let input = [0.25; 256];
    assert_eq!(
        backend.render_dummy(&input).expect("render").len(),
        input.len() * 2,
        "the stream runs with the block as bypass"
    );
}

/// Removes each embedded model of a document.
fn remove_cached_models(document: &mut ronie_core::JuceTree) {
    if document.identifier == "CachedModel" {
        document.properties.retain(|(name, _)| name != "data");
    }
    for child in &mut document.children {
        remove_cached_models(child);
    }
}

/// The check of a preset load finds a model with no bytes before any session change, and with
/// no compile.
#[test]
fn a_preset_with_a_model_with_no_bytes_is_refused_with_no_change() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let (store, id) = nam_only_rig(root);
    let mut document = store.load(&id).expect("valid NAM preset");
    remove_cached_models(&mut document);
    store
        .save("Broken Rig", document)
        .expect("store a preset with no model bytes");
    // An empty session has no cached bytes for the block of the preset.
    std::fs::remove_file(root.join("session.state")).expect("remove saved session");
    std::fs::remove_dir_all(root.join("stash")).expect("remove local stash");

    let backend = backend(root);
    backend
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.75),
            GesturePhase::End,
        )
        .expect("an edit to undo");
    let before = backend.chain_state(0);
    let audio = backend.audio_status().expect("status");

    backend
        .dispatch(UiAction::LoadPreset { id })
        .expect_err("a preset with no model bytes must not load");
    backend.settle_audio().expect("settle");

    let after = backend.chain_state(0);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.chain, before.chain);
    assert_eq!(after.session, before.session);
    assert_eq!(after.can_undo, before.can_undo);
    assert_eq!(
        backend.parameter(ParameterId("inputLevel")),
        Some(ParameterValue(0.75))
    );
    assert_eq!(backend.audio_status().expect("status"), audio, "no compile");
    backend
        .dispatch(UiAction::Undo)
        .expect("the history is intact");
    assert_ne!(
        backend.parameter(ParameterId("inputLevel")),
        Some(ParameterValue(0.75))
    );
}

/// Saves a session with one local NAM block, corrupts its embedded model, and returns the valid
/// bytes. The local stash still holds a valid copy of the model.
fn session_with_bad_embedded_model(root: &Path) -> Vec<u8> {
    nam_only_rig(root);
    let path = root.join("session.state");
    let bytes = std::fs::read(&path).expect("valid session bytes");
    let mut document = StateCodec::new()
        .decode_session(&bytes)
        .expect("valid session document");
    let valid = replace_first_cached_model(&mut document.raw, BAD_MODEL);
    std::fs::write(&path, encode_state(&document.raw)).expect("inject corrupt embedded model");
    valid
}

/// As [`session_with_bad_embedded_model`], with no local copy that can repair the block.
fn session_with_unusable_model(root: &Path) -> Vec<u8> {
    let valid = session_with_bad_embedded_model(root);
    std::fs::remove_dir_all(root.join("stash")).expect("remove local stash");
    valid
}

fn tone_block(backend: &StandaloneBackend) -> ronie_core::ChainBlock {
    backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|block| !block.is_insert)
        .expect("tone block")
        .clone()
}

fn saved_model_bytes(root: &Path) -> Vec<Vec<u8>> {
    StateCodec::new()
        .decode_session(&std::fs::read(root.join("session.state")).expect("saved session"))
        .expect("saved session document")
        .model_cache
        .iter()
        .map(|entry| entry.bytes.to_vec())
        .collect()
}

#[test]
fn a_saved_model_that_cannot_compile_runs_as_bypass_and_does_not_stop_the_launch() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    session_with_unusable_model(root);

    let backend = try_backend(root).expect("a bad saved model must not stop the launch");

    let block = tone_block(&backend);
    assert!(block.tone.is_some(), "the session keeps the tone");
    assert!(block.active_model_id.is_some());
    let status = backend.audio_status().expect("status");
    assert_eq!(status.unavailable_blocks, [block.id]);
    assert_eq!(status.chain_error, None);
    let input = [0.25; 256];
    assert_eq!(
        backend.render_dummy(&input).expect("render").len(),
        input.len() * 2,
        "the stream runs with the block as bypass"
    );
}

/// Runs what the shell runs at launch, and returns the text of the banner and whether the first
/// window has room for it.
fn launch_banner(backend: std::sync::Arc<StandaloneBackend>) -> (Option<String>, bool) {
    let options = ronie_ui::shell::NativeShellOptions {
        backend: Some(backend.clone()),
        ..Default::default()
    };
    let room = ronie_ui::shell::initial_window_has_banner(&options);
    let mut chrome =
        ronie_ui::default::ChromeController::new(backend.clone(), Vec::new(), false, true);
    let mut device = None;
    let mut faults = Vec::new();
    ronie_ui::shell::sync_device_banner(backend.as_ref(), &mut chrome, &mut device, 1_000);
    ronie_ui::shell::sync_chain_faults(backend.as_ref(), &mut chrome, &mut faults, 1_000);
    let text = chrome
        .banners()
        .active()
        .map(|banner| format!("{}{}", banner.headline, banner.body));
    (text, room)
}

#[test]
fn a_launch_with_a_model_that_cannot_compile_shows_its_banner() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    session_with_unusable_model(root);

    let backend = try_backend(root).expect("a bad saved model must not stop the launch");
    let title = tone_block(&backend).tone.expect("tone").title;
    let (text, room) = launch_banner(std::sync::Arc::new(backend));

    let text = text.expect("a banner");
    assert_eq!(
        text,
        format!("{title} cannot run. Ronnie bypasses this block.")
    );
    let root_text = root.display().to_string();
    assert!(!text.contains(&root_text), "{text}");
    assert!(!text.contains("NAM"), "{text}");
    assert!(room, "the first window must have room for the banner");
}

#[test]
fn a_launch_with_a_healthy_saved_session_shows_no_fault_banner() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    nam_only_rig(root);

    let backend = try_backend(root).expect("launch");
    assert!(backend.chain_faults().is_empty());
    let (text, room) = launch_banner(std::sync::Arc::new(backend));

    assert_eq!(text, None);
    assert!(!room);
}

#[test]
fn a_repair_of_the_saved_bytes_clears_the_banner_at_the_next_launch() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let valid = session_with_unusable_model(root);
    let first = try_backend(root).expect("bypassed launch");
    assert_eq!(first.chain_faults().len(), 1);
    drop(first);

    let path = root.join("session.state");
    let mut saved = StateCodec::new()
        .decode_session(&std::fs::read(&path).expect("saved session"))
        .expect("saved session document");
    replace_first_cached_model(&mut saved.raw, &valid);
    std::fs::write(&path, encode_state(&saved.raw)).expect("repair saved model");

    let repaired = try_backend(root).expect("repaired launch");
    assert!(repaired.chain_faults().is_empty());
}

#[test]
fn a_save_after_a_bypassed_launch_keeps_the_block_and_its_bytes() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    session_with_unusable_model(root);

    let first = try_backend(root).expect("first launch");
    let block = tone_block(&first);
    first
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.75),
            GesturePhase::End,
        )
        .expect("edit session parameter");
    drop(first);

    assert_eq!(saved_model_bytes(root), [BAD_MODEL.to_vec()]);
    let saved = StateCodec::new()
        .decode_session(&std::fs::read(root.join("session.state")).expect("saved session"))
        .expect("saved session document");
    let saved_block = saved
        .snapshot
        .chain
        .left
        .iter()
        .find(|candidate| candidate.id == block.id)
        .expect("saved block");
    assert_eq!(saved_block.tone, block.tone);
    assert_eq!(saved_block.active_model_id, block.active_model_id);

    let second = try_backend(root).expect("second launch");
    assert_eq!(tone_block(&second).tone, block.tone);
    assert_eq!(
        second.audio_status().expect("status").unavailable_blocks,
        [block.id]
    );
    assert_eq!(
        second.parameter(ParameterId("inputLevel")),
        Some(ParameterValue(0.75))
    );
    drop(second);
    assert_eq!(saved_model_bytes(root), [BAD_MODEL.to_vec()]);
}

#[test]
fn a_bypassed_block_restores_after_a_later_fix_of_its_bytes() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let valid = session_with_unusable_model(root);
    drop(try_backend(root).expect("bypassed launch"));

    let path = root.join("session.state");
    let mut saved = StateCodec::new()
        .decode_session(&std::fs::read(&path).expect("saved session"))
        .expect("saved session document");
    replace_first_cached_model(&mut saved.raw, &valid);
    std::fs::write(&path, encode_state(&saved.raw)).expect("repair saved model");

    let repaired = try_backend(root).expect("repaired launch");
    let status = repaired.audio_status().expect("status");
    assert!(status.unavailable_blocks.is_empty());
    assert_eq!(status.chain_error, None);
}

#[test]
fn a_valid_local_copy_repairs_a_bad_embedded_model() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let valid = session_with_bad_embedded_model(root);

    let backend = try_backend(root).expect("launch with a valid local copy");

    assert!(
        backend
            .audio_status()
            .expect("status")
            .unavailable_blocks
            .is_empty()
    );
    drop(backend);
    assert_eq!(saved_model_bytes(root), [valid]);
}

/// Replaces each file of the local stash with these bytes.
fn overwrite_stash(root: &Path, bytes: &[u8]) {
    let mut files = 0;
    for entry in std::fs::read_dir(root.join("stash")).expect("stash") {
        std::fs::write(entry.expect("entry").path(), bytes).expect("overwrite stash file");
        files += 1;
    }
    assert_eq!(files, 1, "the fixture has one local model");
}

#[test]
fn a_corrupt_local_copy_does_not_replace_valid_embedded_bytes() {
    let other_model = ronie_oracle::synthetic::synthetic_a2_json(3).expect("synthetic NAM");
    let cases: [(&str, &[u8]); 3] = [
        ("garbage", BAD_MODEL),
        ("truncated", &other_model[..other_model.len() / 2]),
        ("another valid model", &other_model),
    ];
    for (name, corrupt) in cases {
        let directory = tempfile::tempdir().expect("temporary standalone root");
        let root = directory.path();
        nam_only_rig(root);
        let valid = saved_model_bytes(root);
        assert_eq!(valid.len(), 1);
        overwrite_stash(root, corrupt);

        let backend = try_backend(root).expect("launch");
        let block = tone_block(&backend);
        // This edit prepares a new chain from the live session.
        backend
            .dispatch(UiAction::SetStereoMode(false))
            .expect("structure edit");
        backend.settle_audio().expect("settle");

        let status = backend.audio_status().expect("status");
        assert!(
            status.unavailable_blocks.is_empty(),
            "{name}: {:?}",
            status.unavailable_blocks
        );
        assert_eq!(status.chain_error, None, "{name}");
        assert_eq!(tone_block(&backend).id, block.id, "{name}");
        drop(backend);
        assert_eq!(
            saved_model_bytes(root),
            valid,
            "{name}: the save kept the bytes"
        );
    }
}

#[test]
fn two_bad_copies_keep_the_embedded_bytes_and_run_as_bypass() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    session_with_bad_embedded_model(root);
    overwrite_stash(root, b"also not a model");

    let backend = try_backend(root).expect("launch");

    let block = tone_block(&backend);
    assert_eq!(
        backend.audio_status().expect("status").unavailable_blocks,
        [block.id]
    );
    drop(backend);
    assert_eq!(saved_model_bytes(root), [BAD_MODEL.to_vec()]);
}

#[test]
fn a_session_that_cannot_decode_still_stops_the_launch_and_stays_on_disk() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let path = root.join("session.state");
    let corrupted = b"this is not a session".to_vec();
    std::fs::write(&path, &corrupted).expect("inject corrupt session");

    assert!(try_backend(root).is_err(), "a corrupt session must fail");
    assert_eq!(std::fs::read(path).expect("preserved session"), corrupted);
}

#[test]
fn a_block_with_no_active_model_stops_the_launch_after_a_bad_model() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    session_with_unusable_model(root);
    let path = root.join("session.state");
    let mut saved = StateCodec::new()
        .decode_session(&std::fs::read(&path).expect("saved session"))
        .expect("saved session document");
    let blocks = saved
        .raw
        .children
        .iter_mut()
        .find(|child| child.identifier == "ChainSnapshot")
        .expect("chain snapshot")
        .children
        .iter_mut()
        .find(|lane| lane.identifier == "ChainBlocks")
        .expect("left lane");
    let index = blocks
        .children
        .iter()
        .position(|block| {
            block
                .children
                .iter()
                .any(|child| child.identifier == "ModelCache")
        })
        .expect("tone block");
    let mut other = blocks.children[index].clone();
    for (name, value) in &mut other.properties {
        if name == "id" {
            *value = JuceValue::String("no-active-model".to_owned());
        }
    }
    other.properties.retain(|(name, _)| name != "activeModelId");
    blocks.children.insert(index + 1, other);
    let corrupted = encode_state(&saved.raw);
    std::fs::write(&path, &corrupted).expect("inject second fault");

    let Err(error) = try_backend(root) else {
        panic!("a block with no active model must stop the launch");
    };
    assert!(error.to_string().contains("no active model"), "{error}");
    assert_eq!(std::fs::read(path).expect("preserved session"), corrupted);
}

#[test]
fn standalone_session_restores_every_session_scoped_parameter_exactly() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let initial = backend(root);
    for spec in ParameterCatalog::v1()
        .iter()
        .filter(|spec| spec.scope.session)
    {
        let value = if spec.default == spec.min {
            spec.max
        } else {
            spec.min
        };
        initial
            .set_parameter(spec.id, ParameterValue(value), GesturePhase::End)
            .expect("edit session parameter");
    }
    let expected = ParameterCatalog::v1()
        .iter()
        .filter(|spec| spec.scope.session)
        .map(|spec| {
            (
                spec.id,
                initial.parameter(spec.id).expect("current parameter"),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    drop(initial);

    let restarted = backend(root);
    for (id, value) in expected {
        assert_eq!(restarted.parameter(id), Some(value), "parameter {}", id.0);
    }
}

#[test]
fn unknown_session_fields_survive_close_and_reopen() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let initial = backend(root);
    initial
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.75),
            GesturePhase::End,
        )
        .expect("edit session parameter");
    drop(initial);

    let path = root.join("session.state");
    let bytes = std::fs::read(&path).expect("session bytes");
    let mut document = StateCodec::new()
        .decode_session(&bytes)
        .expect("session document");
    document.raw.properties.push((
        "futureSessionRoot".into(),
        JuceValue::String("retained".into()),
    ));
    let snapshot = document
        .raw
        .children
        .iter_mut()
        .find(|child| child.identifier == "ChainSnapshot")
        .expect("chain snapshot");
    snapshot
        .properties
        .push(("futureSessionSnapshot".into(), JuceValue::Int(7)));
    snapshot
        .children
        .iter_mut()
        .find(|child| child.identifier == "ChainBlocks")
        .expect("left lane")
        .children
        .first_mut()
        .expect("first block")
        .properties
        .push(("futureSessionBlock".into(), JuceValue::Int(9)));
    for (name, field) in [
        ("PARAMETERS", "futureSessionParams"),
        ("MidiMappings", "futureSessionMidi"),
    ] {
        document
            .raw
            .children
            .iter_mut()
            .find(|child| child.identifier == name)
            .expect("session subtree")
            .properties
            .push((field.into(), JuceValue::Bool(true)));
    }
    std::fs::write(&path, encode_state(&document.raw)).expect("inject future session fields");

    let restarted = backend(root);
    restarted
        .set_parameter(
            ParameterId("inputLevel"),
            ParameterValue(0.8),
            GesturePhase::End,
        )
        .expect("edit restored session");
    drop(restarted);

    let bytes = std::fs::read(path).expect("updated session bytes");
    let document = StateCodec::new()
        .decode_session(&bytes)
        .expect("updated session document");
    assert!(
        document
            .raw
            .properties
            .iter()
            .any(|(name, _)| name == "futureSessionRoot")
    );
    let snapshot = document
        .raw
        .children
        .iter()
        .find(|child| child.identifier == "ChainSnapshot")
        .expect("updated chain snapshot");
    assert!(
        snapshot
            .properties
            .iter()
            .any(|(name, _)| name == "futureSessionSnapshot")
    );
    assert!(
        snapshot
            .children
            .iter()
            .find(|child| child.identifier == "ChainBlocks")
            .expect("updated left lane")
            .children
            .first()
            .expect("updated first block")
            .properties
            .iter()
            .any(|(name, _)| name == "futureSessionBlock")
    );
    for (name, field) in [
        ("PARAMETERS", "futureSessionParams"),
        ("MidiMappings", "futureSessionMidi"),
    ] {
        assert!(
            document
                .raw
                .children
                .iter()
                .find(|child| child.identifier == name)
                .expect("updated session subtree")
                .properties
                .iter()
                .any(|(name, _)| name == field)
        );
    }
}

#[test]
fn a_preset_with_downloaded_bytes_reloads_after_a_restart_with_no_network() {
    let directory = tempfile::tempdir().expect("temporary standalone root");
    let root = directory.path();
    let nam = ronie_oracle::synthetic::synthetic_a2_json(8).expect("synthetic NAM");
    let tone = ronie_core::Tone::from_value(serde_json::json!({
        "id": 5,
        "title": "Remote",
        "format": "nam",
        "models": [{"id": 7, "name": "Amp", "model_url": "https://models.example/7.nam"}],
    }))
    .expect("tone");
    let backend = backend(root);
    let target = backend
        .chain_state(0)
        .chain
        .left
        .iter()
        .find(|block| block.is_insert)
        .expect("insert")
        .id
        .clone();
    backend
        .dispatch(UiAction::LoadToneWithBytes {
            target_insert_id: target.clone(),
            tone,
            model_id: ronie_core::ModelId(7),
            bytes: nam.into(),
        })
        .expect("load with bytes");
    backend.settle_audio().expect("settle");
    backend
        .dispatch(UiAction::SavePreset {
            name: "Remote Rig".to_owned(),
        })
        .expect("save preset");
    let store = PresetStore::new(root.join("presets"), root.join("system"));
    let id = store.list().expect("presets")[0].id.clone();
    backend
        .dispatch(UiAction::RemoveBlock(target))
        .expect("remove");
    drop(backend);

    let restarted = crate::backend(root);
    restarted
        .dispatch(UiAction::LoadPreset { id })
        .expect("load preset with no network");
    let rendered = restarted.render_dummy(&[0.25; 256]).expect("render");
    assert!(rendered.iter().all(|sample| sample.is_finite()));
    assert!(rendered.iter().any(|sample| sample.abs() > 1.0e-6));
    let status = restarted.audio_status().expect("status");
    assert!(status.unavailable_blocks.is_empty());
    assert_eq!(status.chain_error, None);
}
