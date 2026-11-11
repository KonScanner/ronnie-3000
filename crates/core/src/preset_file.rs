use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::{JuceCodec, JuceTree, JuceValue, PresetError};

const MAGIC_V2: &[u8; 4] = b"T3KH";
const MAGIC_V1: &[u8; 4] = b"T3KB";
const HEADER_TAG: &str = "T3KPresetHeader";
const BODY_TAG: &str = "T3KPreset";
const MAX_HEADER_BYTES: usize = 65_536;
const MAX_STEM_BYTES: usize = 100;
/// Preset filename extension used by existing stores.
pub const PRESET_EXTENSION: &str = ".t3kpreset";

/// Preset framing version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetFraming {
    /// Legacy `T3KB` body-only framing.
    V1,
    /// Current `T3KH` header/body framing.
    V2,
}

/// Identity fields read from a preset header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetHeader {
    /// Trimmed preset identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether identity came from a legacy full-body parse.
    pub legacy: bool,
}

impl PresetHeader {
    /// Returns the framing used by this header.
    #[must_use]
    pub const fn framing(&self) -> PresetFraming {
        if self.legacy {
            PresetFraming::V1
        } else {
            PresetFraming::V2
        }
    }
}

/// Reads v2 identity from the small header, or v1 identity from the complete body.
///
/// # Errors
///
/// Returns invalid framing, length, tree type, or binary codec errors.
pub fn read_preset_header(input: &[u8]) -> Result<PresetHeader, PresetError> {
    let codec = JuceCodec::default();
    if input.starts_with(MAGIC_V2) {
        let header_len = read_i32(input, 4)? as usize;
        if header_len == 0 || header_len > MAX_HEADER_BYTES {
            return Err(PresetError::InvalidHeader);
        }
        let end = 8_usize
            .checked_add(header_len)
            .ok_or(PresetError::InvalidHeader)?;
        if end > input.len() {
            return Err(PresetError::InvalidHeader);
        }
        let tree = codec.read_tree(&input[8..end])?;
        if tree.identifier != HEADER_TAG {
            return Err(PresetError::InvalidHeader);
        }
        return Ok(PresetHeader {
            id: string_property(&tree, "id")
                .unwrap_or_default()
                .trim()
                .to_owned(),
            name: string_property(&tree, "name")
                .unwrap_or_default()
                .to_owned(),
            legacy: false,
        });
    }
    if input.starts_with(MAGIC_V1) {
        let tree = codec.read_tree(&input[MAGIC_V1.len()..])?;
        if tree.identifier != BODY_TAG {
            return Err(PresetError::InvalidBody);
        }
        return Ok(PresetHeader {
            id: string_property(&tree, "id")
                .unwrap_or_default()
                .trim()
                .to_owned(),
            name: string_property(&tree, "name")
                .unwrap_or_default()
                .to_owned(),
            legacy: true,
        });
    }
    Err(PresetError::InvalidFraming)
}

/// Reads only a v2 header from disk. Legacy v1 files require one full-body read.
/// Embedded model bodies are never read for v2 identity scans.
///
/// # Errors
///
/// Returns framing, header, or file I/O errors.
pub fn read_preset_header_file(path: &Path) -> Result<PresetHeader, PresetError> {
    let mut file = std::fs::File::open(path)?;
    let mut prefix = [0_u8; 8];
    let mut read = 0_usize;
    while read < prefix.len() {
        let count = file.read(&mut prefix[read..])?;
        if count == 0 {
            return Err(PresetError::InvalidHeader);
        }
        read += count;
    }
    if &prefix[..4] == MAGIC_V2 {
        let header_len = i32::from_le_bytes(
            prefix[4..]
                .try_into()
                .map_err(|_| PresetError::InvalidHeader)?,
        ) as usize;
        if header_len == 0 || header_len > MAX_HEADER_BYTES {
            return Err(PresetError::InvalidHeader);
        }
        let mut header = vec![0_u8; header_len];
        file.read_exact(&mut header)?;
        let tree = JuceCodec::default().read_tree(&header)?;
        if tree.identifier != HEADER_TAG {
            return Err(PresetError::InvalidHeader);
        }
        return Ok(PresetHeader {
            id: string_property(&tree, "id")
                .unwrap_or_default()
                .trim()
                .to_owned(),
            name: string_property(&tree, "name")
                .unwrap_or_default()
                .to_owned(),
            legacy: false,
        });
    }
    drop(file);
    let tree = read_preset_file(path)?;
    Ok(PresetHeader {
        id: string_property(&tree, "id")
            .unwrap_or_default()
            .trim()
            .to_owned(),
        name: string_property(&tree, "name")
            .unwrap_or_default()
            .to_owned(),
        legacy: true,
    })
}

/// Reads a complete preset from disk with an aggregate byte bound.
///
/// # Errors
///
/// Returns [`PresetError::InputTooLarge`], framing, codec, or file I/O errors.
pub fn read_preset_file(path: &Path) -> Result<JuceTree, PresetError> {
    const MAX_PRESET_BYTES: usize = 512 * 1024 * 1024;
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_PRESET_BYTES as u64 {
        return Err(PresetError::InputTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_PRESET_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PRESET_BYTES {
        return Err(PresetError::InputTooLarge);
    }
    read_preset(&bytes)
}

/// Reads a complete preset body in either framing.
///
/// # Errors
///
/// Returns invalid framing, header, body type, or binary codec errors.
pub fn read_preset(input: &[u8]) -> Result<JuceTree, PresetError> {
    const MAX_PRESET_BYTES: usize = 512 * 1024 * 1024;
    if input.len() > MAX_PRESET_BYTES {
        return Err(PresetError::InputTooLarge);
    }
    let codec = JuceCodec::default();
    let body = if input.starts_with(MAGIC_V2) {
        let header_len = read_i32(input, 4)? as usize;
        if header_len == 0 || header_len > MAX_HEADER_BYTES {
            return Err(PresetError::InvalidHeader);
        }
        let end = 8_usize
            .checked_add(header_len)
            .ok_or(PresetError::InvalidHeader)?;
        if end > input.len() {
            return Err(PresetError::InvalidHeader);
        }
        &input[end..]
    } else if input.starts_with(MAGIC_V1) {
        &input[MAGIC_V1.len()..]
    } else {
        return Err(PresetError::InvalidFraming);
    };
    let tree = codec.read_tree(body)?;
    if tree.identifier != BODY_TAG {
        return Err(PresetError::InvalidBody);
    }
    Ok(tree)
}

/// Encodes a complete preset in v2 framing.
///
/// # Errors
///
/// Returns invalid body, header-length, or binary codec errors.
pub fn write_preset_v2(preset: &JuceTree) -> Result<Vec<u8>, PresetError> {
    if preset.identifier != BODY_TAG {
        return Err(PresetError::InvalidBody);
    }
    let codec = JuceCodec::default();
    let header = JuceTree {
        identifier: HEADER_TAG.into(),
        properties: vec![
            (
                "id".into(),
                JuceValue::String(string_property(preset, "id").unwrap_or_default().to_owned()),
            ),
            (
                "name".into(),
                JuceValue::String(
                    string_property(preset, "name")
                        .unwrap_or_default()
                        .to_owned(),
                ),
            ),
        ],
        children: Vec::new(),
    };
    let header_bytes = codec.write_tree(&header)?;
    if header_bytes.is_empty() || header_bytes.len() > MAX_HEADER_BYTES {
        return Err(PresetError::InvalidHeader);
    }
    let body_bytes = codec.write_tree(preset)?;
    let header_len = i32::try_from(header_bytes.len()).map_err(|_| PresetError::InvalidHeader)?;
    let mut output = Vec::with_capacity(MAGIC_V2.len() + 4 + header_bytes.len() + body_bytes.len());
    output.extend_from_slice(MAGIC_V2);
    output.extend_from_slice(&header_len.to_le_bytes());
    output.extend_from_slice(&header_bytes);
    output.extend_from_slice(&body_bytes);
    Ok(output)
}

/// Atomically replaces a user preset. Factory presets are always refused.
///
/// # Errors
///
/// Returns [`PresetError::FactoryReadOnly`] for factory writes, or encoding/I/O
/// errors while preserving any previous file and removing incomplete temp files.
pub fn write_preset_v2_file(
    path: &Path,
    preset: &JuceTree,
    factory: bool,
) -> Result<(), PresetError> {
    if factory {
        return Err(PresetError::FactoryReadOnly);
    }
    let bytes = write_preset_v2(preset)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| PresetError::Io(error.error))?;
    Ok(())
}

/// Writes a user preset to its canonical filename, then removes the superseded file.
/// A same-file update is atomic; a rename writes the new file before deleting the old one.
///
/// # Errors
///
/// Returns encoding or filesystem errors while preserving the previous preset when possible.
pub fn write_user_preset(
    dir: &Path,
    current: Option<&Path>,
    name: &str,
    preset: &JuceTree,
) -> Result<PathBuf, PresetError> {
    let target = unique_preset_path(dir, &sanitize_stem(name), current);
    if current == Some(target.as_path()) {
        write_preset_v2_file(&target, preset, false)?;
        if let Some(current) = current.filter(|current| current.file_name() != target.file_name()) {
            let _ = std::fs::rename(current, &target);
        }
        return Ok(target);
    }
    write_preset_v2_file(&target, preset, false)?;
    if let Some(current) = current.filter(|current| current.exists()) {
        let _ = std::fs::remove_file(current);
    }
    Ok(target)
}

/// Sanitizes a display name using the Reference cross-platform filename rules.
#[must_use]
pub fn sanitize_stem(name: &str) -> String {
    let mut replaced = String::new();
    let mut last_was_replacement = false;
    for character in name.chars() {
        if forbidden(character) {
            if !last_was_replacement {
                replaced.push('-');
            }
            last_was_replacement = true;
        } else {
            replaced.push(character);
            last_was_replacement = false;
        }
    }
    let mut capped = String::new();
    let mut bytes = 0_usize;
    for character in replaced.chars() {
        bytes += character.len_utf8();
        if bytes > MAX_STEM_BYTES {
            break;
        }
        capped.push(character);
    }
    let trimmed = capped.trim_start_matches(|character: char| {
        character == '.' || character == '~' || character.is_whitespace()
    });
    let trimmed =
        trimmed.trim_end_matches(|character: char| character == '.' || character.is_whitespace());
    if trimmed.is_empty() || trimmed.chars().all(|character| character == '-') {
        return "Preset".into();
    }
    if windows_reserved(trimmed) {
        return format!("_{trimmed}");
    }
    trimmed.to_owned()
}

/// Resolves `<stem>.t3kpreset` or the first free ` 2`, ` 3`, … suffix.
/// `self_path` never counts as a collision, permitting case-only renames.
#[must_use]
pub fn unique_preset_path(dir: &Path, stem: &str, self_path: Option<&Path>) -> PathBuf {
    let taken = |candidate: &Path| candidate.exists() && self_path != Some(candidate);
    let mut candidate = dir.join(format!("{stem}{PRESET_EXTENSION}"));
    let mut number = 2_usize;
    while taken(&candidate) {
        candidate = dir.join(format!("{stem} {number}{PRESET_EXTENSION}"));
        number += 1;
    }
    candidate
}

fn forbidden(character: char) -> bool {
    (character as u32) < 0x20
        || character == '\u{7f}'
        || matches!(
            character,
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
        )
}

fn windows_reserved(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or_default().trim_end();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    RESERVED.iter().any(|name| base.eq_ignore_ascii_case(name))
}

fn read_i32(input: &[u8], offset: usize) -> Result<i32, PresetError> {
    let bytes = input
        .get(offset..offset + 4)
        .ok_or(PresetError::InvalidHeader)?;
    Ok(i32::from_le_bytes(
        bytes.try_into().map_err(|_| PresetError::InvalidHeader)?,
    ))
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
