use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::UNIX_EPOCH,
};

use serde_json::Value;

use crate::{
    JuceTree, JuceValue, PRESET_EXTENSION, PresetError, read_preset_file, read_preset_header_file,
    write_user_preset,
};

const USER_PREFIX: &str = "user:";
const FACTORY_PREFIX: &str = "factory:";

/// Public preset identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetInfo {
    /// Namespaced preset identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Factory section flag.
    pub factory: bool,
}

/// One host program slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProgram {
    /// Preset ID, or `None` for an inert slot.
    pub id: Option<String>,
    /// Program display name, or `(empty)`.
    pub name: String,
}

#[derive(Debug)]
struct Entry {
    info: PresetInfo,
    path: PathBuf,
}

#[derive(Debug, Default)]
struct CachedHeader {
    modification_ms: u64,
    size: u64,
    valid: bool,
    raw_id: String,
    name: String,
}

#[derive(Debug, Default)]
struct StoreCache {
    headers: HashMap<PathBuf, CachedHeader>,
    reads: usize,
}

/// Coherent file-backed preset store with per-instance header caching.
#[derive(Debug)]
pub struct PresetStore {
    user_dir: PathBuf,
    factory_dir: PathBuf,
    system_factory_dir: PathBuf,
    cache: Mutex<StoreCache>,
}

impl PresetStore {
    /// Selects a base user directory and installer-shipped factory directory.
    #[must_use]
    pub fn new(base_dir: impl Into<PathBuf>, system_factory_dir: impl Into<PathBuf>) -> Self {
        let user_dir = base_dir.into();
        Self {
            factory_dir: user_dir.join("Factory"),
            user_dir,
            system_factory_dir: system_factory_dir.into(),
            cache: Mutex::new(StoreCache::default()),
        }
    }

    /// Rescans directories and returns user-before-factory order.
    ///
    /// # Errors
    ///
    /// Returns directory or header read errors. Invalid preset files are skipped and cached.
    pub fn list(&self) -> Result<Vec<PresetInfo>, PresetError> {
        Ok(self
            .entries()?
            .into_iter()
            .map(|entry| entry.info)
            .collect())
    }

    /// Loads the complete body for one preset ID.
    ///
    /// # Errors
    ///
    /// Returns missing-file, framing, or binary codec errors.
    pub fn load(&self, id: &str) -> Result<JuceTree, PresetError> {
        let entry = self
            .entries()?
            .into_iter()
            .find(|entry| entry.info.id == id)
            .ok_or(PresetError::InvalidBody)?;
        read_preset_file(&entry.path)
    }

    /// Saves or updates a user preset with a stable same-name identity.
    ///
    /// # Errors
    ///
    /// Returns directory, framing, or atomic-write errors.
    pub fn save(&self, name: &str, mut preset: JuceTree) -> Result<PresetInfo, PresetError> {
        fs::create_dir_all(&self.user_dir)?;
        let current = self
            .entries()?
            .into_iter()
            .find(|entry| !entry.info.factory && entry.info.name.eq_ignore_ascii_case(name));
        let raw_id = current
            .as_ref()
            .map(|entry| entry.info.id.trim_start_matches(USER_PREFIX).to_owned())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_user_id);
        set_property(&mut preset, "name", JuceValue::String(name.into()));
        set_property(&mut preset, "id", JuceValue::String(raw_id.clone()));
        write_user_preset(
            &self.user_dir,
            current.as_ref().map(|entry| entry.path.as_path()),
            name,
            &preset,
        )?;
        Ok(PresetInfo {
            id: format!("{USER_PREFIX}{raw_id}"),
            name: name.into(),
            factory: false,
        })
    }

    /// Renames a user preset while preserving its ID and unknown body content.
    ///
    /// # Errors
    ///
    /// Returns `Ok(false)` for factory IDs, blank names, or missing presets, and I/O errors otherwise.
    pub fn rename(&self, id: &str, new_name: &str) -> Result<bool, PresetError> {
        let trimmed = new_name.trim();
        if !id.starts_with(USER_PREFIX) || trimmed.is_empty() {
            return Ok(false);
        }
        let Some(entry) = self
            .entries()?
            .into_iter()
            .find(|entry| entry.info.id == id)
        else {
            return Ok(false);
        };
        let mut preset = read_preset_file(&entry.path)?;
        set_property(&mut preset, "name", JuceValue::String(trimmed.into()));
        set_property(
            &mut preset,
            "id",
            JuceValue::String(id.trim_start_matches(USER_PREFIX).into()),
        );
        write_user_preset(&self.user_dir, Some(&entry.path), trimmed, &preset)?;
        Ok(true)
    }

    /// Removes a user preset, refusing every factory ID.
    ///
    /// # Errors
    ///
    /// Returns filesystem errors.
    pub fn remove(&self, id: &str) -> Result<bool, PresetError> {
        if !id.starts_with(USER_PREFIX) {
            return Ok(false);
        }
        let Some(entry) = self
            .entries()?
            .into_iter()
            .find(|entry| entry.info.id == id)
        else {
            return Ok(false);
        };
        fs::remove_file(entry.path)?;
        Ok(true)
    }

    /// Moves a preset within its user/factory section and persists the complete order.
    ///
    /// # Errors
    ///
    /// Returns order-read or atomic-write errors.
    pub fn move_by(&self, id: &str, delta: i32) -> Result<bool, PresetError> {
        if delta == 0 {
            return Ok(false);
        }
        let presets = self.list()?;
        let Some(index) = presets.iter().position(|info| info.id == id) else {
            return Ok(false);
        };
        let factory = presets[index].factory;
        let mut section_start = index;
        while section_start > 0 && presets[section_start - 1].factory == factory {
            section_start -= 1;
        }
        let mut section_end = index + 1;
        while section_end < presets.len() && presets[section_end].factory == factory {
            section_end += 1;
        }
        let target = (index as i64 + i64::from(delta))
            .clamp(section_start as i64, section_end as i64 - 1) as usize;
        if target == index {
            return Ok(false);
        }
        let mut ids: Vec<_> = presets.into_iter().map(|info| info.id).collect();
        let moving = ids.remove(index);
        ids.insert(target, moving);
        self.write_order(&ids)?;
        Ok(true)
    }

    /// Number of actual header parses, used to prove warm scans do not read bodies.
    #[must_use]
    pub fn header_reads(&self) -> usize {
        self.lock().reads
    }

    /// Returns the exact ordered 128-slot host program list.
    #[must_use]
    pub fn host_programs(&self) -> [HostProgram; 128] {
        let presets = self.list().unwrap_or_default();
        std::array::from_fn(|index| {
            presets.get(index).map_or(
                HostProgram {
                    id: None,
                    name: "(empty)".into(),
                },
                |info| HostProgram {
                    id: Some(info.id.clone()),
                    name: info.name.clone(),
                },
            )
        })
    }

    /// Selects a host program, returning `None` for an inert slot or active no-op.
    ///
    /// # Errors
    ///
    /// Returns directory or header read errors.
    pub fn select_host_program(
        &self,
        index: usize,
        active_id: &str,
    ) -> Result<Option<PresetInfo>, PresetError> {
        let presets = self.list()?;
        let Some(info) = presets.get(index) else {
            return Ok(None);
        };
        if info.id == active_id {
            return Ok(None);
        }
        Ok(Some(info.clone()))
    }

    /// Returns the active program index, or zero when absent/out of range.
    #[must_use]
    pub fn current_program(&self, active_id: &str) -> usize {
        self.list()
            .unwrap_or_default()
            .iter()
            .position(|info| info.id == active_id)
            .unwrap_or(0)
    }

    fn entries(&self) -> Result<Vec<Entry>, PresetError> {
        let mut cache = self.lock();
        let mut seen = HashSet::new();
        let mut factory = self.scan_dir(&self.system_factory_dir, true, &mut seen, &mut cache)?;
        for local in self.scan_dir(&self.factory_dir, true, &mut seen, &mut cache)? {
            if let Some(existing) = factory
                .iter_mut()
                .find(|entry| entry.info.id == local.info.id)
            {
                *existing = local;
            } else {
                factory.push(local);
            }
        }
        factory.sort_by(|left, right| {
            left.info
                .name
                .to_lowercase()
                .cmp(&right.info.name.to_lowercase())
        });
        let mut presets = self.scan_dir(&self.user_dir, false, &mut seen, &mut cache)?;
        presets.extend(factory);
        cache.headers.retain(|path, _| seen.contains(path));
        let order = self.read_order()?;
        if !order.is_empty() {
            presets.sort_by(|left, right| {
                let section = left.info.factory.cmp(&right.info.factory);
                let rank = |id: &str| {
                    order
                        .iter()
                        .position(|entry| entry == id)
                        .unwrap_or(usize::MAX)
                };
                section.then_with(|| rank(&left.info.id).cmp(&rank(&right.info.id)))
            });
        }
        Ok(presets)
    }

    fn scan_dir(
        &self,
        dir: &Path,
        factory: bool,
        seen: &mut HashSet<PathBuf>,
        cache: &mut StoreCache,
    ) -> Result<Vec<Entry>, PresetError> {
        let prefix = if factory { FACTORY_PREFIX } else { USER_PREFIX };
        let mut paths = Vec::new();
        match fs::read_dir(dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let path = entry.path();
                    if entry.file_type()?.is_file()
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                name.to_ascii_lowercase().ends_with(PRESET_EXTENSION)
                            })
                    {
                        paths.push(path);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(PresetError::Io(error)),
        }
        paths.sort_by_key(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_lowercase()
        });
        let mut output = Vec::new();
        let mut ids = HashSet::new();
        for path in paths {
            seen.insert(path.clone());
            let metadata = fs::metadata(&path)?;
            let modification_ms = metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis() as u64);
            let size = metadata.len();
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned();
            let cached = cache.headers.entry(path.clone()).or_default();
            if cached.modification_ms != modification_ms || cached.size != size {
                cache.reads += 1;
                let header = read_preset_header_file(&path);
                cached.modification_ms = modification_ms;
                cached.size = size;
                cached.valid = header.is_ok();
                if let Ok(header) = header {
                    cached.raw_id = if header.id.is_empty() {
                        stem.clone()
                    } else {
                        header.id
                    };
                    cached.name = if header.name.is_empty() {
                        stem.clone()
                    } else {
                        header.name
                    };
                } else {
                    cached.raw_id.clear();
                    cached.name.clear();
                }
            }
            if !cached.valid {
                continue;
            }
            let mut id = format!("{prefix}{}", cached.raw_id);
            if !ids.insert(id.clone()) {
                id = format!("{prefix}{stem}");
                if !ids.insert(id.clone()) {
                    continue;
                }
            }
            output.push(Entry {
                info: PresetInfo {
                    id,
                    name: cached.name.clone(),
                    factory,
                },
                path,
            });
        }
        output.sort_by(|left, right| {
            left.info
                .name
                .to_lowercase()
                .cmp(&right.info.name.to_lowercase())
        });
        Ok(output)
    }

    fn read_order(&self) -> Result<Vec<String>, PresetError> {
        let path = self.user_dir.join("order.json");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(PresetError::Io(error)),
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return Ok(Vec::new());
        };
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect())
    }

    fn write_order(&self, ids: &[String]) -> Result<(), PresetError> {
        fs::create_dir_all(&self.user_dir)?;
        let path = self.user_dir.join("order.json");
        let mut temporary = tempfile::NamedTempFile::new_in(&self.user_dir)?;
        temporary.write_all(serde_json::to_string(ids)?.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| PresetError::Io(error.error))?;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, StoreCache> {
        self.cache.lock().unwrap_or_else(|error| error.into_inner())
    }
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
        tree.properties.push((name.into(), value));
    }
}

fn new_user_id() -> String {
    let mut bytes = [0_u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
