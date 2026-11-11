//! The diagnostic log of the standalone app: one small text file for a bug report.
//!
//! A frame can report an event, and a frame must do no file work, so that one thread owns the
//! file. A message holds what the audio engine knows about its stream and its chain. The home
//! directory of the user does not reach the file, because the user pastes this text in public.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Name of the log file in its directory.
const LOG_FILE: &str = "ronnie.log";
/// Name of the previous log file.
const OLD_LOG_FILE: &str = "ronnie.old.log";
/// A log that has this size at the start of the app becomes the previous log.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// The end of the log that a copy for a bug report holds.
const MAX_COPY_BYTES: u64 = 64 * 1024;
/// Longest wait for lines that a copy must contain. A click waits, and a frame does not.
const FLUSH_LIMIT: Duration = Duration::from_millis(200);

enum Message {
    Line(String),
    Flush(mpsc::Sender<()>),
}

/// Handle to the log file. Clones write to the same file.
#[derive(Debug, Clone)]
pub struct DiagnosticLog {
    directory: PathBuf,
    sender: mpsc::Sender<Message>,
    home: Option<String>,
}

impl DiagnosticLog {
    /// Starts the log in `directory`, with its first line. The file work runs on its own thread.
    #[must_use]
    pub fn start(directory: &Path) -> Self {
        // Windows names the home folder in USERPROFILE.
        let home = std::env::var_os("HOME").or_else(|| {
            cfg!(windows)
                .then(|| std::env::var_os("USERPROFILE"))
                .flatten()
        });
        Self::with_home(directory, home.and_then(|home| home.into_string().ok()))
    }

    fn with_home(directory: &Path, home: Option<String>) -> Self {
        let (sender, messages) = mpsc::channel();
        let folder = directory.to_path_buf();
        // With no thread, the log stays empty, and the app has more important work to do.
        let _ = std::thread::Builder::new()
            .name("ronie-diagnostic-log".to_owned())
            .spawn(move || write_messages(&folder, &messages));
        let log = Self {
            directory: directory.to_path_buf(),
            sender,
            // A root directory as the home would replace each path separator.
            home: home.filter(|home| home.len() > 1),
        };
        log.write(&format!("Ronnie {} started", env!("CARGO_PKG_VERSION")));
        log
    }

    /// The directory that holds the log file.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Adds one line, and returns at once.
    pub fn write(&self, message: &str) {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let message = match &self.home {
            Some(home) => message.replace(home.as_str(), "~"),
            None => message.to_owned(),
        };
        let one_line = message.replace(['\n', '\r'], " ");
        let _ = self
            .sender
            .send(Message::Line(format!("[{seconds}] {one_line}\n")));
    }

    /// The end of the log, with each line that was written before this call.
    #[must_use]
    pub fn recent(&self) -> Option<String> {
        let (done, flushed) = mpsc::channel();
        if self.sender.send(Message::Flush(done)).is_ok() {
            let _ = flushed.recv_timeout(FLUSH_LIMIT);
        }
        read_tail(&self.directory.join(LOG_FILE), MAX_COPY_BYTES).ok()
    }
}

fn write_messages(directory: &Path, messages: &mpsc::Receiver<Message>) {
    let mut file = open_log(directory);
    while let Ok(message) = messages.recv() {
        match message {
            Message::Line(line) => {
                if let Some(file) = &mut file {
                    // A log has no place to report that it cannot write.
                    let _ = file.write_all(line.as_bytes());
                }
            }
            Message::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn open_log(directory: &Path) -> Option<File> {
    fs::create_dir_all(directory).ok()?;
    let path = directory.join(LOG_FILE);
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() >= MAX_FILE_BYTES) {
        let _ = fs::rename(&path, directory.join(OLD_LOG_FILE));
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Reads complete lines from the end of a file.
fn read_tail(path: &Path, limit: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(limit);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    if start == 0 {
        return Ok(text.into_owned());
    }
    // The first line starts before the limit, so that only its end is here.
    Ok(text
        .split_once('\n')
        .map_or_else(String::new, |(_, rest)| rest.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_starts_with_a_line_and_holds_what_it_was_told() {
        let directory = tempfile::tempdir().expect("directory");
        let log = DiagnosticLog::with_home(&directory.path().join("logs"), None);
        log.write("first event");
        log.write("second event\nwith two lines");

        let text = log.recent().expect("log");

        assert!(text.contains(&format!("Ronnie {} started", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains("first event\n"));
        assert!(text.contains("second event with two lines\n"));
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn the_home_directory_does_not_reach_the_file() {
        let directory = tempfile::tempdir().expect("directory");
        let log = DiagnosticLog::with_home(directory.path(), Some("/home/someone".to_owned()));
        log.write("could not read /home/someone/.config/file");

        let text = log.recent().expect("log");

        assert!(text.contains("could not read ~/.config/file"), "{text}");
        assert!(!text.contains("someone"), "{text}");
    }

    #[test]
    fn a_second_start_adds_to_the_same_file() {
        let directory = tempfile::tempdir().expect("directory");
        let first = DiagnosticLog::with_home(directory.path(), None);
        first.write("from the first run");
        let _ = first.recent();
        drop(first);

        let second = DiagnosticLog::with_home(directory.path(), None);
        let text = second.recent().expect("log");

        assert!(text.contains("from the first run"), "{text}");
        assert_eq!(text.matches("started").count(), 2, "{text}");
    }

    #[test]
    fn a_large_log_moves_aside_at_the_start() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join(LOG_FILE);
        fs::write(&path, vec![b'x'; MAX_FILE_BYTES as usize]).expect("large log");

        let log = DiagnosticLog::with_home(directory.path(), None);
        let text = log.recent().expect("log");

        assert_eq!(text.lines().count(), 1, "{text}");
        assert_eq!(
            fs::metadata(directory.path().join(OLD_LOG_FILE))
                .expect("old log")
                .len(),
            MAX_FILE_BYTES
        );
    }

    #[test]
    fn a_tail_holds_only_complete_lines() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("tail");
        fs::write(&path, "first line\nsecond line\nthird\n").expect("file");

        assert_eq!(
            read_tail(&path, 100).expect("all"),
            "first line\nsecond line\nthird\n"
        );
        assert_eq!(read_tail(&path, 15).expect("end"), "third\n");
        assert_eq!(read_tail(&path, 3).expect("no whole line"), "");
    }

    #[test]
    fn a_log_with_no_place_to_write_stays_quiet() {
        let directory = tempfile::tempdir().expect("directory");
        let blocker = directory.path().join("file");
        fs::write(&blocker, "not a directory").expect("file");

        let log = DiagnosticLog::with_home(&blocker.join("logs"), None);
        log.write("nowhere to go");

        assert_eq!(log.recent(), None);
    }
}
