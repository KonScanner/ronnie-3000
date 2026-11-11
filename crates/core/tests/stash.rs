//! Content-addressed stash naming, liveness, and recovery tests.

use std::{
    fs,
    time::{Duration, SystemTime},
};

use ronie_core::{LocalStash, StashError};
use tempfile::TempDir;
use url::Url;

/// A file URL of an absolute path on this system. A Windows path needs a drive.
fn file_url(path: &str) -> String {
    if cfg!(windows) {
        format!("file:///C:{path}")
    } else {
        format!("file://{path}")
    }
}

fn set_mtime(path: &std::path::Path, time: SystemTime) {
    let file = fs::File::options()
        .write(true)
        .open(path)
        .expect("open for times");
    file.set_times(fs::FileTimes::new().set_modified(time))
        .expect("set mtime");
}

#[test]
fn stash_uses_reference_fnv_names_and_deduplicates_bytes() {
    let temp = TempDir::new().expect("temp dir");
    let stash = LocalStash::new(temp.path().join("models")).expect("stash");

    let first = stash.stash("hello.NAM", b"hello").expect("first stash");
    let second = stash.stash("hello.nam", b"hello").expect("dedupe");

    assert_eq!(first.stash_name, "5a0d15131ec7a1-5.nam");
    assert_eq!(first.id.0, 344_390_646);
    assert_eq!(first.path, second.path);
    assert_eq!(fs::read(&first.path).expect("stored bytes"), b"hello");
}

#[test]
fn stash_refreshes_liveness_on_repeated_writes_and_touch() {
    let temp = TempDir::new().expect("temp dir");
    let stash = LocalStash::new(temp.path().join("models")).expect("stash");
    let model = stash.stash("one.wav", b"one").expect("stash");
    let old = SystemTime::now() - Duration::from_secs(60);
    set_mtime(&model.path, old);

    stash.stash("one.wav", b"one").expect("dedupe should touch");
    let refreshed = fs::metadata(&model.path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    assert!(refreshed > old);

    set_mtime(&model.path, old);
    stash.touch(&model.path).expect("touch");
    let touched = fs::metadata(&model.path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    assert!(touched > old);
}

#[test]
fn collection_uses_a_strict_seven_day_cutoff() {
    let temp = TempDir::new().expect("temp dir");
    let stash = LocalStash::new(temp.path().join("models")).expect("stash");
    let now = SystemTime::now();
    let week = Duration::from_secs(7 * 24 * 60 * 60);
    let boundary = stash.stash("boundary.nam", b"boundary").expect("boundary");
    let old = stash.stash("old.nam", b"old").expect("old");
    let recent = stash.stash("recent.nam", b"recent").expect("recent");
    set_mtime(&boundary.path, now - week);
    set_mtime(&old.path, now - week - Duration::from_secs(1));
    set_mtime(&recent.path, now - Duration::from_secs(60));

    let removed = stash.collect_at(week, now).expect("collection");

    assert_eq!(removed, 1);
    assert!(boundary.path.is_file());
    assert!(!old.path.exists());
    assert!(recent.path.is_file());
}

#[test]
fn resolution_reroots_missing_urls_and_never_returns_an_outside_file() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path().join("models");
    let stash = LocalStash::new(&root).expect("stash");
    let live = stash.stash("hash two.nam", b"hello").expect("stash");
    let stale = Url::from_file_path(temp.path().join("old-root").join(&live.stash_name))
        .expect("stale URL");

    assert_eq!(stash.resolve(stale.as_str()).expect("re-root"), live.path);
    assert!(matches!(
        stash.resolve(live.path.to_str().expect("UTF-8 path")),
        Err(StashError::InvalidUrl)
    ));

    let outside = stash
        .resolve(&file_url("/etc/passwd"))
        .expect("outside is re-rooted");
    assert_eq!(outside, root.join("passwd"));
    assert_ne!(outside, std::path::PathBuf::from("/etc/passwd"));
    assert!(matches!(
        stash.resolve("https://fixtures.local/model.nam"),
        Err(StashError::InvalidUrl)
    ));
}

#[test]
fn file_squatting_moves_evidence_aside_without_deleting_it() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path().join("models");
    fs::write(&root, b"old bytes").expect("squatting file");
    fs::write(temp.path().join("models.unwritable"), b"keep me").expect("old evidence");

    let stash = LocalStash::new(&root).expect("recovery should pass");
    stash
        .stash("one.nam", b"one")
        .expect("stash after recovery");

    assert!(root.is_dir());
    assert_eq!(
        fs::read(temp.path().join("models.unwritable")).expect("first evidence"),
        b"keep me"
    );
    assert_eq!(
        fs::read(temp.path().join("models.unwritable.1")).expect("moved evidence"),
        b"old bytes"
    );
}

#[cfg(unix)]
#[test]
fn owner_only_write_bit_is_restored_in_place() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().expect("temp dir");
    let root = temp.path().join("models");
    fs::create_dir(&root).expect("root");
    fs::write(root.join("keep.bin"), b"keep").expect("contents");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).expect("lock root");
    if fs::write(root.join("root-probe"), b"").is_ok() {
        return;
    }

    LocalStash::new(&root).expect("heal root");

    let mode = fs::metadata(&root).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    assert!(root.join("keep.bin").is_file());
    assert!(!temp.path().join("models.unwritable").exists());
}
