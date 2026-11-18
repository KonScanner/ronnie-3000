//! Helpers shared by the UI integration tests.

#![allow(dead_code)]

/// A file URL of an absolute path on this system. A Windows path needs a drive.
pub(crate) fn file_url(path: &str) -> String {
    if cfg!(windows) {
        format!("file:///C:{path}")
    } else {
        format!("file://{path}")
    }
}
