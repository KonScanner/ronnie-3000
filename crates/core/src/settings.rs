use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use fs2::FileExt;

use crate::{PropertiesError, PropertiesFile};

/// Machine-preferences filename shared with existing installations.
pub const PREFERENCES_SETTINGS: &str = "preferences.settings";
/// UI-preferences filename shared with existing installations.
pub const UI_PREFERENCES_SETTINGS: &str = "ui-preferences.settings";

/// File-backed settings with same-file lock/reload/merge/atomic-write behavior.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
    root: Option<PathBuf>,
}

impl SettingsStore {
    /// Selects one settings file.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            root: None,
        }
    }

    /// Selects the machine or UI filename under a private configuration root.
    #[must_use]
    pub fn in_root(root: &Path, filename: &str) -> Self {
        Self {
            path: root.join(filename),
            root: Some(root.to_path_buf()),
        }
    }

    /// Selects the standalone settings filename supplied by the private ledger.
    #[must_use]
    pub fn standalone(root: &Path, filename: &str) -> Self {
        Self::in_root(root, filename)
    }

    /// Returns the selected settings file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reloads the complete current document.
    ///
    /// # Errors
    ///
    /// Returns I/O or malformed-XML errors.
    pub fn read(&self) -> Result<PropertiesFile, PropertiesError> {
        self.validate()?;
        PropertiesFile::read(&self.path)
    }

    /// Alias for an explicit cross-process refresh.
    ///
    /// # Errors
    ///
    /// Returns I/O or malformed-XML errors.
    pub fn sync(&self) -> Result<PropertiesFile, PropertiesError> {
        self.read()
    }

    /// Merges exactly one key under an inter-process lock and writes once.
    ///
    /// # Errors
    ///
    /// Returns lock, read, or atomic-write errors without replacing the previous file on failure.
    pub fn merge_key(&self, name: &str, value: &str) -> Result<(), PropertiesError> {
        let _guard = self.lock()?;
        let mut properties = self.read()?;
        properties.set(name, value);
        self.write(&properties)
    }

    /// Removes exactly one key under the same merge discipline.
    ///
    /// # Errors
    ///
    /// Returns lock, read, or atomic-write errors.
    pub fn remove_key(&self, name: &str) -> Result<(), PropertiesError> {
        let _guard = self.lock()?;
        let mut properties = self.read()?;
        properties.remove(name);
        self.write(&properties)
    }

    /// Changes one key only when its current serialized value still matches `expected`.
    ///
    /// # Errors
    ///
    /// Returns lock, read, or atomic-write errors. A mismatch returns `Ok(false)` without writing.
    pub fn compare_and_swap_key(
        &self,
        name: &str,
        expected: Option<&str>,
        value: Option<&str>,
    ) -> Result<bool, PropertiesError> {
        let _guard = self.lock()?;
        let mut properties = self.read()?;
        if properties.get(name) != expected {
            return Ok(false);
        }
        match value {
            Some(value) => properties.set(name, value),
            None => properties.remove(name),
        }
        self.write(&properties)?;
        Ok(true)
    }

    fn validate(&self) -> Result<(), PropertiesError> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let relative = self
            .path
            .strip_prefix(root)
            .map_err(|_| PropertiesError::UnsafePath)?;
        let mut components = relative.components();
        match (components.next(), components.next()) {
            (Some(std::path::Component::Normal(_)), None) => Ok(()),
            _ => Err(PropertiesError::UnsafePath),
        }
    }

    fn lock(&self) -> Result<SettingsLock, PropertiesError> {
        self.validate()?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let path = self.path.with_extension("lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.lock_exclusive()?;
        Ok(SettingsLock(file))
    }

    fn write(&self, properties: &PropertiesFile) -> Result<(), PropertiesError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(properties.to_xml().as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| PropertiesError::Io(error.error))?;
        Ok(())
    }
}

struct SettingsLock(fs::File);

impl Drop for SettingsLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
