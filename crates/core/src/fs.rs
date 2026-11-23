use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(crate) fn ensure_writable_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            if directory_has_owner_write(&metadata) {
                return Ok(());
            }
            restore_owner_write(path, &metadata)?;
            let healed = fs::symlink_metadata(path)?;
            if healed.file_type().is_dir() && directory_has_owner_write(&healed) {
                return Ok(());
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    if fs::symlink_metadata(path).is_ok() {
        let aside = nonexistent_unwritable_sibling(path)?;
        fs::rename(path, aside)?;
    }
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir()
        && !metadata.file_type().is_symlink()
        && directory_has_owner_write(&metadata)
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory is not writable",
        ))
    }
}

fn restore_owner_write(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o200))?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = metadata.permissions();
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "this code is not on Unix, and on Windows it only clears the read-only flag"
        )]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn directory_has_owner_write(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o200 != 0
    }
    #[cfg(not(unix))]
    {
        !metadata.permissions().readonly()
    }
}

fn nonexistent_unwritable_sibling(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory needs a UTF-8 filename",
            )
        })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for index in 0..10_000_u32 {
        let suffix = if index == 0 {
            String::new()
        } else {
            format!(".{index}")
        };
        let candidate = parent.join(format!("{name}.unwritable{suffix}"));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no recovery filename is available",
    ))
}
