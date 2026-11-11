//! Conversion between control-side processor frames and compatible preset documents.

use ronie_core::{
    BlockId, ByteCacheKey, ChainBlock, ChainBranch, ChainSnapshot, EqBandType, JuceTree, JuceValue,
    LocalStash, ModelByteCache, ModelBytes, ModelId, ModelSource, SessionDocument, SessionSnapshot,
    Tone, ToneModel,
    params::{ParameterCatalog, ParameterId, ParameterValue},
};
use ronie_ui::backend::{ChainFault, ChainFaultKind, UiError};

use ronie_ui::session_edit::SessionFrame;

pub(crate) fn encode_preset(
    frame: &SessionFrame,
    stash: Option<&LocalStash>,
    previous: Option<&JuceTree>,
    cache: &ModelByteCache,
) -> Result<JuceTree, UiError> {
    let migrated = previous.map(|document| {
        let mut document = document.clone();
        if let Some(mut parameters) = child(&document, "Params").cloned() {
            let _ = ronie_core::migrate_param_ids(&mut parameters, "id");
            upsert_child(&mut document, "Params", parameters);
        }
        document
    });
    let previous = migrated.as_ref();
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree("T3KPreset", Vec::new(), Vec::new()));
    output.identifier = "T3KPreset".to_owned();
    set_property(&mut output, "schemaVersion", JuceValue::Int(1));
    let chain = encode_chain(
        &frame.session.chain,
        stash,
        previous.and_then(|document| child(document, "ChainSnapshot")),
        cache,
    )?;
    upsert_child(&mut output, "ChainSnapshot", chain);
    let parameters = encode_parameters(
        frame,
        previous.and_then(|document| child(document, "Params")),
        "Params",
        true,
    );
    upsert_child(&mut output, "Params", parameters);
    Ok(output)
}

pub(crate) fn finalize_preset(document: &mut JuceTree, id: &str, name: &str) {
    set_property(document, "name", JuceValue::String(name.to_owned()));
    set_property(
        document,
        "id",
        JuceValue::String(id.trim_start_matches("user:").to_owned()),
    );
}

fn encode_parameters(
    frame: &SessionFrame,
    previous: Option<&JuceTree>,
    identifier: &str,
    preset_only: bool,
) -> JuceTree {
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree(identifier, Vec::new(), Vec::new()));
    let mut children = Vec::new();
    for spec in ParameterCatalog::v1().iter().filter(|spec| {
        if preset_only {
            spec.scope.preset
        } else {
            spec.scope.session
        }
    }) {
        let previous_param = previous.and_then(|params| {
            params
                .children
                .iter()
                .find(|param| string_property(param, "id") == Some(spec.id.0))
        });
        let mut param = previous_param
            .cloned()
            .unwrap_or_else(|| tree("Param", Vec::new(), Vec::new()));
        let value = frame
            .parameters
            .get(&spec.id)
            .copied()
            .unwrap_or(ParameterValue(spec.default));
        set_property(&mut param, "id", JuceValue::String(spec.id.0.to_owned()));
        set_property(&mut param, "value", JuceValue::Double(f64::from(value.0)));
        children.push(param);
    }
    if let Some(previous) = previous {
        children.extend(
            previous
                .children
                .iter()
                .filter(|param| {
                    param.identifier != "Param"
                        || !ParameterCatalog::v1()
                            .iter()
                            .any(|spec| string_property(param, "id") == Some(spec.id.0))
                })
                .cloned(),
        );
    }
    output.children = children;
    output
}

pub(crate) struct DecodedSession {
    pub(crate) frame: SessionFrame,
    pub(crate) midi_channel: u8,
    pub(crate) document: SessionDocument,
}

pub(crate) struct DecodedPreset {
    pub(crate) frame: SessionFrame,
    pub(crate) document: SessionDocument,
}

pub(crate) fn encode_session(
    frame: &SessionFrame,
    midi_channel: u8,
    stash: Option<&LocalStash>,
    previous: Option<&JuceTree>,
    cache: &ModelByteCache,
) -> Result<Vec<u8>, UiError> {
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree("RonieState", Vec::new(), Vec::new()));
    output.identifier = "RonieState".to_owned();
    set_property(&mut output, "schemaVersion", JuceValue::Int(1));
    set_property(
        &mut output,
        "inputMode",
        JuceValue::String(frame.session.input_mode.as_str().to_owned()),
    );
    set_property(
        &mut output,
        "editorScale",
        JuceValue::Double(frame.session.editor_scale),
    );
    set_property(
        &mut output,
        "editorExtraHeight",
        JuceValue::Int(frame.session.editor_extra_height),
    );
    let (active_id, active_name) = frame
        .session
        .active_preset
        .as_ref()
        .map(|preset| (preset.id.clone(), preset.name.clone()))
        .unwrap_or_default();
    set_property(&mut output, "activePresetId", JuceValue::String(active_id));
    set_property(
        &mut output,
        "activePresetName",
        JuceValue::String(active_name),
    );
    let parameters = encode_parameters(
        frame,
        previous.and_then(|document| child(document, "PARAMETERS")),
        "PARAMETERS",
        false,
    );
    upsert_child(&mut output, "PARAMETERS", parameters);
    let midi = encode_midi(
        frame,
        midi_channel,
        previous.and_then(|document| child(document, "MidiMappings")),
    );
    upsert_child(&mut output, "MidiMappings", midi);
    let chain = encode_chain(
        &frame.session.chain,
        stash,
        previous.and_then(|document| child(document, "ChainSnapshot")),
        cache,
    )?;
    upsert_child(&mut output, "ChainSnapshot", chain);
    let mut bytes = b"T3KB".to_vec();
    bytes.extend_from_slice(
        &ronie_core::JuceCodec::default()
            .write_tree(&output)
            .map_err(|error| UiError::Control(error.to_string()))?,
    );
    Ok(bytes)
}

fn encode_midi(frame: &SessionFrame, channel: u8, previous: Option<&JuceTree>) -> JuceTree {
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree("MidiMappings", Vec::new(), Vec::new()));
    set_property(&mut output, "channel", JuceValue::Int(i32::from(channel)));
    let mut children = frame
        .session
        .midi_mappings
        .iter()
        .map(|mapping| {
            let mut output = previous
                .and_then(|midi| {
                    midi.children.iter().find(|candidate| {
                        string_property(candidate, "targetId") == Some(&mapping.target_id)
                    })
                })
                .cloned()
                .unwrap_or_else(|| tree("Mapping", Vec::new(), Vec::new()));
            set_property(
                &mut output,
                "targetId",
                JuceValue::String(mapping.target_id.clone()),
            );
            set_property(
                &mut output,
                "source",
                JuceValue::String(
                    match mapping.source {
                        ronie_core::MidiSource::Cc => "cc",
                        ronie_core::MidiSource::Note => "note",
                    }
                    .to_owned(),
                ),
            );
            set_property(
                &mut output,
                "number",
                JuceValue::Int(i32::from(mapping.number)),
            );
            output
        })
        .collect::<Vec<_>>();
    if let Some(previous) = previous {
        children.extend(
            previous
                .children
                .iter()
                .filter(|child| child.identifier != "Mapping")
                .cloned(),
        );
    }
    output.children = children;
    output
}

pub(crate) fn decode_session(
    input: &[u8],
    current: &SessionFrame,
) -> Result<DecodedSession, UiError> {
    let document = ronie_core::StateCodec::new()
        .decode_session(input)
        .map_err(|error| UiError::Control(error.to_string()))?;
    let mut parameters = current.parameters.clone();
    for spec in ParameterCatalog::v1()
        .iter()
        .filter(|spec| spec.scope.session)
    {
        let value = document
            .parameters
            .children
            .iter()
            .find(|param| string_property(param, "id") == Some(spec.id.0))
            .and_then(|param| number_property(param, "value"))
            .unwrap_or(spec.default);
        parameters.insert(spec.id, checked_value(spec.id, value)?);
    }
    let midi_channel = integer_property(&document.midi, "channel")
        .unwrap_or(0)
        .clamp(0, 16) as u8;
    Ok(DecodedSession {
        frame: SessionFrame {
            session: document.snapshot.clone(),
            parameters,
            multi_core: current.multi_core,
            nam_full_size: current.nam_full_size,
        },
        midi_channel,
        document,
    })
}

pub(crate) fn decode_preset(
    document: &JuceTree,
    current: &SessionFrame,
) -> Result<DecodedPreset, UiError> {
    if !document
        .children
        .iter()
        .any(|child| child.identifier == "ChainSnapshot")
    {
        return Err(UiError::InvalidAction(
            "preset has no chain snapshot".to_owned(),
        ));
    }
    let mut state_bytes = b"T3KB".to_vec();
    state_bytes.extend_from_slice(
        &ronie_core::JuceCodec::default()
            .write_tree(document)
            .map_err(|error| UiError::Control(error.to_string()))?,
    );
    let mut document = ronie_core::StateCodec::new()
        .decode_session(&state_bytes)
        .map_err(|error| UiError::Control(error.to_string()))?;
    let mut session = document.snapshot.clone();
    session.input_mode = current.session.input_mode;
    session.editor_scale = current.session.editor_scale;
    session.editor_extra_height = current.session.editor_extra_height;
    session.midi_mappings = current.session.midi_mappings.clone();
    session.active_preset = None;
    document.snapshot = session.clone();

    let mut parameter_document = document
        .raw
        .children
        .iter()
        .find(|child| child.identifier == "Params")
        .cloned()
        .unwrap_or_else(|| tree("Params", Vec::new(), Vec::new()));
    let _ = ronie_core::migrate_param_ids(&mut parameter_document, "id");
    upsert_child(&mut document.raw, "Params", parameter_document.clone());
    let mut parameters = current.parameters.clone();
    for spec in ParameterCatalog::v1()
        .iter()
        .filter(|spec| spec.scope.preset)
    {
        let value = parameter_document
            .children
            .iter()
            .find(|param| string_property(param, "id") == Some(spec.id.0))
            .and_then(|param| number_property(param, "value"))
            .unwrap_or(spec.default);
        parameters.insert(spec.id, checked_value(spec.id, value)?);
    }
    Ok(DecodedPreset {
        frame: SessionFrame {
            session,
            parameters,
            multi_core: current.multi_core,
            nam_full_size: current.nam_full_size,
        },
        document,
    })
}

fn checked_value(id: ParameterId, value: f32) -> Result<ParameterValue, UiError> {
    if !value.is_finite() {
        return Err(UiError::InvalidAction(format!(
            "stored parameter {} is not finite",
            id.0
        )));
    }
    let spec = ParameterCatalog::v1()
        .iter()
        .find(|spec| spec.id == id)
        .ok_or(UiError::UnknownParameter(id))?;
    let mut next = value.clamp(spec.min, spec.max);
    if !spec.choices.is_empty() {
        next = next.round();
    } else if let Some(interval) = spec.interval.filter(|interval| *interval > 0.0) {
        next = (next / interval).round() * interval;
    }
    Ok(ParameterValue(next))
}

fn encode_chain(
    chain: &ChainSnapshot,
    stash: Option<&LocalStash>,
    previous: Option<&JuceTree>,
    cache: &ModelByteCache,
) -> Result<JuceTree, UiError> {
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree("ChainSnapshot", Vec::new(), Vec::new()));
    set_property(
        &mut output,
        "stereoEnabled",
        JuceValue::Bool(chain.stereo_enabled),
    );
    set_property(
        &mut output,
        "branchSide",
        JuceValue::String(chain.active_side.as_str().to_owned()),
    );
    set_property(
        &mut output,
        "branchAfterBlockId",
        JuceValue::String(
            chain
                .branch
                .as_ref()
                .map(|branch| branch.after_block_id.0.clone())
                .unwrap_or_default(),
        ),
    );
    let left = encode_lane(
        "ChainBlocks",
        &chain.left,
        stash,
        previous.and_then(|snapshot| child(snapshot, "ChainBlocks")),
        cache,
    )?;
    upsert_child(&mut output, "ChainBlocks", left);
    let right = encode_lane(
        "RightChainBlocks",
        &chain.right,
        stash,
        previous.and_then(|snapshot| child(snapshot, "RightChainBlocks")),
        cache,
    )?;
    upsert_child(&mut output, "RightChainBlocks", right);
    Ok(output)
}

fn encode_lane(
    identifier: &str,
    blocks: &[ChainBlock],
    stash: Option<&LocalStash>,
    previous: Option<&JuceTree>,
    cache: &ModelByteCache,
) -> Result<JuceTree, UiError> {
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree(identifier, Vec::new(), Vec::new()));
    let mut children = blocks
        .iter()
        .map(|block| {
            let previous_block = previous.and_then(|lane| {
                lane.children
                    .iter()
                    .find(|candidate| string_property(candidate, "id") == Some(&block.id.0))
            });
            encode_block(block, stash, previous_block, cache)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(previous) = previous {
        children.extend(
            previous
                .children
                .iter()
                .filter(|child| child.identifier != "ChainBlock")
                .cloned(),
        );
    }
    output.children = children;
    Ok(output)
}

fn encode_block(
    block: &ChainBlock,
    stash: Option<&LocalStash>,
    previous: Option<&JuceTree>,
    cache: &ModelByteCache,
) -> Result<JuceTree, UiError> {
    let kind = block
        .tone
        .as_ref()
        .map(|tone| match tone.kind {
            ronie_core::ModelKind::Nam => "NAM",
            ronie_core::ModelKind::Ir => "IR",
        })
        .unwrap_or("INSERT");
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree("ChainBlock", Vec::new(), Vec::new()));
    set_property(&mut output, "id", JuceValue::String(block.id.0.clone()));
    set_property(&mut output, "type", JuceValue::String(kind.to_owned()));
    set_property(
        &mut output,
        "enabled",
        JuceValue::Bool(block.params.enabled),
    );
    set_property(
        &mut output,
        "normalize",
        JuceValue::Bool(block.params.normalize),
    );
    set_property(
        &mut output,
        "slimSize",
        JuceValue::Double(block.params.slim_size),
    );
    set_property(
        &mut output,
        "inputGain",
        JuceValue::Double(block.params.input_gain),
    );
    set_property(
        &mut output,
        "outputGain",
        JuceValue::Double(block.params.output_gain),
    );
    set_property(&mut output, "mix", JuceValue::Double(block.params.mix));
    if let Some(tone) = &block.tone {
        let active = block.active_model_id.ok_or_else(|| {
            UiError::InvalidAction(format!("tone block {} has no active model", block.id.0))
        })?;
        let stored = tone
            .clone()
            .for_storage(active)
            .map_err(|error| UiError::Control(error.to_string()))?;
        set_property(&mut output, "toneId", JuceValue::Int64(tone.id.0));
        set_property(
            &mut output,
            "toneJson",
            JuceValue::String(
                stored
                    .to_json()
                    .map_err(|error| UiError::Control(error.to_string()))?,
            ),
        );
        set_property(&mut output, "activeModelId", JuceValue::Int64(active.0));
        let eq = encode_eq(block, previous.and_then(|block| child(block, "Eq")));
        upsert_child(&mut output, "Eq", eq);
        let cache = encode_model_cache(
            block,
            stash,
            previous.and_then(|block| child(block, "ModelCache")),
            cache,
        )?;
        upsert_child(&mut output, "ModelCache", cache);
    } else {
        for name in ["toneId", "toneJson", "activeModelId"] {
            remove_property(&mut output, name);
        }
        output
            .children
            .retain(|child| child.identifier != "Eq" && child.identifier != "ModelCache");
    }
    Ok(output)
}

fn encode_eq(block: &ChainBlock, previous: Option<&JuceTree>) -> JuceTree {
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree("Eq", Vec::new(), Vec::new()));
    set_property(
        &mut output,
        "enabled",
        JuceValue::Bool(block.params.eq.enabled),
    );
    set_property(&mut output, "pre", JuceValue::Bool(block.params.eq.pre));
    let mut children = block
        .params
        .eq
        .bands
        .iter()
        .enumerate()
        .map(|(index, band)| {
            let mut output = previous
                .and_then(|eq| eq.children.get(index))
                .cloned()
                .unwrap_or_else(|| tree("Band", Vec::new(), Vec::new()));
            let kind = match band.kind {
                EqBandType::Lowcut => "lowcut",
                EqBandType::Lowshelf => "lowshelf",
                EqBandType::Bell => "bell",
                EqBandType::Highshelf => "highshelf",
                EqBandType::Highcut => "highcut",
            };
            set_property(&mut output, "type", JuceValue::String(kind.to_owned()));
            set_property(&mut output, "freqHz", JuceValue::Double(band.freq_hz));
            set_property(&mut output, "gainDb", JuceValue::Double(band.gain_db));
            set_property(&mut output, "q", JuceValue::Double(band.q));
            output
        })
        .collect::<Vec<_>>();
    if let Some(previous) = previous {
        children.extend(
            previous
                .children
                .iter()
                .filter(|child| child.identifier != "Band")
                .cloned(),
        );
    }
    output.children = children;
    output
}

fn encode_model_cache(
    block: &ChainBlock,
    stash: Option<&LocalStash>,
    previous: Option<&JuceTree>,
    bytes_cache: &ModelByteCache,
) -> Result<JuceTree, UiError> {
    let mut output = previous
        .cloned()
        .unwrap_or_else(|| tree("ModelCache", Vec::new(), Vec::new()));
    let mut cached = Vec::new();
    let Some(tone) = &block.tone else {
        output.children = Vec::new();
        return Ok(output);
    };
    for model in tone
        .models
        .iter()
        .filter(|model| ronie_core::references_model(block, model.id))
    {
        let previous_model = previous.and_then(|cache| {
            cache.children.iter().find(|cached| {
                cached.identifier == "CachedModel"
                    && integer_property(cached, "modelId") == Some(model.id.0)
            })
        });
        let mut cached_model = previous_model
            .cloned()
            .unwrap_or_else(|| tree("CachedModel", Vec::new(), Vec::new()));
        set_property(&mut cached_model, "modelId", JuceValue::Int64(model.id.0));
        if matches!(model.source, Some(ModelSource::Local { .. })) {
            let stashed = (|| {
                let url = model
                    .raw
                    .get("model_url")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        UiError::InvalidAction(format!(
                            "local model {} has no source URL",
                            model.id.0
                        ))
                    })?;
                let stash = stash.ok_or_else(|| {
                    UiError::InvalidAction(format!(
                        "local model stash for model {} is unavailable",
                        model.id.0
                    ))
                })?;
                let path = stash.resolve(url).map_err(|error| {
                    UiError::Control(format!(
                        "resolving local model {} from {url}: {error}",
                        model.id.0
                    ))
                })?;
                std::fs::read(&path).map_err(|error| {
                    UiError::Control(format!(
                        "reading local model {} from {}: {error}",
                        model.id.0,
                        path.display()
                    ))
                })
            })();
            let bytes = match stashed {
                Ok(bytes) if holds_local_identity(model, &bytes) => bytes,
                stashed => {
                    let embedded = previous_model
                        .and_then(|cached| {
                            cached.properties.iter().find(|(name, _)| name == "data")
                        })
                        .and_then(|(_, value)| match value {
                            JuceValue::Binary(bytes) => Some(bytes.clone()),
                            _ => None,
                        });
                    match embedded {
                        Some(bytes) => bytes,
                        None => stashed?,
                    }
                }
            };
            set_property(&mut cached_model, "data", JuceValue::Binary(bytes));
        } else if let Some(bytes) = bytes_cache.get(&ByteCacheKey {
            block_id: block.id.clone(),
            model_id: model.id,
            namespace: ronie_core::model_namespace(block, model.id),
        }) {
            // A catalog model must restore with no network, so its bytes travel with the state.
            set_property(&mut cached_model, "data", JuceValue::Binary(bytes.to_vec()));
        } else if previous_model.is_none() {
            continue;
        }
        cached.push(cached_model);
    }
    if let Some(previous) = previous {
        cached.extend(
            previous
                .children
                .iter()
                .filter(|child| child.identifier != "CachedModel")
                .cloned(),
        );
    }
    output.children = cached;
    Ok(output)
}

fn set_property(tree: &mut JuceTree, name: &str, value: JuceValue) {
    if let Some(existing) = tree
        .properties
        .iter_mut()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
    {
        *existing = value;
    } else {
        tree.properties.push((name.to_owned(), value));
    }
}

fn remove_property(tree: &mut JuceTree, name: &str) {
    tree.properties.retain(|(key, _)| key != name);
}

fn child<'a>(tree: &'a JuceTree, name: &str) -> Option<&'a JuceTree> {
    tree.children.iter().find(|child| child.identifier == name)
}

fn upsert_child(tree: &mut JuceTree, name: &str, mut value: JuceTree) {
    value.identifier = name.to_owned();
    if let Some(existing) = tree
        .children
        .iter_mut()
        .find(|child| child.identifier == name)
    {
        *existing = value;
    } else {
        tree.children.push(value);
    }
}

fn integer_property(tree: &JuceTree, name: &str) -> Option<i64> {
    tree.properties
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| match value {
            JuceValue::Int(value) => Some(i64::from(*value)),
            JuceValue::Int64(value) => Some(*value),
            JuceValue::Double(value) => Some(*value as i64),
            JuceValue::String(value) => value.parse().ok(),
            _ => None,
        })
}

fn tree(
    identifier: &str,
    properties: Vec<(String, JuceValue)>,
    children: Vec<JuceTree>,
) -> JuceTree {
    JuceTree {
        identifier: identifier.to_owned(),
        properties,
        children,
    }
}

fn string_property<'a>(tree: &'a JuceTree, name: &str) -> Option<&'a str> {
    tree.properties
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| match value {
            JuceValue::String(value) => Some(value.as_str()),
            _ => None,
        })
}

fn number_property(tree: &JuceTree, name: &str) -> Option<f32> {
    tree.properties
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| match value {
            JuceValue::Int(value) => Some(*value as f32),
            JuceValue::Int64(value) => Some(*value as f32),
            JuceValue::Double(value) => Some(*value as f32),
            JuceValue::String(value) => value.parse().ok(),
            _ => None,
        })
}

/// Checks a restore with no compile, and publishes its embedded models to the byte cache.
///
/// Each tone block must have an active model with embedded or cached bytes, as for an offline
/// restore. A model that has bytes but cannot compile passes: the chain of the session runs that
/// block as bypass and shows its fault.
pub(crate) fn check_restore(
    document: &SessionDocument,
    current: &SessionFrame,
    cache: &ModelByteCache,
) -> Result<(), UiError> {
    let plan = plan_restore(document, current, cache)?;
    let chain = &document.snapshot.chain;
    for block in chain.left.iter().chain(&chain.right) {
        restored_model(block, &plan.jobs)?;
    }
    Ok(())
}

fn plan_restore(
    document: &SessionDocument,
    current: &SessionFrame,
    cache: &ModelByteCache,
) -> Result<ronie_core::RestorePlan, UiError> {
    ronie_core::restore_offline(
        document,
        ronie_core::RestoreCaches {
            bytes: cache,
            current: Some(&current.session),
        },
    )
    .map_err(|error| UiError::Control(format!("planning offline model restore: {error}")))
}

/// Prepares a saved session at launch. Returns `None` when a saved model cannot compile, because
/// the audio engine must then prepare the chain from its own sources and report that block.
/// Every other fault of the session stays an error.
pub(crate) fn prepare_session_restore(
    document: &SessionDocument,
    current: &SessionFrame,
    spec: ronie_dsp::PrepareSpec,
    generation: u64,
    cache: &ModelByteCache,
) -> Result<Option<ronie_dsp::prepare::PreparedChainChange>, UiError> {
    let lanes = prepare_restored_lanes(document, current, cache)?;
    let usable = lanes.compile_error.is_none();
    let change = prepare_change(
        &document.snapshot,
        spec,
        generation,
        lanes.left,
        lanes.right,
    )?;
    Ok(usable.then_some(change))
}

/// Lanes of a restore, where a block whose model cannot compile runs as bypass.
struct RestoredLanes {
    left: ronie_dsp::lane::LaneSnapshot,
    right: ronie_dsp::lane::LaneSnapshot,
    /// The first model compile failure.
    compile_error: Option<UiError>,
}

fn prepare_restored_lanes(
    document: &SessionDocument,
    current: &SessionFrame,
    cache: &ModelByteCache,
) -> Result<RestoredLanes, UiError> {
    let plan = plan_restore(document, current, cache)?;
    let chain = &document.snapshot.chain;
    let mut compile_error = None;
    let left = prepare_lane(&chain.left, &plan.jobs, &mut compile_error)?;
    let right = prepare_lane(&chain.right, &plan.jobs, &mut compile_error)?;
    Ok(RestoredLanes {
        left,
        right,
        compile_error,
    })
}

/// Whether an active model in this document has no embedded bytes.
pub(crate) fn active_model_lacks_bytes(document: &SessionDocument) -> bool {
    let chain = &document.snapshot.chain;
    chain
        .left
        .iter()
        .chain(&chain.right)
        .filter(|block| !block.is_insert)
        .any(|block| {
            block.active_model_id.is_some_and(|active| {
                !document
                    .model_cache
                    .iter()
                    .any(|entry| entry.block_id == block.id && entry.model_id == active)
            })
        })
}

/// Sources of model bytes for a chain prepared from the live session.
pub(crate) struct ModelSources<'a> {
    pub(crate) stash: Option<&'a LocalStash>,
    pub(crate) document: Option<&'a JuceTree>,
    pub(crate) cache: &'a ModelByteCache,
}

/// Prepares the live session. A block without usable model bytes runs as bypass, so that one
/// missing model cannot stop every other edit from reaching audio.
pub(crate) fn prepare_session_chain(
    session: &SessionSnapshot,
    spec: ronie_dsp::PrepareSpec,
    generation: u64,
    sources: &ModelSources<'_>,
) -> Result<crate::audio_session::SessionChain, UiError> {
    let mut unavailable = Vec::new();
    let left = live_lane(&session.chain.left, sources, &mut unavailable);
    let right = live_lane(&session.chain.right, sources, &mut unavailable);
    let change = prepare_change(session, spec, generation, left, right)?;
    Ok(crate::audio_session::SessionChain {
        change,
        unavailable,
    })
}

fn prepare_change(
    session: &SessionSnapshot,
    spec: ronie_dsp::PrepareSpec,
    generation: u64,
    left: ronie_dsp::lane::LaneSnapshot,
    right: ronie_dsp::lane::LaneSnapshot,
) -> Result<ronie_dsp::prepare::PreparedChainChange, UiError> {
    let spec = ronie_dsp::PrepareSpec {
        input_mode: session.input_mode,
        ..spec
    };
    let mut change = ronie_dsp::prepare::PreparedChainChange::new(
        generation,
        spec,
        left,
        right,
        session.chain.stereo_enabled,
        ronie_dsp::LatencyReport {
            samples: 0,
            tail_seconds: 0.0,
        },
    )
    .map_err(|error| UiError::Control(format!("preparing audio chain: {error}")))?;
    if let Some(ChainBranch {
        side,
        after_block_id,
    }) = &session.chain.branch
    {
        // A tap on a bypassed block is rejected here, and the chains then run independently.
        let _ = change
            .topology
            .set_branch(&mut change.blocks, *side, after_block_id.clone());
    }
    Ok(change)
}

fn live_lane(
    blocks: &[ChainBlock],
    sources: &ModelSources<'_>,
    unavailable: &mut Vec<(BlockId, ChainFault)>,
) -> ronie_dsp::lane::LaneSnapshot {
    let mut snapshot = ronie_dsp::lane::LaneSnapshot {
        blocks: Vec::with_capacity(blocks.len()),
        models: Vec::with_capacity(blocks.len()),
    };
    for block in blocks {
        if block.is_insert {
            snapshot.blocks.push(block.clone());
            snapshot.models.push(None);
            continue;
        }
        match live_model(block, sources) {
            Ok(model) => {
                snapshot.blocks.push(block.clone());
                snapshot.models.push(Some(model));
            }
            Err(kind) => {
                unavailable.push((block.id.clone(), ChainFault::for_block(kind, block)));
                snapshot.blocks.push(ChainBlock::insert(block.id.0.clone()));
                snapshot.models.push(None);
            }
        }
    }
    snapshot
}

fn live_model(
    block: &ChainBlock,
    sources: &ModelSources<'_>,
) -> Result<ronie_dsp::block::BlockModel, ChainFaultKind> {
    let (Some(tone), Some(active)) = (block.tone.as_ref(), block.active_model_id) else {
        return Err(ChainFaultKind::MissingModel);
    };
    let model = tone
        .models
        .iter()
        .find(|model| model.id == active)
        .ok_or(ChainFaultKind::MissingModel)?;
    let bytes =
        resolve_model_bytes(block, tone, model, sources).ok_or(ChainFaultKind::MissingModel)?;
    compile_block_model(&block.id, tone, model, &bytes).map_err(|_| ChainFaultKind::ModelRejected)
}

/// Compiles untrusted bytes as one model of `tone`, and keeps nothing.
pub(crate) fn validate_model(
    tone: &Tone,
    model_id: ModelId,
    bytes: &ModelBytes,
) -> Result<(), UiError> {
    let model = tone
        .models
        .iter()
        .find(|model| model.id == model_id)
        .ok_or_else(|| {
            UiError::InvalidAction(format!(
                "tone {} does not contain model {}",
                tone.id.0, model_id.0
            ))
        })?;
    compile_block_model(&BlockId("candidate".to_owned()), tone, model, bytes).map(|_| ())
}

/// Whether a stash file still holds the model. A local model takes its identity from its bytes, so
/// a file with other bytes is corrupt, also when it can compile.
fn holds_local_identity(model: &ToneModel, bytes: &[u8]) -> bool {
    ModelId::from_local_bytes(bytes) == model.id
}

fn stashed_model_bytes(model: &ToneModel, sources: &ModelSources<'_>) -> Option<Vec<u8>> {
    if !matches!(model.source, Some(ModelSource::Local { .. })) {
        return None;
    }
    let url = model.raw.get("model_url")?.as_str()?;
    std::fs::read(sources.stash?.resolve(url).ok()?).ok()
}

fn resolve_model_bytes(
    block: &ChainBlock,
    tone: &Tone,
    model: &ToneModel,
    sources: &ModelSources<'_>,
) -> Option<ModelBytes> {
    let stashed = stashed_model_bytes(model, sources);
    if let Some(bytes) = stashed
        .as_ref()
        .filter(|bytes| holds_local_identity(model, bytes))
    {
        return Some(ModelBytes(bytes.as_slice().into()));
    }
    // A restore seeds the cache from its document, so an entry here is at least as new. A new
    // download of the same model must win over the document of an earlier launch.
    sources
        .cache
        .get(&ByteCacheKey {
            block_id: block.id.clone(),
            model_id: model.id,
            namespace: ronie_core::model_namespace(block, model.id),
        })
        .or_else(|| embedded_model_bytes(sources.document, &block.id, tone, model.id))
        .or_else(|| stashed.map(|bytes| ModelBytes(bytes.into())))
}

fn embedded_model_bytes(
    document: Option<&JuceTree>,
    block_id: &BlockId,
    tone: &Tone,
    model_id: ModelId,
) -> Option<ModelBytes> {
    let snapshot = child(document?, "ChainSnapshot")?;
    let block = ["ChainBlocks", "RightChainBlocks"]
        .iter()
        .filter_map(|lane| child(snapshot, lane))
        .flat_map(|lane| &lane.children)
        .find(|block| string_property(block, "id") == Some(&block_id.0))
        .filter(|block| integer_property(block, "toneId") == Some(tone.id.0))?;
    child(block, "ModelCache")?
        .children
        .iter()
        .find(|cached| {
            cached.identifier == "CachedModel"
                && integer_property(cached, "modelId") == Some(model_id.0)
        })?
        .properties
        .iter()
        .find_map(|(name, value)| match value {
            JuceValue::Binary(bytes) if name == "data" => Some(ModelBytes(bytes.as_slice().into())),
            _ => None,
        })
}

/// Prepares one lane of a restore. A model that cannot compile does not stop the lane, so that a
/// later fault of the document stays an error and does not hide behind that model.
fn prepare_lane(
    blocks: &[ChainBlock],
    jobs: &[ronie_core::PreparationJob],
    compile_error: &mut Option<UiError>,
) -> Result<ronie_dsp::lane::LaneSnapshot, UiError> {
    let mut snapshot = ronie_dsp::lane::LaneSnapshot {
        blocks: Vec::with_capacity(blocks.len()),
        models: Vec::with_capacity(blocks.len()),
    };
    for block in blocks {
        let Some((tone, model, bytes)) = restored_model(block, jobs)? else {
            snapshot.blocks.push(block.clone());
            snapshot.models.push(None);
            continue;
        };
        match compile_block_model(&block.id, tone, model, bytes) {
            Ok(model) => {
                snapshot.blocks.push(block.clone());
                snapshot.models.push(Some(model));
            }
            Err(error) => {
                compile_error.get_or_insert(error);
                snapshot.blocks.push(ChainBlock::insert(block.id.0.clone()));
                snapshot.models.push(None);
            }
        }
    }
    Ok(snapshot)
}

/// The tone, the active model, and the bytes that a restore prepares for one block, with no
/// compile. An insert has none.
fn restored_model<'a>(
    block: &'a ChainBlock,
    jobs: &'a [ronie_core::PreparationJob],
) -> Result<Option<(&'a Tone, &'a ToneModel, &'a ModelBytes)>, UiError> {
    if block.is_insert {
        return Ok(None);
    }
    let active = block.active_model_id.ok_or_else(|| {
        UiError::InvalidAction(format!("tone block {} has no active model", block.id.0))
    })?;
    let job = jobs
        .iter()
        .find(|job| job.block_id == block.id && job.model_id == active)
        .ok_or_else(|| {
            UiError::Control(format!(
                "offline restore omitted model {} in block {}",
                active.0, block.id.0
            ))
        })?;
    let bytes = job.bytes.as_ref().ok_or_else(|| {
        UiError::Control(format!(
            "model {} in block {} has no embedded bytes for offline preparation",
            active.0, block.id.0
        ))
    })?;
    let tone = block.tone.as_ref().ok_or_else(|| {
        UiError::Control(format!("tone block {} has no tone metadata", block.id.0))
    })?;
    let model = tone
        .models
        .iter()
        .find(|model| model.id == active)
        .ok_or_else(|| {
            UiError::Control(format!(
                "tone block {} does not contain model {}",
                block.id.0, active.0
            ))
        })?;
    Ok(Some((tone, model, bytes)))
}

fn compile_block_model(
    block_id: &BlockId,
    tone: &Tone,
    model: &ToneModel,
    bytes: &ModelBytes,
) -> Result<ronie_dsp::block::BlockModel, UiError> {
    match tone.kind {
        ronie_core::ModelKind::Nam => {
            let document = ronie_core::parse_nam(bytes.as_ref()).map_err(|error| {
                UiError::Control(format!(
                    "parsing NAM model {} in block {}: {error}",
                    model.id.0, block_id.0
                ))
            })?;
            let compiled = ronie_dsp::compile_model(&document).map_err(|error| {
                UiError::Control(format!(
                    "compiling NAM model {} in block {}: {error}",
                    model.id.0, block_id.0
                ))
            })?;
            Ok(ronie_dsp::block::BlockModel::Nam(compiled))
        }
        ronie_core::ModelKind::Ir => {
            let gear = model.raw.get("gear").and_then(|value| value.as_str());
            let compiled = ronie_dsp::ir::compile_ir_bytes(bytes, gear).map_err(|error| {
                UiError::Control(format!(
                    "compiling WAV model {} in block {}: {error}",
                    model.id.0, block_id.0
                ))
            })?;
            Ok(ronie_dsp::block::BlockModel::Ir(compiled))
        }
    }
}
