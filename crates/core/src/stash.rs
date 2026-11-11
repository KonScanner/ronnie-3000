use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use url::Url;

use crate::{
    ModelId, StashError,
    fs::ensure_writable_dir,
    local::{extension_from_name, fnv1a64},
};

const MAX_LOCAL_FILE_BYTES: usize = 50 * 1024 * 1024;

/// One content-addressed model stored under the authorized root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashedModel {
    /// Positive local model identity.
    pub id: ModelId,
    /// Lowercase content hash, byte-count suffix, and extension.
    pub stash_name: String,
    /// Path under the current stash root.
    pub path: PathBuf,
}

/// Filesystem-backed local model stash.
#[derive(Debug)]
pub struct LocalStash {
    root: PathBuf,
}

impl LocalStash {
    /// Creates or repairs a stash root without deleting existing evidence.
    ///
    /// # Errors
    ///
    /// Returns filesystem errors when the root cannot be created, healed, or
    /// moved aside to a unique `.unwritable` sibling.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StashError> {
        let root = root.into();
        ensure_writable_dir(&root)?;
        Ok(Self { root })
    }

    /// Validates, atomically stores, and touches one model byte string.
    ///
    /// # Errors
    ///
    /// Returns an error for empty/oversized input, unsupported names, or I/O failure.
    pub fn stash(&self, name: &str, bytes: &[u8]) -> Result<StashedModel, StashError> {
        if bytes.is_empty() {
            return Err(StashError::EmptyInput);
        }
        if bytes.len() > MAX_LOCAL_FILE_BYTES {
            return Err(StashError::InputTooLarge);
        }
        let kind = extension_from_name(name).ok_or(StashError::InvalidName)?;
        let hash = fnv1a64(bytes);
        let stash_name = format!("{hash:x}-{}.{}", bytes.len(), kind.as_str());
        let path = self.root.join(&stash_name);
        if path.exists() {
            self.touch(&path)?;
        } else {
            let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
            temporary.write_all(bytes)?;
            temporary.as_file().sync_all()?;
            match temporary.persist(&path) {
                Ok(_) => {}
                Err(error) if path.exists() => {
                    drop(error.file);
                    self.touch(&path)?;
                }
                Err(error) => return Err(StashError::Io(error.error)),
            }
        }
        Ok(StashedModel {
            id: ModelId::from_local_bytes(bytes),
            stash_name,
            path,
        })
    }

    /// Resolves a file URL, re-rooting its decoded filename under this stash.
    /// Existing paths outside the authorized root are never returned.
    ///
    /// # Errors
    ///
    /// Returns [`StashError::InvalidUrl`] for non-file or malformed URLs and
    /// [`StashError::InvalidName`] for unsafe names or symlinks.
    pub fn resolve(&self, model_url: &str) -> Result<PathBuf, StashError> {
        let url = Url::parse(model_url).map_err(|_| StashError::InvalidUrl)?;
        if url.scheme() != "file" {
            return Err(StashError::InvalidUrl);
        }
        let stored = url.to_file_path().map_err(|_| StashError::InvalidUrl)?;
        let name = stored
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty() && *name != "." && *name != "..")
            .ok_or(StashError::InvalidName)?;
        let candidate = if stored.parent() == Some(self.root.as_path()) {
            stored
        } else {
            self.root.join(name)
        };
        if fs::symlink_metadata(&candidate)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(StashError::InvalidName);
        }
        Ok(candidate)
    }

    /// Re-stamps a direct stash child with the current modification time.
    ///
    /// # Errors
    ///
    /// Returns an error for paths outside the root, symlinks, or filesystem failure.
    pub fn touch(&self, path: &Path) -> Result<(), StashError> {
        if path.parent() != Some(self.root.as_path()) {
            return Err(StashError::InvalidName);
        }
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(StashError::InvalidName);
        }
        let file = fs::File::options().write(true).open(path)?;
        file.set_times(fs::FileTimes::new().set_modified(SystemTime::now()))?;
        Ok(())
    }

    /// Removes files whose modification time is strictly older than the cutoff.
    ///
    /// # Errors
    ///
    /// Returns directory, metadata, or deletion errors.
    pub fn collect(&self, older_than: Duration) -> Result<usize, StashError> {
        self.collect_at(older_than, SystemTime::now())
    }

    /// Deterministic form of [`LocalStash::collect`] for tests and maintenance tools.
    ///
    /// # Errors
    ///
    /// Returns directory, metadata, or deletion errors.
    pub fn collect_at(&self, older_than: Duration, now: SystemTime) -> Result<usize, StashError> {
        let cutoff = now
            .checked_sub(older_than)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut removed = 0_usize;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() || file_type.is_symlink() {
                continue;
            }
            let modified = entry.metadata()?.modified()?;
            if modified < cutoff {
                fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}
