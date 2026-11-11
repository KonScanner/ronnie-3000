//! Common wheel, keyboard, focus-reveal, and drag-pan scrolling tests.

use ronie_ui::scroller::{Axis, ClickOutcome, ScrollKey, Scroller};

fn scroller() -> Scroller {
    Scroller::new(Axis::Vertical, 200, 1_000)
}

#[test]
fn arrows_pages_and_end_keys_scroll_like_a_browser() {
    let mut scroll = scroller();
    assert!(scroll.scroll_by_key(ScrollKey::Forward));
    assert_eq!(scroll.position(), 40);
    assert!(scroll.scroll_by_key(ScrollKey::PageDown));
    assert_eq!(scroll.position(), 200);
    assert!(scroll.scroll_by_key(ScrollKey::Home));
    assert_eq!(scroll.position(), 0);
    assert!(scroll.scroll_by_key(ScrollKey::End));
    assert_eq!(scroll.position(), 800);
    assert!(scroll.scroll_by_key(ScrollKey::Back));
    assert_eq!(scroll.position(), 760);
    assert!(scroll.scroll_by_key(ScrollKey::PageUp));
    assert_eq!(scroll.position(), 600);
}

#[test]
fn horizontal_wheel_uses_dominant_axis_and_carries_fractions() {
    let mut scroll = Scroller::new(Axis::Horizontal, 100, 500);
    assert!(!scroll.wheel(0.0001, -0.0025, false));
    assert_eq!(scroll.position(), 0);
    assert!(scroll.wheel(0.0001, -0.0025, false));
    assert_eq!(scroll.position(), 1);
    assert!(!scroll.wheel(0.0, 1.0, true));
    assert_eq!(scroll.position(), 1);
    assert!(!Scroller::new(Axis::Horizontal, 100, 100).wheel(0.0, 1.0, false));
}

#[test]
fn focus_reveal_moves_only_when_needed_and_preserves_margin() {
    let mut scroll = scroller();
    scroll.reveal(120, 160, 8);
    assert_eq!(scroll.position(), 0);
    scroll.reveal(390, 430, 8);
    assert_eq!(scroll.position(), 238);
    scroll.reveal(100, 140, 8);
    assert_eq!(scroll.position(), 92);
    scroll.reveal(92, 500, 8);
    assert_eq!(scroll.position(), 84);
}

#[test]
fn drag_becomes_a_pan_only_after_eight_pixels_and_cancels_click() {
    let mut scroll = scroller();
    scroll.press(100);
    assert_eq!(scroll.drag_to(92), ClickOutcome::Click);
    assert!(!scroll.panning());
    assert_eq!(scroll.drag_to(91), ClickOutcome::Pan);
    assert!(scroll.panning());
    assert_eq!(scroll.release(), ClickOutcome::Pan);

    scroll.press(100);
    assert_eq!(scroll.drag_to(105), ClickOutcome::Click);
    assert_eq!(scroll.release(), ClickOutcome::Click);
}
