use std::{
    cmp::Ordering,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use percent_encoding::percent_decode_str;
use serde_json::{Value, json};
use url::Url;

use crate::{
    DspResult, LocalError, LocalStash, ModelId, ModelKind, NamDocument, StashedModel, Tone,
    decode_wav, parse_nam,
};

const MAX_LOCAL_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_FOLDER_MODELS: usize = 300;
const MAX_DISCOVERY_DEPTH: usize = 64;
const MAX_DISCOVERY_ENTRIES: usize = 10_000;
const SHORT_IR_MAX_SECONDS: f64 = 1.0;

/// One discovered input file and its selected model family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalInput {
    /// Canonical path under the selected root.
    pub path: PathBuf,
    /// UTF-8 filename without parent directories.
    pub name: String,
    /// Lowercase-extension model family.
    pub kind: ModelKind,
}

/// Validated metadata used to synthesize a local tone model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModelInspection {
    /// Positive content-derived model identity.
    pub id: ModelId,
    /// Display name with the final extension removed.
    pub name: String,
    /// Model family.
    pub kind: ModelKind,
    /// Content-addressed stash filename.
    pub stash_name: String,
    /// Best-effort gear identifier.
    pub gear: Option<String>,
    /// `.nam` metadata `gear_make`, when present.
    pub gear_make: Option<String>,
    /// `.nam` metadata `gear_model`, when present.
    pub gear_model: Option<String>,
    /// `.nam` metadata `tone_type`, when present.
    pub tone_type: Option<String>,
}

/// Successful local tone load with a count of rejected files.
#[derive(Debug, Clone)]
pub struct LocalLoad {
    /// Synthetic local tone retaining every surviving model.
    pub tone: Tone,
    /// Stashed model files in tone order.
    pub stashed: Vec<StashedModel>,
    /// Number of files rejected while later files still loaded.
    pub skipped: usize,
}

/// Discovers one file or a recursive folder without following symbolic links.
/// Folder loading selects NAM on extension-count ties and sorts by natural filename.
///
/// # Errors
///
/// Returns an error for unsupported or missing paths, more than 300 selected
/// files, files over 50 MiB, excessive traversal, and filesystem failures.
pub fn discover_local(path: &Path) -> Result<Vec<LocalInput>, LocalError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(LocalError::UnsafePath(
            "symbolic-link roots are not followed",
        ));
    }
    if metadata.is_file() {
        let name = utf8_file_name(path)?;
        let kind = extension_from_name(&name).ok_or(LocalError::UnsupportedExtension)?;
        if metadata.len() > MAX_LOCAL_FILE_BYTES as u64 {
            return Err(LocalError::InputTooLarge);
        }
        return Ok(vec![LocalInput {
            path: fs::canonicalize(path)?,
            name,
            kind,
        }]);
    }
    if !metadata.is_dir() {
        return Err(LocalError::UnsafePath(
            "local input must be a file or directory",
        ));
    }

    let root = fs::canonicalize(path)?;
    let mut nams = Vec::new();
    let mut wavs = Vec::new();
    let mut visited = 0_usize;
    discover_directory(&root, 0, &mut visited, &mut nams, &mut wavs)?;
    let mut picked = if nams.len() >= wavs.len() { nams } else { wavs };
    if picked.is_empty() {
        return Err(LocalError::NoSupportedFiles);
    }
    if picked.len() > MAX_FOLDER_MODELS {
        return Err(LocalError::TooManyFiles {
            actual: picked.len(),
            max: MAX_FOLDER_MODELS,
        });
    }
    for input in &picked {
        let size = fs::metadata(&input.path)?.len();
        if size > MAX_LOCAL_FILE_BYTES as u64 {
            return Err(LocalError::InputTooLarge);
        }
    }
    picked.sort_by(|left, right| natural_compare(&left.name, &right.name));
    Ok(picked)
}

/// Validates a model and computes its local identity and stash name.
/// Core parses the document; the supplied callback owns the complete A2 gate.
///
/// # Errors
///
/// Returns an error for empty/oversized input, unsupported extensions,
/// malformed NAM/WAV data, or validator rejection.
pub fn inspect_local_model(
    name: &str,
    bytes: &[u8],
    validate_nam: &dyn Fn(&NamDocument) -> DspResult<()>,
) -> Result<LocalModelInspection, LocalError> {
    if bytes.is_empty() {
        return Err(LocalError::EmptyInput);
    }
    if bytes.len() > MAX_LOCAL_FILE_BYTES {
        return Err(LocalError::InputTooLarge);
    }
    let kind = extension_from_name(name).ok_or(LocalError::UnsupportedExtension)?;
    let mut gear_make = None;
    let mut gear_model = None;
    let mut tone_type = None;
    let gear = match kind {
        ModelKind::Nam => {
            let document = parse_nam(bytes)?;
            // NAM writes `metadata` at the top level; older files put it under `config`.
            let field = |key: &str| {
                document
                    .metadata
                    .extra
                    .get(key)
                    .or_else(|| {
                        document
                            .config
                            .get("metadata")
                            .and_then(|meta| meta.get(key))
                    })
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            };
            let gear = field("gear_type")
                .as_deref()
                .and_then(gear_from_nam_type)
                .map(str::to_owned);
            gear_make = field("gear_make");
            gear_model = field("gear_model");
            tone_type = field("tone_type");
            validate_nam(&document)?;
            gear
        }
        ModelKind::Ir => {
            let audio = decode_wav(bytes)?;
            let seconds = audio
                .channels
                .first()
                .map_or(0.0, |channel| channel.len() as f64 / audio.sample_rate);
            (seconds <= SHORT_IR_MAX_SECONDS).then(|| "cab".to_owned())
        }
    };
    let hash = fnv1a64(bytes);
    let model_name = name
        .rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_owned();
    Ok(LocalModelInspection {
        id: ModelId::from_local_bytes(bytes),
        name: model_name,
        kind,
        stash_name: format!("{hash:x}-{}.{}", bytes.len(), kind.as_str()),
        gear,
        gear_make,
        gear_model,
        tone_type,
    })
}

/// Discovers, validates, and atomically stashes every surviving local model.
/// Folder failures are skipped until at least one valid model remains; the first
/// error is returned when no model survives.
///
/// # Errors
///
/// Returns discovery or first per-file validation/stash errors, and tone synthesis errors.
pub fn load_local_path(
    path: &Path,
    stash: &LocalStash,
    validate_nam: &dyn Fn(&NamDocument) -> DspResult<()>,
) -> Result<LocalLoad, LocalError> {
    let root = if path.is_dir() {
        fs::canonicalize(path)?
    } else {
        fs::canonicalize(
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )?
    };
    let root_dir = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())?;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let opened = fs::read_link(format!("/proc/self/fd/{}", root_dir.as_raw_fd()))?;
        if opened != root {
            return Err(LocalError::UnsafePath(
                "local root changed before it could be opened",
            ));
        }
    }
    let inputs = discover_local(path)?;
    let mut inspections = Vec::new();
    let mut stashed = Vec::new();
    let mut first_error = None;
    for input in &inputs {
        let relative = if path.is_dir() {
            input
                .path
                .strip_prefix(&root)
                .map_err(|_| LocalError::UnsafePath("local input escapes its root"))?
        } else {
            Path::new(&input.name)
        };
        let result = read_bounded(&root_dir, relative).and_then(|bytes| {
            let inspection = inspect_local_model(&input.name, &bytes, validate_nam)?;
            let stored = stash.stash(&input.name, &bytes)?;
            Ok((inspection, stored))
        });
        match result {
            Ok((inspection, stored)) => {
                inspections.push(inspection);
                stashed.push(stored);
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    if inspections.is_empty() {
        return Err(first_error.unwrap_or(LocalError::NoSupportedFiles));
    }

    let mut models = Vec::with_capacity(inspections.len());
    for (inspection, stored) in inspections.iter().zip(&stashed) {
        let url = Url::from_file_path(&stored.path).map_err(|_| {
            LocalError::UnsafePath("stash path cannot be represented as a file URL")
        })?;
        let mut model = json!({
            "id": inspection.id.0,
            "name": inspection.name,
            "model_url": url.as_str(),
        });
        if let Some(gear) = &inspection.gear {
            model["gear"] = Value::String(gear.clone());
        }
        for (key, value) in [
            ("gear_make", &inspection.gear_make),
            ("gear_model", &inspection.gear_model),
            ("tone_type", &inspection.tone_type),
        ] {
            if let Some(value) = value {
                model[key] = Value::String(value.clone());
            }
        }
        models.push(model);
    }
    let title = if path.is_dir() {
        utf8_file_name(path)?
    } else {
        utf8_file_name(path)?
            .rsplit_once('.')
            .map_or_else(|| utf8_file_name(path), |(stem, _)| Ok(stem.to_owned()))?
    };
    let first_kind = inspections[0].kind;
    let tone = Tone::from_value(json!({
        "id": 0,
        "local": true,
        "title": title,
        "format": first_kind.as_str(),
        "models": models,
    }))?;
    Ok(LocalLoad {
        tone,
        stashed,
        skipped: inputs.len() - inspections.len(),
    })
}

/// Returns the decoded final filename component of a local or remote URL.
///
/// # Errors
///
/// Returns an error for malformed URLs, non-UTF-8 paths, or a missing filename.
pub fn local_file_name_from_url(raw_url: &str) -> Result<String, LocalError> {
    let url = Url::parse(raw_url)?;
    if url.scheme() == "file" {
        let path = url
            .to_file_path()
            .map_err(|_| LocalError::UnsafePath("file URL cannot be converted to a path"))?;
        return utf8_file_name(&path);
    }
    let encoded = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .ok_or(LocalError::UnsafePath("URL has no filename"))?;
    percent_decode_str(encoded)
        .decode_utf8()
        .map(|name| name.into_owned())
        .map_err(|_| LocalError::UnsafePath("URL filename is not UTF-8"))
}

pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 1_469_598_103_934_665_603_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

pub(crate) fn extension_from_name(name: &str) -> Option<ModelKind> {
    match name.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
        "nam" => Some(ModelKind::Nam),
        "wav" => Some(ModelKind::Ir),
        _ => None,
    }
}

fn discover_directory(
    root: &Path,
    depth: usize,
    visited: &mut usize,
    nams: &mut Vec<LocalInput>,
    wavs: &mut Vec<LocalInput>,
) -> Result<(), LocalError> {
    if depth > MAX_DISCOVERY_DEPTH {
        return Err(LocalError::UnsafePath(
            "directory nesting exceeds the discovery limit",
        ));
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        *visited = visited.checked_add(1).ok_or(LocalError::TooManyFiles {
            actual: MAX_DISCOVERY_ENTRIES,
            max: MAX_DISCOVERY_ENTRIES,
        })?;
        if *visited > MAX_DISCOVERY_ENTRIES {
            return Err(LocalError::TooManyFiles {
                actual: *visited,
                max: MAX_DISCOVERY_ENTRIES,
            });
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            discover_directory(&path, depth + 1, visited, nams, wavs)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = utf8_file_name(&path)?;
        match extension_from_name(&name) {
            Some(ModelKind::Nam) => nams.push(LocalInput {
                path,
                name,
                kind: ModelKind::Nam,
            }),
            Some(ModelKind::Ir) => wavs.push(LocalInput {
                path,
                name,
                kind: ModelKind::Ir,
            }),
            None => {}
        }
    }
    Ok(())
}

fn read_bounded(root: &cap_std::fs::Dir, path: &Path) -> Result<Vec<u8>, LocalError> {
    let file = root.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(LocalError::UnsafePath("local input is not a regular file"));
    }
    if metadata.len() > MAX_LOCAL_FILE_BYTES as u64 {
        return Err(LocalError::InputTooLarge);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(metadata.len() as usize)
        .map_err(|_| LocalError::InputTooLarge)?;
    file.take(MAX_LOCAL_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_LOCAL_FILE_BYTES {
        return Err(LocalError::InputTooLarge);
    }
    Ok(bytes)
}

fn utf8_file_name(path: &Path) -> Result<String, LocalError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or(LocalError::UnsafePath("path needs a UTF-8 filename"))
}

fn natural_compare(left: &str, right: &str) -> Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut a = 0_usize;
    let mut b = 0_usize;
    while a < left.len() && b < right.len() {
        if left[a].is_ascii_digit() && right[b].is_ascii_digit() {
            let a_end = digits_end(left, a);
            let b_end = digits_end(right, b);
            let a_trim = trim_zeroes(&left[a..a_end]);
            let b_trim = trim_zeroes(&right[b..b_end]);
            let ordering = a_trim
                .len()
                .cmp(&b_trim.len())
                .then_with(|| a_trim.cmp(b_trim))
                .then_with(|| (a_end - a).cmp(&(b_end - b)));
            if ordering != Ordering::Equal {
                return ordering;
            }
            a = a_end;
            b = b_end;
            continue;
        }
        let ordering = left[a]
            .to_ascii_lowercase()
            .cmp(&right[b].to_ascii_lowercase());
        if ordering != Ordering::Equal {
            return ordering;
        }
        a += 1;
        b += 1;
    }
    (left.len() - a)
        .cmp(&(right.len() - b))
        .then_with(|| left.cmp(right))
}

fn digits_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    end
}

fn trim_zeroes(bytes: &[u8]) -> &[u8] {
    let first = bytes
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(bytes.len() - 1);
    &bytes[first..]
}

fn gear_from_nam_type(value: &str) -> Option<&str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "amp" | "pedal_amp" | "preamp" => Some("amp"),
        "amp_cab" | "amp_pedal_cab" | "amp-cab" => Some("amp-cab"),
        "studio" | "outboard" => Some("outboard"),
        "pedal" => Some("pedal"),
        "cab" => Some("cab"),
        _ => None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn anchored_reads_reject_parent_symlink_escape() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).expect("root");
        fs::create_dir(&outside).expect("outside");
        fs::write(outside.join("model.nam"), b"outside").expect("outside model");
        fs::create_dir(root.join("input")).expect("input");
        fs::rename(root.join("input"), root.join("real-input")).expect("rename directory");
        std::os::unix::fs::symlink(&outside, root.join("input")).expect("parent symlink");
        let root = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())
            .expect("root capability");

        let result = read_bounded(&root, Path::new("input/model.nam"));

        assert!(result.is_err());
    }
}
