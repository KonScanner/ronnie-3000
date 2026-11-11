//! The key that the user saves in the app: storage, permissions, removal, and its place in the
//! order of the service values. The tests use a fake key and contact no service.

use ronie_app::auth::{UserKeySetup, UserKeyStore, configured_community_with_launch};
use ronie_core::{SettingsStore, UI_PREFERENCES_SETTINGS};
use ronie_ui::services::key_setup::{KeySetup, KeySetupError};

const KEY: &str = "t3k_pub_test_fake";
const OTHER_KEY: &str = "t3k_pub_other_fake";

#[test]
fn a_key_is_saved_read_replaced_and_removed() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let store = UserKeyStore::new(directory.path());
    assert_eq!(store.read().expect("read"), None);

    store.save(KEY).expect("save");
    assert_eq!(store.read().expect("read").as_deref(), Some(KEY));
    store.save(OTHER_KEY).expect("replace");
    assert_eq!(store.read().expect("read").as_deref(), Some(OTHER_KEY));

    store.remove().expect("remove");
    assert_eq!(store.read().expect("read"), None);
    store.remove().expect("a second removal does nothing");
}

#[test]
fn a_value_that_is_not_a_key_is_not_saved_and_the_error_does_not_hold_it() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let store = UserKeyStore::new(directory.path());
    let value = "t3k_pub_private value";

    let error = store.save(value).expect_err("not a key").to_string();

    assert!(!error.contains(value), "{error}");
    assert!(!error.contains("private"), "{error}");
    assert_eq!(store.read().expect("read"), None);
    assert!(
        !directory.path().join(UI_PREFERENCES_SETTINGS).exists(),
        "nothing is written"
    );
}

#[test]
fn a_saved_value_that_is_not_a_key_counts_as_no_key() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    SettingsStore::standalone(directory.path(), UI_PREFERENCES_SETTINGS)
        .merge_key("t3k_publishable_key", "edited by hand")
        .expect("seed");

    assert_eq!(
        UserKeyStore::new(directory.path()).read().expect("read"),
        None
    );
    assert!(!UserKeySetup::new(directory.path()).has_saved_key());
}

#[test]
fn the_key_keeps_the_other_settings_of_the_file() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let settings = SettingsStore::standalone(directory.path(), UI_PREFERENCES_SETTINGS);
    settings.merge_key("showHints", "0").expect("seed");
    let store = UserKeyStore::new(directory.path());

    store.save(KEY).expect("save");
    store.remove().expect("remove");

    assert_eq!(settings.read().expect("read").get("showHints"), Some("0"));
}

#[cfg(unix)]
#[test]
fn only_the_owner_can_read_or_write_the_saved_key() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temporary configuration root");
    let path = directory.path().join(UI_PREFERENCES_SETTINGS);
    // An older file that other users can read gets replaced by a private one.
    SettingsStore::standalone(directory.path(), UI_PREFERENCES_SETTINGS)
        .merge_key("showHints", "1")
        .expect("seed");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");

    UserKeyStore::new(directory.path()).save(KEY).expect("save");

    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "{mode:o}");
}

#[test]
fn the_setup_saves_a_key_and_starts_a_service() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let mut setup = UserKeySetup::new(directory.path());
    assert!(!setup.has_saved_key());
    assert_eq!(setup.origin(), "https://www.tone3000.com");

    let parts = setup.save(KEY).expect("save");

    assert!(parts.is_some(), "a saved key starts a service");
    assert!(setup.has_saved_key());
    setup.remove().expect("remove");
    assert!(!setup.has_saved_key());
}

#[test]
fn the_setup_refuses_a_value_that_is_not_a_key() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    let mut setup = UserKeySetup::new(directory.path());

    let error = setup.save("not-a-key").err().expect("refused");

    assert_eq!(error, KeySetupError::NotAKey);
    assert!(!error.to_string().contains("not-a-key"));
    assert!(!setup.has_saved_key());
}

#[test]
fn a_saved_key_makes_a_service_with_no_launch_values() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    UserKeyStore::new(directory.path()).save(KEY).expect("save");

    let service = configured_community_with_launch(directory.path(), None, None, None)
        .expect("valid configuration");

    assert!(service.is_some());
}

#[test]
fn a_launch_origin_wins_over_the_saved_key() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    UserKeyStore::new(directory.path()).save(KEY).expect("save");

    // The launch origin needs its own key. The saved key never goes to that origin.
    let error =
        configured_community_with_launch(directory.path(), Some("https://api.example"), None, None)
            .err()
            .expect("incomplete launch pair")
            .to_string();

    assert!(error.contains("RONIE_OAUTH_CLIENT_ID"), "{error}");
    assert!(!error.contains(KEY), "{error}");
}

#[test]
fn an_invalid_launch_model_origin_still_stops_a_saved_key() {
    let directory = tempfile::tempdir().expect("temporary configuration root");
    UserKeyStore::new(directory.path()).save(KEY).expect("save");

    let error = configured_community_with_launch(
        directory.path(),
        None,
        None,
        Some("http://models.example"),
    )
    .err()
    .expect("invalid model origin")
    .to_string();

    assert!(error.contains("RONIE_MODEL_ORIGINS"), "{error}");
    assert!(!error.contains(KEY), "{error}");
}
