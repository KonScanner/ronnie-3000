//! Pointer, keyboard, touch, duplicate, and stale-revision drag tests.

use std::sync::Arc;

use ronie_core::ChainSide;
use ronie_ui::{
    backend::{MockBackend, UiBackend},
    default::{ChainGallery, DragOutcome},
    testbed::load_scenarios,
};

fn gallery() -> (ChainGallery, Arc<MockBackend>) {
    let scenario = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == "main-stereo")
        .expect("scenario");
    let backend = Arc::new(MockBackend::from_scenario(&scenario));
    (ChainGallery::new(backend.clone()), backend)
}

#[test]
fn drag_uses_slop_and_cross_lane_move_commits_once() {
    let (mut gallery, backend) = gallery();
    let revision = backend.chain_state(0).revision;
    gallery.begin_drag("blk-l1", false);
    assert_eq!(gallery.drag_by(3.0), DragOutcome::None);
    assert_eq!(gallery.drag_by(6.0), DragOutcome::Engaged);
    assert_eq!(gallery.drop(ChainSide::Right, 1), DragOutcome::Moved);
    assert_eq!(backend.chain_state(0).revision, revision + 1);
    let chain = backend.chain_state(0).chain;
    assert!(chain.left.len() >= 3 && chain.left.iter().any(|block| block.is_insert));
    assert_eq!(
        chain
            .right
            .iter()
            .filter(|block| block.id.0 == "blk-l1")
            .count(),
        1
    );
}

#[test]
fn reverse_cross_lane_move_uses_the_right_source_index() {
    let (mut gallery, backend) = gallery();
    gallery.begin_drag("blk-r1", false);
    gallery.drag_by(6.0);
    assert_eq!(gallery.drop(ChainSide::Left, 1), DragOutcome::Moved);
    let chain = backend.chain_state(0).chain;
    assert_eq!(chain.left[1].id.0, "blk-r1");
    assert_eq!(
        chain
            .left
            .iter()
            .filter(|block| block.id.0 == "blk-r1")
            .count(),
        1
    );
    assert!(!chain.right.iter().any(|block| block.id.0 == "blk-r1"));
}

#[test]
fn optimistic_preview_does_not_mutate_until_drop() {
    let (mut gallery, backend) = gallery();
    let revision = backend.chain_state(0).revision;
    gallery.begin_drag("blk-l1", false);
    gallery.drag_by(6.0);
    assert_eq!(
        gallery.preview_drop(ChainSide::Right, 1),
        DragOutcome::Engaged
    );
    assert_eq!(gallery.lane_ids(ChainSide::Right)[1], "blk-l1");
    assert!(
        !backend
            .chain_state(0)
            .chain
            .right
            .iter()
            .any(|block| block.id.0 == "blk-l1")
    );
    assert_eq!(backend.chain_state(0).revision, revision);
    assert_eq!(gallery.drop(ChainSide::Right, 1), DragOutcome::Moved);
    assert!(
        backend
            .chain_state(0)
            .chain
            .right
            .iter()
            .any(|block| block.id.0 == "blk-l1")
    );
}

#[test]
fn alt_drop_duplicates_into_the_requested_lane_and_index() {
    let (mut gallery, backend) = gallery();
    let revision = backend.chain_state(0).revision;
    gallery.begin_drag("blk-l1", true);
    gallery.drag_by(6.0);
    assert_eq!(gallery.drop(ChainSide::Right, 1), DragOutcome::Duplicated);
    assert_eq!(backend.chain_state(0).revision, revision + 1);
    let chain = backend.chain_state(0).chain;
    assert!(chain.left.iter().any(|block| block.id.0 == "blk-l1"));
    assert_eq!(chain.right[1].tone, chain.left[0].tone);
    assert_ne!(chain.right[1].id, chain.left[0].id);
}

#[test]
fn alt_can_change_during_a_drag() {
    let (mut gallery, _backend) = gallery();
    gallery.begin_drag("blk-l1", false);
    gallery.drag_by(6.0);
    gallery.set_drag_duplicate(true);
    assert_eq!(gallery.drop(ChainSide::Left, 2), DragOutcome::Duplicated);
}

#[test]
fn keyboard_sort_uses_preview_and_the_same_commit_command() {
    let (mut gallery, backend) = gallery();
    let revision = backend.chain_state(0).revision;
    gallery.begin_keyboard_sort("blk-l2");
    assert_eq!(gallery.move_keyboard_by(-1, 0), DragOutcome::Engaged);
    assert_eq!(gallery.lane_ids(ChainSide::Left)[0], "blk-l2");
    assert_eq!(gallery.drop(ChainSide::Left, 0), DragOutcome::Moved);
    assert_eq!(backend.chain_state(0).revision, revision + 1);
}

#[test]
fn insert_slots_never_cross_lanes() {
    let (mut gallery, backend) = gallery();
    let insert = gallery
        .lane_ids(ChainSide::Left)
        .last()
        .expect("insert")
        .clone();
    let revision = backend.chain_state(0).revision;
    gallery.begin_drag(&insert, false);
    gallery.drag_by(6.0);
    assert_eq!(gallery.drop(ChainSide::Right, 0), DragOutcome::Stale);
    assert_eq!(backend.chain_state(0).revision, revision);
}

#[test]
fn stale_ids_and_cancellation_never_mutate_the_chain() {
    let (mut gallery, backend) = gallery();
    gallery.begin_drag("missing", false);
    gallery.drag_by(6.0);
    assert_eq!(gallery.drop(ChainSide::Right, 0), DragOutcome::Stale);
    let revision = backend.chain_state(0).revision;
    gallery.begin_drag("blk-l1", false);
    gallery.cancel_drag();
    assert_eq!(backend.chain_state(0).revision, revision);
    gallery.begin_drag("blk-l1", false);
    gallery.drag_by(6.0);
    backend
        .dispatch(ronie_ui::backend::UiAction::Undo)
        .expect("external revision");
    assert_eq!(gallery.drop(ChainSide::Right, 0), DragOutcome::Stale);
}

#[test]
fn quick_touch_flick_pans_instead_of_sorting() {
    let (gallery, backend) = gallery();
    let revision = backend.chain_state(0).revision;
    assert!(gallery.touch_flick(80.0, 20.0, 140));
    assert_eq!(backend.chain_state(0).revision, revision);
    assert!(!gallery.touch_flick(5.0, 20.0, 140));
    assert!(!gallery.touch_flick(80.0, 20.0, 200));
}

#[test]
fn touch_hold_requires_delay_and_stays_inside_slop() {
    let (gallery, _backend) = gallery();
    assert!(gallery.touch_hold(5.0, -5.0, 500));
    assert!(!gallery.touch_hold(0.0, 0.0, 499));
    assert!(!gallery.touch_hold(6.0, 0.0, 500));
}
