//! Focus, accessible metadata, announcements, and host-key tests.

use ronie_ui::accessibility::{AccessibleNode, AccessibleRole, Announcer, FocusManager, HostKey};

fn id(value: &str) -> String {
    value.to_owned()
}

#[test]
fn nothing_is_focused_until_tab_or_shift_tab_enters_the_order() {
    let mut focus = FocusManager::default();
    assert_eq!(focus.focused(), None);
    let order = vec![id("preset"), id("save"), id("add")];
    assert!(focus.tab(&order, false));
    assert_eq!(focus.focused(), Some(&id("preset")));
    assert!(focus.tab(&order, false));
    assert_eq!(focus.focused(), Some(&id("save")));
    assert!(focus.tab(&order, true));
    assert_eq!(focus.focused(), Some(&id("preset")));
    focus.clear();
    assert!(focus.tab(&order, true));
    assert_eq!(focus.focused(), Some(&id("add")));
}

#[test]
fn escape_and_unrelated_pointer_work_dismiss_without_a_default_fallback() {
    let mut focus = FocusManager::default();
    focus.focus(id("editor"));
    assert!(focus.escape());
    assert_eq!(focus.focused(), None);
    assert!(!focus.escape());

    focus.focus(id("editor"));
    focus.pointer_down(false);
    assert_eq!(focus.focused(), None);
    focus.focus(id("editor"));
    focus.pointer_down(true);
    assert_eq!(focus.focused(), Some(&id("editor")));
    focus.remove(&id("editor"));
    assert_eq!(focus.focused(), None);
}

#[test]
fn space_and_enter_pass_to_the_host_only_without_a_focused_control() {
    let mut focus = FocusManager::default();
    assert!(focus.forward_key(HostKey::Space));
    assert!(focus.forward_key(HostKey::Enter));
    focus.focus(id("preset"));
    assert!(!focus.forward_key(HostKey::Space));
    assert!(!focus.forward_key(HostKey::Enter));
    focus.clear();
    assert!(focus.forward_key(HostKey::Space));
}

#[test]
fn accessible_names_roles_values_and_announcements_are_preserved() {
    let slider =
        AccessibleNode::new(id("gain"), AccessibleRole::Slider, "Input gain").with_value("0.5");
    assert_eq!(slider.name, "Input gain");
    assert_eq!(slider.value.as_deref(), Some("0.5"));
    assert!(slider.validate().is_ok());
    let unnamed = AccessibleNode::new(id("unknown"), AccessibleRole::Button, "");
    assert!(unnamed.validate().is_err());

    let mut announcements = Announcer::default();
    announcements.announce("Audio device disconnected");
    announcements.announce("Audio device disconnected");
    announcements.announce("Reconnected");
    assert_eq!(
        announcements.drain(),
        vec!["Audio device disconnected", "Reconnected"]
    );
}
