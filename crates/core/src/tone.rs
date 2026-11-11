use std::{fmt, ops::Deref, sync::Arc};

use serde_json::Value;
use url::Url;

use crate::ToneError;

const MAX_TONE_JSON_BYTES: usize = 50 * 1024 * 1024;
const MAX_MODELS_PER_TONE: usize = 300;
const FNV_OFFSET_BASIS: u64 = 1_469_598_103_934_665_603;
const FNV_PRIME: u64 = 1_099_511_628_211;

/// Stable model identity inside one chain block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(pub i64);

impl ModelId {
    /// Computes the positive local identity from the Reference FNV-1a hash.
    #[must_use]
    pub fn from_local_bytes(bytes: &[u8]) -> Self {
        let mut hash = FNV_OFFSET_BASIS;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        Self((hash % 0x7fff_fffe) as i64 + 1)
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Stable catalog tone identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToneId(pub i64);

impl fmt::Display for ToneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Model family used by a tone block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    /// Neural amplifier model.
    Nam,
    /// Impulse-response audio.
    Ir,
}

impl ModelKind {
    /// Persisted format spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nam => "nam",
            Self::Ir => "ir",
        }
    }
}

/// Location from which model bytes can be acquired.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModelSource {
    /// Remote catalog resource.
    Catalog {
        /// Parsed remote URL.
        url: Url,
    },
    /// Content-addressed local stash entry.
    Local {
        /// Filename only, never an attacker-selected path.
        stash_name: String,
    },
}

/// One model summary plus its complete original JSON object.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneModel {
    /// Model identity.
    pub id: ModelId,
    /// Display name.
    pub name: String,
    /// Byte source, absent in slim UI-only summaries.
    pub source: Option<ModelSource>,
    /// Complete JSON including fields unknown to this version.
    pub raw: Value,
}

impl ToneModel {
    /// Converts one model JSON object without discarding unknown fields.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-object model, invalid URL, or invalid local path.
    pub fn from_value(value: &Value, local: bool) -> Result<Self, ToneError> {
        let object = value
            .as_object()
            .ok_or(ToneError::InvalidDocument("model must be an object"))?;
        let id = ModelId(object.get("id").and_then(Value::as_i64).unwrap_or(0));
        let name = string_field(object.get("name"), "");
        let source = match object.get("model_url").and_then(Value::as_str) {
            Some(raw_url) if !raw_url.is_empty() => {
                let url = Url::parse(raw_url)?;
                if local || url.scheme() == "file" {
                    let path = url.to_file_path().map_err(|_| {
                        ToneError::InvalidDocument("local model URL must be a file path")
                    })?;
                    let stash_name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                        .ok_or(ToneError::InvalidDocument(
                            "local model URL must end in a UTF-8 stash filename",
                        ))?;
                    Some(ModelSource::Local { stash_name })
                } else {
                    Some(ModelSource::Catalog { url })
                }
            }
            _ => None,
        };
        Ok(Self {
            id,
            name,
            source,
            raw: value.clone(),
        })
    }
}

/// Typed tone metadata with lossless raw JSON for forward-compatible writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Tone {
    /// Catalog identity, or zero for a synthetic local tone.
    pub id: ToneId,
    /// Display title.
    pub title: String,
    /// Model family.
    pub kind: ModelKind,
    /// Whether this tone uses the local full-model-list persistence rule.
    pub local: bool,
    /// Models currently referenced by the tone JSON.
    pub models: Vec<ToneModel>,
    /// Complete parsed JSON including every unrecognized field.
    pub raw: Value,
}

impl Tone {
    /// Parses and validates one tone JSON document.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized or malformed JSON, invalid model entries,
    /// invalid URLs, or more than 300 models.
    pub fn from_json(input: &str) -> Result<Self, ToneError> {
        if input.len() > MAX_TONE_JSON_BYTES {
            return Err(ToneError::InputTooLarge);
        }
        let value = serde_json::from_str(input)?;
        Self::from_value(value)
    }

    /// Converts a parsed JSON value into a tone without discarding any fields.
    ///
    /// # Errors
    ///
    /// Returns an error when the document shape, models, or URLs are invalid.
    pub fn from_value(value: Value) -> Result<Self, ToneError> {
        let object = value
            .as_object()
            .ok_or(ToneError::InvalidDocument("tone must be an object"))?;
        let id = ToneId(object.get("id").and_then(Value::as_i64).unwrap_or(0));
        let title = string_field(object.get("title"), "");
        let format = string_field(object.get("format"), "nam");
        let kind = if format.eq_ignore_ascii_case("ir") {
            ModelKind::Ir
        } else {
            ModelKind::Nam
        };
        let local = object
            .get("local")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let model_values = match object.get("models") {
            Some(Value::Array(models)) => models,
            Some(_) => return Err(ToneError::InvalidDocument("models must be an array")),
            None => &Vec::new(),
        };
        if model_values.len() > MAX_MODELS_PER_TONE {
            return Err(ToneError::TooManyModels {
                actual: model_values.len(),
                max: MAX_MODELS_PER_TONE,
            });
        }
        let mut models = Vec::with_capacity(model_values.len());
        for model in model_values {
            models.push(ToneModel::from_value(model, local)?);
        }
        Ok(Self {
            id,
            title,
            kind,
            local,
            models,
            raw: value,
        })
    }

    /// Returns the tone shape whose models persistence may reference.
    ///
    /// Local tones retain their complete list. Catalog tones retain only the
    /// active model, in both the typed projection and raw JSON.
    ///
    /// # Errors
    ///
    /// Returns [`ToneError::MissingActiveModel`] if a catalog tone does not
    /// contain the requested active model.
    pub fn for_storage(self, active_model_id: ModelId) -> Result<Self, ToneError> {
        if self.local {
            return Ok(self);
        }
        let active = self
            .models
            .iter()
            .position(|model| model.id == active_model_id)
            .ok_or(ToneError::MissingActiveModel(active_model_id))?;
        let mut stored = self;
        stored.models = vec![stored.models[active].clone()];
        let raw_model = stored.models[0].raw.clone();
        let object = stored
            .raw
            .as_object_mut()
            .ok_or(ToneError::InvalidDocument("tone must be an object"))?;
        object.insert("models".into(), Value::Array(vec![raw_model]));
        Ok(stored)
    }

    /// Serializes the complete retained JSON document.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    pub fn to_json(&self) -> Result<String, ToneError> {
        Ok(serde_json::to_string(&self.raw)?)
    }
}

/// Immutable shared model bytes. Cloning this value does not copy the bytes.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ModelBytes(pub Arc<[u8]>);

/// Diagnostic text shows the size only, because a model can be 50 MiB.
impl std::fmt::Debug for ModelBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ModelBytes({} bytes)", self.len())
    }
}

impl ModelBytes {
    /// Returns the byte length without exposing allocation details.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the model byte string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Deref for ModelBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<[u8]> for ModelBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for ModelBytes {
    fn from(value: Vec<u8>) -> Self {
        Self(Arc::from(value))
    }
}

impl From<&[u8]> for ModelBytes {
    fn from(value: &[u8]) -> Self {
        Self(Arc::from(value))
    }
}

fn string_field(value: Option<&Value>, fallback: &str) -> String {
    value.and_then(Value::as_str).unwrap_or(fallback).to_owned()
}
