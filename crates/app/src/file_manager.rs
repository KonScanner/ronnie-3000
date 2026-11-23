//! Opens a folder that the app owns in the file manager of the desktop.
//!
//! The opener is `xdg-open` on Linux, `open` on macOS, and `explorer` on Windows. A web page goes
//! through the `links` rule of the UI crate and the browser of the system instead.
//!
//! The `links` rule of the UI crate refuses `file:` URLs, because a URL can come from a server. A
//! folder here comes only from the standalone settings directory, and no shell reads it.

use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, Stdio},
};

use ronnie_ui::backend::UiError;

/// Asks the desktop to show one folder.
///
/// This reports only whether the opener started. The desktop reports nothing after that.
///
/// # Errors
/// Returns an error when the opener of the desktop cannot start.
pub fn open_in_file_manager(folder: &Path) -> Result<(), UiError> {
    open_folder_with(OsStr::new(folder_opener(std::env::consts::OS)), folder)
}

/// The program that shows a folder on one operating system, as [`std::env::consts::OS`] names it.
///
/// Each program gets the folder as its one argument. Windows Explorer starts with no console
/// window, because it is a desktop program.
fn folder_opener(os: &str) -> &'static str {
    match os {
        "macos" => "open",
        "windows" => "explorer",
        _ => "xdg-open",
    }
}

fn open_folder_with(program: &OsStr, folder: &Path) -> Result<(), UiError> {
    let mut child = spawn_folder_opener(program, folder)
        .map_err(|error| UiError::Control(format!("starting the file manager: {error}")))?;
    // An opener can run for a long time, and a child that ends must not stay as a zombie.
    let _ = std::thread::Builder::new()
        .name("ronnie-file-manager".to_owned())
        .spawn(move || child.wait());
    Ok(())
}

/// Starts `program` with `folder` as its one argument, and with no shell.
fn spawn_folder_opener(program: &OsStr, folder: &Path) -> std::io::Result<std::process::Child> {
    Command::new(program)
        .arg(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };

    #[test]
    fn each_operating_system_has_its_own_folder_opener() {
        assert_eq!(folder_opener("linux"), "xdg-open");
        assert_eq!(folder_opener("freebsd"), "xdg-open");
        assert_eq!(folder_opener("macos"), "open");
        assert_eq!(folder_opener("windows"), "explorer");
    }

    /// Starts `program` once, and retries while Linux refuses the start with `ETXTBSY`.
    ///
    /// A test thread that forks while this test holds the script open for writing leaves the
    /// child with the write handle until its own exec, and Linux then refuses to start the
    /// script with "Text file busy". New forks cannot get the handle after the write, so one run
    /// that starts proves that later runs also start. The fake scripts have no effect with a
    /// probe argument.
    #[cfg(unix)]
    fn wait_until_executable(program: &OsStr) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match spawn_folder_opener(program, Path::new("probe")) {
                Ok(mut child) => {
                    let _ = child.wait();
                    return;
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("fake opener should start: {error}"),
            }
        }
    }

    /// The fake opener is a shell script, so that this test runs only on a Unix system.
    #[cfg(unix)]
    #[test]
    fn the_opener_gets_the_folder_as_one_argument_with_no_shell() {
        let directory = tempfile::tempdir().expect("directory");
        let record = directory.path().join("record");
        let opener = directory.path().join("fake-opener");
        std::fs::write(
            &opener,
            format!(
                "#!/bin/sh\nprintf '%s|%s' \"$#\" \"$1\" > '{}'\n",
                record.display()
            ),
        )
        .expect("script");
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755))
            .expect("permissions");
        let folder = directory.path().join("a folder; with $(a) shell word");

        // A test thread that forks at the same time can hold the write handle of the script open
        // until its exec, and Linux then refuses to start the script with "Text file busy". One
        // started run proves that every later run starts, so this waits for that run first.
        wait_until_executable(opener.as_os_str());
        open_folder_with(opener.as_os_str(), &folder).expect("start");

        let deadline = Instant::now() + Duration::from_secs(20);

        let text = loop {
            match std::fs::read_to_string(&record) {
                Ok(text) if text.ends_with("shell word") => break text,
                _ => {
                    assert!(Instant::now() < deadline, "the opener did not run");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        assert_eq!(text, format!("1|{}", folder.display()));
    }

    #[test]
    fn an_opener_that_cannot_start_is_an_error() {
        let directory = tempfile::tempdir().expect("directory");
        let missing = directory.path().join("no-such-program");
        assert!(matches!(
            open_folder_with(missing.as_os_str(), directory.path()),
            Err(UiError::Control(_))
        ));
    }
}
