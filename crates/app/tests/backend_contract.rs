//! One set of behaviour checks for each `UiBackend` adapter.
//!
//! Each check runs through `&dyn UiBackend`, because that is all that a UI caller sees. The rules
//! here are what the UI depends on, so a check that fails for one adapter is a real difference.
//!
//! Each adapter sends its edits to one session edit module, so this file proves that each
//! adapter keeps no rule of its own on top of it. [`Gap`] lists what an adapter does not do by
//! design. A check for a gap also proves the gap, so an entry that stops being true fails here.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ronie_app::standalone_backend::StandaloneBackend;
use ronie_core::{
    BlockId, BlockParams, ChainBranch, ChainSide, ChainSnapshot, EqParams, InputMode, LocalStash,
    MidiSource, ModelBytes, ModelId, PREFERENCES_SETTINGS, Tone,
    params::{ParameterCatalog, ParameterId, ParameterValue},
};
use ronie_dsp::PrepareSpec;
use ronie_plugin::{ProcessorBackend, RonieProcessor};
use ronie_ui::{
    backend::{
        AudioDeviceConfig, ChainFaultKind, ChainViewState, DeviceAction, GesturePhase, HostControl,
        MidiAction, MockBackend, NavigationAction, UiAction, UiBackend, UiEffect, UiError,
    },
    testbed::{UiScenario, UiSection},
};

/// What one adapter does not do by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gap {
    /// The mock holds no preset documents. The preset list of a fixture is in the UI, so an
    /// unknown ID must load, rename, and delete with no error.
    MockHasNoPresetStore,
    /// The mock has no model compiler, so each set of bytes is valid.
    MockCompilesNoModel,
    /// Only a standalone adapter owns an audio device and a running chain.
    OnlyStandaloneHasAudio,
    /// The visual scenarios show each control, so the mock offers even those with no real action.
    MockOffersEachControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mock,
    Processor,
    Standalone,
}

struct Adapter {
    kind: Kind,
    backend: Box<dyn UiBackend>,
    _root: tempfile::TempDir,
}

impl Adapter {
    fn new(kind: Kind) -> Self {
        let root = tempfile::tempdir().expect("temporary root");
        let backend: Box<dyn UiBackend> = match kind {
            Kind::Mock => Box::new(MockBackend::from_scenario(&UiScenario {
                id: "contract".to_owned(),
                section: UiSection::Main,
                state: serde_json::json!({}),
                drives: Vec::new(),
                viewport: Default::default(),
            })),
            Kind::Processor => Box::new(processor(root.path())),
            Kind::Standalone => Box::new(
                StandaloneBackend::dummy(
                    processor(root.path()),
                    &root.path().join(PREFERENCES_SETTINGS),
                )
                .expect("dummy standalone backend"),
            ),
        };
        Self {
            kind,
            backend,
            _root: root,
        }
    }

    fn backend(&self) -> &dyn UiBackend {
        self.backend.as_ref()
    }

    fn has(&self, gap: Gap) -> bool {
        match gap {
            Gap::MockHasNoPresetStore | Gap::MockCompilesNoModel | Gap::MockOffersEachControl => {
                self.kind == Kind::Mock
            }
            Gap::OnlyStandaloneHasAudio => self.kind != Kind::Standalone,
        }
    }

    /// Sends an action that the rules accept, and checks what each accepted action must do.
    fn accept(&self, action: UiAction) -> UiEffect {
        let backend = self.backend();
        let before = backend.chain_state(0).revision;
        let shown = format!("{action:?}");
        let receipt = backend
            .dispatch(action)
            .unwrap_or_else(|error| panic!("{shown}: {error}"));
        let view = backend.chain_state(0);
        assert!(view.revision > before, "{shown}");
        assert_eq!(view.revision, receipt.revision, "{shown}");
        assert!(backend.chain_state(before).changed);
        assert!(!backend.chain_state(view.revision).changed);
        self.check_view(&view);
        receipt.effect
    }

    /// Sends an action that the rules refuse, and checks that the refusal changes nothing.
    fn refuse(&self, action: UiAction, wanted: Refusal) {
        let backend = self.backend();
        let before = backend.chain_state(0);
        let paste = backend.can_paste_block();
        let shown = format!("{action:?}");
        let error = backend
            .dispatch(action)
            .expect_err(&format!("{shown} must fail"));
        assert!(wanted.matches(&error), "{shown}: {error}");
        let after = backend.chain_state(0);
        assert_eq!(after.revision, before.revision, "{shown}");
        assert_eq!(after.chain, before.chain, "{shown}");
        assert_eq!(after.can_undo, before.can_undo, "{shown}");
        assert_eq!(after.can_redo, before.can_redo, "{shown}");
        assert_eq!(backend.can_paste_block(), paste, "{shown}");
    }

    /// What is true of a chain view after each change.
    fn check_view(&self, view: &ChainViewState) {
        assert_eq!(view.chain.revision, view.revision);
        assert_eq!(view.session.chain, *view.chain);
        assert_eq!(view.preparation.input_mode, view.session.input_mode);
        assert_eq!(view.preparation.stereo_input, view.chain.stereo_input);
        let minimum = if view.chain.stereo_enabled { 3 } else { 5 };
        for lane in [&view.chain.left, &view.chain.right] {
            let tones = lane.iter().take_while(|block| !block.is_insert).count();
            assert!(
                lane[tones..].iter().all(|block| block.is_insert),
                "the tones of a lane have no open slot between them"
            );
            assert_eq!(
                lane.len(),
                (tones + 1).max(minimum),
                "a lane keeps one open slot after its last tone, and its minimum"
            );
        }
    }

    /// Loads a tone into the open slot at `index` of the left lane.
    fn load(&self, index: usize, tone_id: i64) -> BlockId {
        let target = self.backend().chain_state(0).chain.left[index].id.clone();
        assert_eq!(
            self.accept(UiAction::LoadTone {
                target_insert_id: target.clone(),
                tone: tone(tone_id),
                model_id: ModelId(tone_id * 10),
            }),
            UiEffect::ChainChanged
        );
        target
    }

    /// The chain with no revision, so that two chains compare by their content.
    fn chain(&self) -> ChainSnapshot {
        let mut chain = std::sync::Arc::unwrap_or_clone(self.backend().chain_state(0).chain);
        chain.revision = 0;
        chain
    }

    fn value(&self, id: &'static str) -> f32 {
        self.backend()
            .parameter(ParameterId(id))
            .expect("catalog parameter")
            .0
    }
}

#[derive(Debug, Clone, Copy)]
enum Refusal {
    UnknownTarget,
    UnknownParameter,
    Invalid,
}

impl Refusal {
    fn matches(self, error: &UiError) -> bool {
        matches!(
            (self, error),
            (Self::UnknownTarget, UiError::UnknownTarget(_))
                | (Self::UnknownParameter, UiError::UnknownParameter(_))
                | (Self::Invalid, UiError::InvalidAction(_))
        )
    }
}

fn processor(root: &std::path::Path) -> ProcessorBackend {
    let stash = LocalStash::new(root.join("stash")).expect("stash");
    ProcessorBackend::new(RonieProcessor::with_stash(
        PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 128,
            oversampling: 1,
            input_mode: InputMode::Left,
            stereo_input: false,
            stereo_output: true,
        },
        stash,
    ))
}

/// Runs one check against each adapter, and names each adapter that fails it.
fn for_each_adapter(check: impl Fn(&Adapter)) {
    let failed: Vec<Kind> = [Kind::Mock, Kind::Processor, Kind::Standalone]
        .into_iter()
        .filter(|kind| {
            let adapter = Adapter::new(*kind);
            catch_unwind(AssertUnwindSafe(|| check(&adapter))).is_err()
        })
        .collect();
    assert!(failed.is_empty(), "the contract fails for {failed:?}");
}

fn tone(id: i64) -> Tone {
    Tone::from_value(serde_json::json!({
        "id": id,
        "title": format!("Tone {id}"),
        "format": "ir",
        "models": [
            {"id": id * 10, "name": "First", "model_url": "https://models.example/first.wav"},
            {"id": id * 10 + 1, "name": "Second", "model_url": "https://models.example/second.wav"},
        ],
    }))
    .expect("tone")
}

fn bytes() -> ModelBytes {
    b"bytes that a download gave".to_vec().into()
}

fn unknown() -> BlockId {
    BlockId("no-such-block".to_owned())
}

fn position(lane: &[ronie_core::ChainBlock], id: &BlockId) -> Option<usize> {
    lane.iter().position(|block| &block.id == id)
}

#[test]
fn an_accepted_edit_raises_the_revision_and_reports_it() {
    for_each_adapter(|adapter| {
        let block = adapter.load(0, 1);
        let backend = adapter.backend();
        for action in [
            UiAction::SetStereoMode(true),
            UiAction::SelectSide(ChainSide::Right),
            UiAction::SelectSide(ChainSide::Left),
            UiAction::SetBlockParams {
                block_id: block.clone(),
                params: BlockParams::default(),
            },
            UiAction::CopyBlock(block.clone()),
            UiAction::PasteBlock {
                side: ChainSide::Right,
                index: 0,
            },
            UiAction::DuplicateBlock(block.clone()),
            UiAction::DuplicateBlockToLane {
                block_id: block.clone(),
                target_side: ChainSide::Right,
                target_index: 1,
            },
            UiAction::MoveBlock {
                block_id: block.clone(),
                target_index: 1,
            },
            UiAction::MoveBlockToLane {
                block_id: block.clone(),
                target_side: ChainSide::Right,
                target_index: 0,
            },
            UiAction::SwapChains,
            UiAction::SetBranch(None),
            UiAction::SetInputMode(InputMode::DualMono),
            UiAction::RemoveBlock(block),
        ] {
            assert_eq!(adapter.accept(action), UiEffect::ChainChanged);
        }
        assert!(backend.chain_state(0).can_undo);
    });
}

#[test]
fn a_loaded_tone_fills_the_slot_with_default_params() {
    for_each_adapter(|adapter| {
        let block = adapter.load(1, 7);
        let chain = adapter.chain();
        let loaded = chain.block(&block).expect("loaded block");
        assert!(!loaded.is_insert);
        assert_eq!(loaded.tone.as_ref().expect("tone").id, tone(7).id);
        assert_eq!(loaded.active_model_id, Some(ModelId(70)));
        assert_eq!(loaded.params, BlockParams::default());
        assert_eq!(
            position(&chain.left, &block),
            Some(0),
            "a tone in a later open slot moves left to close the gap"
        );
    });
}

#[test]
fn a_full_lane_grows_one_open_slot() {
    for_each_adapter(|adapter| {
        let slots = adapter.chain().left.len();
        for index in 0..slots {
            adapter.load(index, index as i64 + 1);
        }
        let chain = adapter.chain();
        assert_eq!(chain.left.len(), slots + 1);
        assert!(chain.left[slots].is_insert);
    });
}

#[test]
fn a_removed_block_takes_its_slot_and_the_later_tones_move_left() {
    for_each_adapter(|adapter| {
        let first = adapter.load(0, 1);
        let second = adapter.load(1, 2);
        let third = adapter.load(2, 3);
        adapter.accept(UiAction::RemoveBlock(first.clone()));
        let chain = adapter.chain();
        // An open slot can take a free numbered name again, but no tone has the removed name.
        assert!(chain.block(&first).is_none_or(|block| block.is_insert));
        assert_eq!(position(&chain.left, &second), Some(0));
        assert_eq!(position(&chain.left, &third), Some(1));
        assert_eq!(chain.left.len(), 5, "a mono lane keeps five slots");
    });
}

#[test]
fn a_long_lane_keeps_one_open_slot_after_a_remove() {
    for_each_adapter(|adapter| {
        let blocks: Vec<BlockId> = (0..6)
            .map(|index| adapter.load(index, index as i64 + 1))
            .collect();
        adapter.accept(UiAction::RemoveBlock(blocks[2].clone()));
        let chain = adapter.chain();
        assert_eq!(chain.left.len(), 6);
        assert!(chain.left[5].is_insert);
        assert_eq!(position(&chain.left, &blocks[3]), Some(2));
    });
}

#[test]
fn an_undo_of_a_remove_puts_the_tone_back_in_its_slot() {
    for_each_adapter(|adapter| {
        let first = adapter.load(0, 1);
        let second = adapter.load(1, 2);
        let third = adapter.load(2, 3);
        let before = adapter.chain();
        adapter.accept(UiAction::RemoveBlock(second.clone()));
        adapter.accept(UiAction::Undo);
        let chain = adapter.chain();
        assert_eq!(position(&chain.left, &first), Some(0));
        assert_eq!(position(&chain.left, &second), Some(1));
        assert_eq!(position(&chain.left, &third), Some(2));
        assert_eq!(chain.left, before.left);
    });
}

#[test]
fn a_branch_after_a_removed_tone_moves_to_the_tone_before_it() {
    for_each_adapter(|adapter| {
        let first = adapter.load(0, 1);
        let second = adapter.load(1, 2);
        adapter.accept(UiAction::SetStereoMode(true));
        adapter.accept(UiAction::SetBranch(Some(ronie_core::ChainBranch {
            side: ChainSide::Left,
            after_block_id: second.clone(),
        })));
        adapter.accept(UiAction::RemoveBlock(second));
        let branch = adapter.chain().branch.expect("the branch stays");
        assert_eq!(branch.after_block_id, first);
        adapter.accept(UiAction::RemoveBlock(first));
        assert_eq!(
            adapter.chain().branch,
            None,
            "no tone is left to split after"
        );
    });
}

#[test]
fn a_block_moves_within_the_active_lane_and_the_index_is_clamped() {
    for_each_adapter(|adapter| {
        let first = adapter.load(0, 1);
        adapter.load(1, 2);
        adapter.load(2, 3);
        adapter.accept(UiAction::MoveBlock {
            block_id: first.clone(),
            target_index: 1,
        });
        assert_eq!(position(&adapter.chain().left, &first), Some(1));
        adapter.accept(UiAction::MoveBlock {
            block_id: first.clone(),
            target_index: 1_000,
        });
        assert_eq!(
            position(&adapter.chain().left, &first),
            Some(2),
            "a move past the end puts the tone after the last tone"
        );
    });
}

#[test]
fn a_block_moves_to_the_other_lane_only_in_stereo() {
    for_each_adapter(|adapter| {
        let block = adapter.load(0, 1);
        let to_right = || UiAction::MoveBlockToLane {
            block_id: block.clone(),
            target_side: ChainSide::Right,
            target_index: 1,
        };
        adapter.refuse(to_right(), Refusal::Invalid);
        adapter.accept(UiAction::SetStereoMode(true));
        adapter.accept(to_right());
        let chain = adapter.chain();
        assert_eq!(position(&chain.left, &block), None);
        assert_eq!(
            position(&chain.right, &block),
            Some(0),
            "a tone in an empty lane moves to its first slot"
        );

        let slot = chain.left[0].id.clone();
        adapter.refuse(
            UiAction::MoveBlockToLane {
                block_id: slot,
                target_side: ChainSide::Right,
                target_index: 0,
            },
            Refusal::Invalid,
        );
    });
}

#[test]
fn a_duplicate_is_a_new_block_with_the_same_tone_after_its_source() {
    for_each_adapter(|adapter| {
        let block = adapter.load(0, 1);
        let params = BlockParams {
            mix: 0.25,
            ..Default::default()
        };
        adapter.accept(UiAction::SetBlockParams {
            block_id: block.clone(),
            params: params.clone(),
        });
        adapter.accept(UiAction::DuplicateBlock(block.clone()));
        let chain = adapter.chain();
        let copy = &chain.left[1];
        assert_ne!(copy.id, block);
        assert!(!copy.is_insert);
        assert_eq!(copy.tone, chain.left[0].tone);
        assert_eq!(copy.params, params);

        let copy = copy.id.clone();
        let changed = BlockParams {
            mix: 0.75,
            ..params.clone()
        };
        adapter.accept(UiAction::SetBlockParams {
            block_id: copy,
            params: changed,
        });
        assert_eq!(
            adapter.chain().block(&block).expect("source").params,
            params,
            "an edit of the copy must not reach the source"
        );

        adapter.accept(UiAction::SetStereoMode(true));
        adapter.accept(UiAction::DuplicateBlockToLane {
            block_id: block.clone(),
            target_side: ChainSide::Right,
            target_index: 1_000,
        });
        let chain = adapter.chain();
        assert!(
            chain
                .right
                .iter()
                .any(|copy| copy.tone == chain.left[0].tone && !copy.is_insert)
        );
    });
}

#[test]
fn a_copied_block_pastes_as_a_new_block() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        assert!(!backend.can_paste_block());
        let block = adapter.load(0, 1);
        adapter.accept(UiAction::CopyBlock(block.clone()));
        assert!(backend.can_paste_block());
        adapter.refuse(
            UiAction::PasteBlock {
                side: ChainSide::Right,
                index: 0,
            },
            Refusal::Invalid,
        );
        adapter.accept(UiAction::PasteBlock {
            side: ChainSide::Left,
            index: 2,
        });
        // The paste goes after the last tone, because a lane has no gap between tones.
        let chain = adapter.chain();
        assert_ne!(chain.left[1].id, block);
        assert_eq!(chain.left[1].tone, chain.left[0].tone);
    });
}

#[test]
fn a_bypass_and_other_block_params_read_back_on_that_block_only() {
    for_each_adapter(|adapter| {
        let first = adapter.load(0, 1);
        let second = adapter.load(1, 2);
        let bypassed = BlockParams {
            enabled: false,
            output_gain: 0.25,
            ..Default::default()
        };
        adapter.accept(UiAction::SetBlockParams {
            block_id: first.clone(),
            params: bypassed.clone(),
        });
        let chain = adapter.chain();
        assert_eq!(chain.block(&first).expect("first").params, bypassed);
        assert_eq!(
            chain.block(&second).expect("second").params,
            BlockParams::default()
        );

        let eq = EqParams {
            enabled: true,
            pre: true,
            bands: Vec::new(),
        };
        adapter.accept(UiAction::SetEq {
            block_id: second.clone(),
            params: eq.clone(),
            gesture: GesturePhase::End,
        });
        let chain = adapter.chain();
        assert_eq!(chain.block(&second).expect("second").params.eq, eq);
    });
}

#[test]
fn stereo_mode_sets_the_lanes_and_a_swap_needs_it() {
    for_each_adapter(|adapter| {
        let block = adapter.load(0, 1);
        adapter.refuse(UiAction::SwapChains, Refusal::Invalid);
        adapter.refuse(
            UiAction::SetBranch(Some(ChainBranch {
                side: ChainSide::Left,
                after_block_id: block.clone(),
            })),
            Refusal::Invalid,
        );

        adapter.accept(UiAction::SetStereoMode(true));
        assert!(adapter.chain().stereo_enabled);
        adapter.accept(UiAction::SetBranch(Some(ChainBranch {
            side: ChainSide::Left,
            after_block_id: block.clone(),
        })));
        adapter
            .backend()
            .set_parameter(
                ParameterId("chainInvertLeft"),
                ParameterValue(1.0),
                GesturePhase::End,
            )
            .expect("invert");
        let invert_right = adapter.value("chainInvertRight");

        adapter.accept(UiAction::SwapChains);
        let chain = adapter.chain();
        assert!(position(&chain.right, &block).is_some());
        assert_eq!(
            chain.branch.as_ref().expect("branch").side,
            ChainSide::Right
        );
        assert_eq!(adapter.value("chainInvertRight"), 1.0);
        assert_eq!(adapter.value("chainInvertLeft"), invert_right);

        adapter.accept(UiAction::SetStereoMode(false));
        let chain = adapter.chain();
        assert!(!chain.stereo_enabled);
        assert_eq!(chain.branch, None, "mono has no branch");
    });
}

#[test]
fn undo_and_redo_walk_the_history() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();

        let start = backend.chain_state(0);
        assert!(!start.can_undo);
        assert!(!start.can_redo);
        adapter.refuse(UiAction::Undo, Refusal::Invalid);
        adapter.refuse(UiAction::Redo, Refusal::Invalid);

        let empty = adapter.chain();
        adapter.load(0, 1);
        let loaded = adapter.chain();
        assert_ne!(loaded, empty);
        assert!(backend.chain_state(0).can_undo);

        adapter.accept(UiAction::Undo);
        assert_eq!(adapter.chain(), empty);
        let view = backend.chain_state(0);
        assert!(!view.can_undo);
        assert!(view.can_redo);

        adapter.accept(UiAction::Redo);
        assert_eq!(adapter.chain(), loaded);
        assert!(!backend.chain_state(0).can_redo);

        adapter.accept(UiAction::Undo);
        adapter.accept(UiAction::SetStereoMode(true));
        assert!(
            !backend.chain_state(0).can_redo,
            "a new edit ends the redo history"
        );

        let id = ParameterId("outputLevel");
        let value = adapter.value("outputLevel");
        backend
            .set_parameter(id, ParameterValue(value / 2.0), GesturePhase::End)
            .expect("edit");
        adapter.accept(UiAction::Undo);
        assert_eq!(adapter.value("outputLevel"), value);

        let block = adapter.load(0, 1);
        let before = adapter
            .chain()
            .block(&block)
            .expect("block")
            .params
            .eq
            .clone();
        let edit = EqParams {
            enabled: !before.enabled,
            ..before.clone()
        };
        adapter.accept(UiAction::SetEq {
            block_id: block.clone(),
            params: edit.clone(),
            gesture: GesturePhase::Begin,
        });
        assert_eq!(
            adapter.chain().block(&block).expect("block").params.eq,
            edit
        );
        adapter.accept(UiAction::SetEq {
            block_id: block.clone(),
            params: edit,
            gesture: GesturePhase::Cancel,
        });
        assert_eq!(
            adapter.chain().block(&block).expect("block").params.eq,
            before,
            "a cancel restores the value from before the gesture"
        );
    });
}

/// One edit of `outputLevel` through the parameter boundary.
fn write_level(backend: &dyn UiBackend, value: f32, gesture: GesturePhase) {
    backend
        .set_parameter(ParameterId("outputLevel"), ParameterValue(value), gesture)
        .expect("write");
}

/// Walks the whole undo history, and returns the steps that it holds.
fn undo_steps(backend: &dyn UiBackend) -> usize {
    let mut steps = 0;
    while backend.chain_state(0).can_undo {
        backend.dispatch(UiAction::Undo).expect("undo");
        steps += 1;
    }
    steps
}

#[test]
fn one_gesture_is_one_undo_step_and_each_change_still_arrives() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let start = adapter.value("outputLevel");
        let before = backend.chain_state(0).revision;

        write_level(backend, start, GesturePhase::Begin);
        for value in [0.6, 0.65, 0.7] {
            write_level(backend, value, GesturePhase::Change);
            assert!((adapter.value("outputLevel") - value).abs() < 0.001);
        }
        write_level(backend, 0.7, GesturePhase::End);
        assert!(backend.chain_state(0).revision > before);

        assert!(backend.chain_state(0).can_undo);
        adapter.accept(UiAction::Undo);
        assert_eq!(adapter.value("outputLevel"), start);
        assert!(!backend.chain_state(0).can_undo, "the drag was one step");
        adapter.accept(UiAction::Redo);
        assert!((adapter.value("outputLevel") - 0.7).abs() < 0.001);
        assert!(!backend.chain_state(0).can_redo);
    });
}

#[test]
fn a_gesture_that_changes_nothing_is_no_undo_step_and_keeps_the_redo_history() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let start = adapter.value("outputLevel");
        write_level(backend, start, GesturePhase::Begin);
        write_level(backend, start, GesturePhase::Change);
        write_level(backend, start, GesturePhase::End);
        assert!(!backend.chain_state(0).can_undo);

        write_level(backend, 0.3, GesturePhase::End);
        adapter.accept(UiAction::Undo);
        assert!(backend.chain_state(0).can_redo);
        write_level(backend, start, GesturePhase::Begin);
        write_level(backend, start, GesturePhase::End);
        assert!(backend.chain_state(0).can_redo);
    });
}

#[test]
fn a_write_with_no_gesture_is_one_undo_step() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        write_level(backend, 0.2, GesturePhase::End);
        write_level(backend, 0.3, GesturePhase::Change);
        write_level(backend, 0.4, GesturePhase::Change);
        assert_eq!(undo_steps(backend), 3);
    });
}

#[test]
fn two_gestures_are_two_undo_steps() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        for value in [0.2, 0.3] {
            write_level(backend, adapter.value("outputLevel"), GesturePhase::Begin);
            write_level(backend, value, GesturePhase::Change);
            write_level(backend, value, GesturePhase::End);
        }
        assert_eq!(undo_steps(backend), 2);
    });
}

#[test]
fn a_gesture_that_writes_two_parameters_is_one_undo_step() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let (left, right) = (ParameterId("chainPanLeft"), ParameterId("chainPanRight"));
        let (first_left, first_right) = (
            adapter.value("chainPanLeft"),
            adapter.value("chainPanRight"),
        );
        for (id, value, gesture) in [
            (left, first_left, GesturePhase::Begin),
            (left, 0.2, GesturePhase::Change),
            (right, 0.8, GesturePhase::Change),
            (left, 0.2, GesturePhase::End),
        ] {
            backend
                .set_parameter(id, ParameterValue(value), gesture)
                .expect("write");
        }
        adapter.accept(UiAction::Undo);
        assert_eq!(adapter.value("chainPanLeft"), first_left);
        assert_eq!(adapter.value("chainPanRight"), first_right);
        assert!(!backend.chain_state(0).can_undo);
    });
}

#[test]
fn a_cancelled_gesture_restores_each_value_and_leaves_no_undo_step() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let start = adapter.value("outputLevel");
        let id = ParameterId("outputLevel");
        assert!(matches!(
            backend.set_parameter(id, ParameterValue(0.5), GesturePhase::Cancel),
            Err(UiError::InvalidAction(_))
        ));
        write_level(backend, start, GesturePhase::Begin);
        write_level(backend, 0.1, GesturePhase::Change);
        write_level(backend, 0.1, GesturePhase::Cancel);
        assert_eq!(adapter.value("outputLevel"), start);
        assert!(!backend.chain_state(0).can_undo);
    });
}

#[test]
fn an_undo_during_a_gesture_ends_it() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let start = adapter.value("outputLevel");
        write_level(backend, start, GesturePhase::Begin);
        write_level(backend, 0.1, GesturePhase::Change);
        adapter.accept(UiAction::Undo);
        assert_eq!(adapter.value("outputLevel"), start);

        // The next write has no open gesture, so that it is a step of its own.
        write_level(backend, 0.2, GesturePhase::Change);
        write_level(backend, 0.3, GesturePhase::Change);
        assert_eq!(undo_steps(backend), 2);
    });
}

#[test]
fn another_edit_during_a_gesture_keeps_its_own_undo_step() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let start = adapter.value("outputLevel");
        write_level(backend, start, GesturePhase::Begin);
        write_level(backend, 0.1, GesturePhase::Change);
        // A download that ends during a drag loads its tone here.
        adapter.load(0, 1);
        write_level(backend, 0.2, GesturePhase::Change);
        write_level(backend, 0.2, GesturePhase::End);
        assert_eq!(undo_steps(backend), 3);
        assert_eq!(adapter.value("outputLevel"), start);
    });
}

#[test]
fn a_block_knob_gesture_is_one_undo_step_and_each_change_still_arrives() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let block = adapter.load(0, 1);
        let start = adapter.chain().block(&block).expect("block").params.clone();
        let step = |mix: f64, gesture| {
            adapter.accept(UiAction::SetBlockParamsGesture {
                block_id: block.clone(),
                params: BlockParams {
                    mix,
                    ..start.clone()
                },
                gesture,
            });
        };

        step(start.mix, GesturePhase::Begin);
        for mix in [0.6, 0.7, 0.8] {
            step(mix, GesturePhase::Change);
            assert_eq!(
                adapter.chain().block(&block).expect("block").params.mix,
                mix
            );
        }
        step(0.8, GesturePhase::End);
        adapter.accept(UiAction::Undo);
        assert_eq!(adapter.chain().block(&block).expect("block").params, start);
        adapter.accept(UiAction::Undo);
        assert!(!backend.chain_state(0).can_undo, "the load and one drag");

        adapter.accept(UiAction::Redo);
        step(start.mix, GesturePhase::Begin);
        step(start.mix, GesturePhase::End);
        adapter.accept(UiAction::Undo);
        assert!(!backend.chain_state(0).can_undo, "no change, no step");

        adapter.accept(UiAction::Redo);
        step(start.mix, GesturePhase::Begin);
        step(0.1, GesturePhase::Change);
        step(0.1, GesturePhase::Cancel);
        assert_eq!(adapter.chain().block(&block).expect("block").params, start);
        adapter.accept(UiAction::Undo);
        assert!(!backend.chain_state(0).can_undo, "a cancel leaves no step");
    });
}

#[test]
fn a_block_knob_gesture_needs_a_known_block() {
    for_each_adapter(|adapter| {
        adapter.refuse(
            UiAction::SetBlockParamsGesture {
                block_id: unknown(),
                params: BlockParams::default(),
                gesture: GesturePhase::Begin,
            },
            Refusal::UnknownTarget,
        );
    });
}

/// One block knob write of `mix`.
fn knob(adapter: &Adapter, block: &BlockId, mix: f64, gesture: GesturePhase) -> UiEffect {
    let shown = format!("{block:?} {gesture:?}");
    adapter
        .backend()
        .dispatch(UiAction::SetBlockParamsGesture {
            block_id: block.clone(),
            params: BlockParams {
                mix,
                ..Default::default()
            },
            gesture,
        })
        .unwrap_or_else(|error| panic!("{shown}: {error}"))
        .effect
}

#[test]
fn a_removed_block_closes_its_gesture_and_its_end_changes_nothing() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let first = adapter.load(0, 1);
        let second = adapter.load(1, 2);
        knob(
            adapter,
            &first,
            BlockParams::default().mix,
            GesturePhase::Begin,
        );
        knob(adapter, &first, 0.1, GesturePhase::Change);
        adapter.accept(UiAction::RemoveBlock(first.clone()));

        let before = backend.chain_state(0);
        for gesture in [GesturePhase::End, GesturePhase::Cancel] {
            assert_eq!(knob(adapter, &first, 0.2, gesture), UiEffect::None);
            let eq = backend
                .dispatch(UiAction::SetEq {
                    block_id: first.clone(),
                    params: EqParams::default(),
                    gesture,
                })
                .expect("an EQ end for a lost block");
            assert_eq!(eq.effect, UiEffect::None);
        }
        let after = backend.chain_state(0);
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.chain, before.chain);
        assert_eq!(after.can_redo, before.can_redo);

        // Each write of the other block is a step of its own, not a part of the closed gesture.
        knob(adapter, &second, 0.2, GesturePhase::Change);
        knob(adapter, &second, 0.3, GesturePhase::Change);
        adapter.accept(UiAction::Undo);
        assert_eq!(
            adapter.chain().block(&second).expect("second").params.mix,
            0.2
        );
    });
}

#[test]
fn an_eq_cancel_keeps_the_step_of_another_action() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let block = adapter.load(0, 1);
        let other = adapter.load(1, 2);
        let before = adapter
            .chain()
            .block(&block)
            .expect("block")
            .params
            .eq
            .clone();
        let edit = EqParams {
            enabled: !before.enabled,
            ..before.clone()
        };
        let eq = |block: &BlockId, gesture| UiAction::SetEq {
            block_id: block.clone(),
            params: edit.clone(),
            gesture,
        };
        adapter.accept(eq(&block, GesturePhase::Begin));
        adapter.accept(UiAction::SetMultiCore(false));
        adapter.refuse(eq(&other, GesturePhase::Cancel), Refusal::Invalid);
        adapter.accept(eq(&block, GesturePhase::Cancel));
        let shown = || {
            let view = backend.chain_state(0);
            let eq = view.chain.block(&block).expect("block").params.eq.clone();
            (eq, view.multi_core)
        };
        assert_eq!(shown(), (before.clone(), false));

        adapter.accept(UiAction::Undo);
        assert_eq!(shown(), (before, true), "the undo of the multicore change");
        assert_eq!(undo_steps(backend), 2, "the two loads, and no EQ step");
    });
}

#[test]
fn each_frame_drops_a_stale_or_deleted_active_preset() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let active = || {
            backend
                .chain_state(0)
                .session
                .active_preset
                .as_ref()
                .map(|preset| preset.name.clone())
        };
        let id = save(adapter, "Lead");
        adapter.accept(UiAction::SetMultiCore(false));
        adapter.accept(UiAction::RenamePreset {
            id: id.clone(),
            name: "Solo".to_owned(),
        });
        adapter.accept(UiAction::Undo);
        assert_eq!(active().as_deref(), Some("Solo"), "an undo after a rename");
        adapter.accept(UiAction::Redo);

        adapter.accept(UiAction::DeletePreset { id });
        adapter.accept(UiAction::Undo);
        assert_eq!(active(), None, "an undo after a delete");
        adapter.accept(UiAction::Undo);
        assert_eq!(active(), None, "an undo to before the save");
    });
}

#[test]
fn each_catalog_parameter_reads_and_an_unknown_one_does_not() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        for spec in ParameterCatalog::v1().iter() {
            let value = backend
                .parameter(spec.id)
                .unwrap_or_else(|| panic!("{:?}", spec.id));
            assert!(value.0 >= spec.min && value.0 <= spec.max, "{:?}", spec.id);
        }
        assert_eq!(backend.parameter(ParameterId("noSuchParameter")), None);
    });
}

#[test]
fn a_parameter_write_is_clamped_and_reads_back() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let id = ParameterId("outputLevel");
        let before = backend.chain_state(0).revision;
        backend
            .set_parameter(id, ParameterValue(0.25), GesturePhase::Begin)
            .expect("write");
        assert!((adapter.value("outputLevel") - 0.25).abs() < 0.001);
        let view = backend.chain_state(0);
        assert!(view.revision > before);
        adapter.check_view(&view);

        backend
            .set_parameter(id, ParameterValue(50.0), GesturePhase::End)
            .expect("write above the range");
        assert_eq!(adapter.value("outputLevel"), 1.0);

        assert_eq!(
            adapter.accept(UiAction::SetParameter {
                id,
                value: ParameterValue(-50.0),
                gesture: GesturePhase::Change,
            }),
            UiEffect::ParameterChanged
        );
        assert_eq!(adapter.value("outputLevel"), 0.0);
    });
}

#[test]
fn a_stepped_parameter_keeps_its_steps() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        backend
            .set_parameter(
                ParameterId("osFactor"),
                ParameterValue(1.4),
                GesturePhase::End,
            )
            .expect("choice");
        assert_eq!(adapter.value("osFactor"), 1.0);
        backend
            .set_parameter(
                ParameterId("gateEnabled"),
                ParameterValue(0.7),
                GesturePhase::End,
            )
            .expect("switch");
        assert_eq!(adapter.value("gateEnabled"), 1.0);
    });
}

#[test]
fn a_refused_parameter_write_changes_nothing() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let unknown = ParameterId("noSuchParameter");
        let revision = backend.chain_state(0).revision;
        assert!(matches!(
            backend.set_parameter(unknown, ParameterValue(0.5), GesturePhase::End),
            Err(UiError::UnknownParameter(_))
        ));
        let value = adapter.value("outputLevel");
        assert!(matches!(
            backend.set_parameter(
                ParameterId("outputLevel"),
                ParameterValue(f32::NAN),
                GesturePhase::End
            ),
            Err(UiError::InvalidAction(_))
        ));
        assert_eq!(adapter.value("outputLevel"), value);
        assert_eq!(backend.chain_state(0).revision, revision);

        adapter.refuse(
            UiAction::SetParameter {
                id: unknown,
                value: ParameterValue(0.5),
                gesture: GesturePhase::End,
            },
            Refusal::UnknownParameter,
        );
        adapter.refuse(
            UiAction::SetParameter {
                id: ParameterId("outputLevel"),
                value: ParameterValue(f32::INFINITY),
                gesture: GesturePhase::End,
            },
            Refusal::Invalid,
        );
    });
}

#[test]
fn a_request_for_an_unknown_block_is_refused() {
    for_each_adapter(|adapter| {
        adapter.load(0, 1);
        // With a redo step ready, `refuse` also proves that a refusal keeps the history.
        adapter.accept(UiAction::SetStereoMode(true));
        adapter.accept(UiAction::Undo);
        for action in [
            UiAction::RemoveBlock(unknown()),
            UiAction::DuplicateBlock(unknown()),
            UiAction::CopyBlock(unknown()),
            UiAction::DuplicateBlockToLane {
                block_id: unknown(),
                target_side: ChainSide::Left,
                target_index: 0,
            },
            UiAction::MoveBlock {
                block_id: unknown(),
                target_index: 0,
            },
            UiAction::MoveBlockToLane {
                block_id: unknown(),
                target_side: ChainSide::Left,
                target_index: 0,
            },
            UiAction::LoadTone {
                target_insert_id: unknown(),
                tone: tone(2),
                model_id: ModelId(20),
            },
            UiAction::LoadToneWithBytes {
                target_insert_id: unknown(),
                tone: tone(2),
                model_id: ModelId(20),
                bytes: bytes(),
            },
            UiAction::SelectModelWithBytes {
                block_id: unknown(),
                tone: tone(2),
                model_id: ModelId(20),
                replaces: None,
                bytes: bytes(),
            },
            UiAction::DropFile {
                target_insert_id: unknown(),
                path: "no-such-file.wav".into(),
            },
            UiAction::SetBlockParams {
                block_id: unknown(),
                params: BlockParams::default(),
            },
            // An end or a cancel for a block that is gone is accepted with no change.
            UiAction::SetEq {
                block_id: unknown(),
                params: EqParams::default(),
                gesture: GesturePhase::Change,
            },
            UiAction::SelectModel {
                block_id: unknown(),
                model_id: ModelId(10),
            },
        ] {
            adapter.refuse(action, Refusal::UnknownTarget);
        }
    });
}

#[test]
fn a_request_for_an_open_slot_or_in_the_wrong_state_is_refused() {
    for_each_adapter(|adapter| {
        let block = adapter.load(0, 1);
        let slot = adapter.chain().left[1].id.clone();
        for action in [
            UiAction::RemoveBlock(slot.clone()),
            UiAction::DuplicateBlock(slot.clone()),
            UiAction::CopyBlock(slot.clone()),
            UiAction::DuplicateBlockToLane {
                block_id: slot.clone(),
                target_side: ChainSide::Left,
                target_index: 0,
            },
        ] {
            adapter.refuse(action, Refusal::UnknownTarget);
        }
        for action in [
            UiAction::PasteBlock {
                side: ChainSide::Left,
                index: 0,
            },
            UiAction::RetryModelLoad(block.clone()),
            UiAction::RetryModelLoad(unknown()),
            UiAction::DuplicateBlockToLane {
                block_id: block.clone(),
                target_side: ChainSide::Right,
                target_index: 0,
            },
        ] {
            adapter.refuse(action, Refusal::Invalid);
        }

        adapter.accept(UiAction::SetStereoMode(true));
        adapter.refuse(
            UiAction::SetBranch(Some(ChainBranch {
                side: ChainSide::Left,
                after_block_id: unknown(),
            })),
            Refusal::UnknownTarget,
        );
        adapter.refuse(
            UiAction::SetBranch(Some(ChainBranch {
                side: ChainSide::Left,
                after_block_id: slot,
            })),
            Refusal::UnknownTarget,
        );
        adapter.refuse(
            UiAction::MoveBlock {
                block_id: adapter.chain().right[0].id.clone(),
                target_index: 0,
            },
            Refusal::UnknownTarget,
        );
    });
}

#[test]
fn downloaded_bytes_need_a_catalog_model_of_the_block() {
    for_each_adapter(|adapter| {
        let slot = adapter.chain().left[0].id.clone();
        for (tone, model_id, bytes) in [
            (tone(1), ModelId(99), bytes()),
            (tone(1), ModelId(10), ModelBytes::from(Vec::new())),
        ] {
            adapter.refuse(
                UiAction::LoadToneWithBytes {
                    target_insert_id: slot.clone(),
                    tone,
                    model_id,
                    bytes,
                },
                Refusal::Invalid,
            );
        }

        adapter.accept(UiAction::LoadToneWithBytes {
            target_insert_id: slot.clone(),
            tone: tone(1),
            model_id: ModelId(10),
            bytes: bytes(),
        });
        adapter.backend().settle_audio().expect("settle");
        for (held, replaces) in [(tone(2), Some(ModelId(10))), (tone(1), Some(ModelId(11)))] {
            let model_id = held.models[1].id;
            adapter.refuse(
                UiAction::SelectModelWithBytes {
                    block_id: slot.clone(),
                    tone: held,
                    model_id,
                    replaces,
                    bytes: bytes(),
                },
                Refusal::Invalid,
            );
        }
        adapter.accept(UiAction::SelectModelWithBytes {
            block_id: slot.clone(),
            tone: tone(1),
            model_id: ModelId(11),
            replaces: Some(ModelId(10)),
            bytes: bytes(),
        });
        adapter.backend().settle_audio().expect("settle");
        assert_eq!(
            adapter.chain().block(&slot).expect("block").active_model_id,
            Some(ModelId(11))
        );
    });
}

#[test]
fn input_mode_reaches_the_session_and_the_preparation() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        adapter.accept(UiAction::SetInputMode(InputMode::Right));
        assert_eq!(backend.chain_state(0).session.input_mode, InputMode::Right);
        assert_eq!(
            adapter.accept(UiAction::Device(DeviceAction::CycleInputMode)),
            UiEffect::DeviceChanged
        );
        assert_eq!(
            backend.chain_state(0).session.input_mode,
            InputMode::DualMono
        );
    });
}

#[test]
fn a_reset_keeps_the_input_mode_of_the_preparation_in_step() {
    for_each_adapter(|adapter| {
        adapter.accept(UiAction::SetInputMode(InputMode::Right));
        adapter.accept(UiAction::ResetToDefault);
    });
}

#[test]
fn a_reset_restores_the_default_session() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        adapter.load(0, 1);
        adapter.accept(UiAction::SetStereoMode(true));
        adapter.accept(UiAction::SetInputMode(InputMode::Right));
        adapter.accept(UiAction::SavePreset {
            name: "Before reset".to_owned(),
        });
        adapter.accept(UiAction::Midi(MidiAction::SetMapping {
            target_id: "outputLevel".to_owned(),
            source: MidiSource::Cc,
            number: 7,
        }));
        backend
            .set_parameter(
                ParameterId("outputLevel"),
                ParameterValue(0.123),
                GesturePhase::End,
            )
            .expect("edit");

        adapter.accept(UiAction::ResetToDefault);

        let view = backend.chain_state(0);
        let mut empty = ChainSnapshot::empty();
        empty.stereo_input = view.preparation.stereo_input;
        empty.stereo_output = view.preparation.stereo_output;
        empty.revision = view.revision;
        assert_eq!(*view.chain, empty);
        assert_eq!(view.session.input_mode, InputMode::Stereo);
        assert_eq!(view.session.active_preset, None);
        assert!(view.session.midi_mappings.is_empty());
        assert!(backend.midi_map().mappings.is_empty());
        let default = ParameterCatalog::v1()
            .iter()
            .find(|spec| spec.id == ParameterId("outputLevel"))
            .expect("catalog entry")
            .default;
        assert_eq!(adapter.value("outputLevel"), default);
    });
}

#[test]
fn multi_core_and_model_size_read_back() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        assert!(backend.chain_state(0).multi_core);
        assert!(!backend.chain_state(0).nam_full_size);
        adapter.accept(UiAction::SetMultiCore(false));
        adapter.accept(UiAction::SetNamFullSize(true));
        assert!(!backend.chain_state(0).multi_core);
        assert!(backend.chain_state(0).nam_full_size);
    });
}

#[test]
fn a_saved_preset_becomes_the_active_preset() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        for name in ["", "   "] {
            adapter.refuse(
                UiAction::SavePreset {
                    name: name.to_owned(),
                },
                Refusal::Invalid,
            );
        }
        assert_eq!(
            adapter.accept(UiAction::SavePreset {
                name: "Stadium Lead".to_owned(),
            }),
            UiEffect::PresetSaved
        );
        let active = backend
            .chain_state(0)
            .session
            .active_preset
            .clone()
            .expect("active preset");
        assert_eq!(active.name, "Stadium Lead");

        adapter.accept(UiAction::RenamePreset {
            id: active.id.clone(),
            name: "Arena Lead".to_owned(),
        });
        let renamed = backend
            .chain_state(0)
            .session
            .active_preset
            .clone()
            .expect("active preset");
        assert_eq!(renamed.name, "Arena Lead");
        assert_eq!(renamed.id, active.id);

        adapter.accept(UiAction::DeletePreset {
            id: active.id.clone(),
        });
        assert_eq!(backend.chain_state(0).session.active_preset, None);

        let missing = "user:no-such-preset".to_owned();
        if adapter.has(Gap::MockHasNoPresetStore) {
            adapter.accept(UiAction::LoadPreset {
                id: missing.clone(),
            });
            adapter.accept(UiAction::RenamePreset {
                id: missing.clone(),
                name: "Other".to_owned(),
            });
            adapter.accept(UiAction::MovePreset {
                id: missing.clone(),
                delta: 1,
            });
            adapter.accept(UiAction::DeletePreset { id: missing });
        } else {
            adapter.refuse(
                UiAction::MovePreset {
                    id: missing.clone(),
                    delta: 1,
                },
                Refusal::UnknownTarget,
            );
            adapter.refuse(
                UiAction::LoadPreset {
                    id: missing.clone(),
                },
                Refusal::UnknownTarget,
            );
            adapter.refuse(
                UiAction::RenamePreset {
                    id: missing.clone(),
                    name: "Other".to_owned(),
                },
                Refusal::UnknownTarget,
            );
            adapter.refuse(
                UiAction::DeletePreset { id: missing },
                Refusal::UnknownTarget,
            );
        }
    });
}

#[test]
fn midi_requests_follow_the_same_limits() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        adapter.refuse(UiAction::Midi(MidiAction::SetChannel(17)), Refusal::Invalid);
        adapter.refuse(
            UiAction::Midi(MidiAction::SetMapping {
                target_id: "outputLevel".to_owned(),
                source: MidiSource::Cc,
                number: 128,
            }),
            Refusal::Invalid,
        );

        adapter.accept(UiAction::Midi(MidiAction::SetChannel(16)));
        assert_eq!(backend.midi_map().channel, 16);

        adapter.accept(UiAction::Midi(MidiAction::StartLearn(
            "outputLevel".to_owned(),
        )));
        adapter.accept(UiAction::Midi(MidiAction::SetDraft(Some("12".to_owned()))));
        assert_eq!(backend.midi_map().learn_target_id, "outputLevel");
        assert_eq!(backend.midi_map().cc_draft.as_deref(), Some("12"));

        for number in [7, 8] {
            adapter.accept(UiAction::Midi(MidiAction::SetMapping {
                target_id: "outputLevel".to_owned(),
                source: MidiSource::Cc,
                number,
            }));
        }
        let map = backend.midi_map();
        assert_eq!(map.mappings.len(), 1);
        assert_eq!(map.mappings[0].number, 8);
        assert_eq!(map.learn_target_id, "");
        assert_eq!(map.cc_draft, None);
        assert_eq!(backend.chain_state(0).session.midi_mappings, map.mappings);

        adapter.accept(UiAction::Midi(MidiAction::RemoveMapping(
            "outputLevel".to_owned(),
        )));
        assert!(backend.midi_map().mappings.is_empty());
    });
}

#[test]
fn only_a_machine_default_can_be_persisted() {
    for_each_adapter(|adapter| {
        adapter.refuse(
            UiAction::PersistMachineDefault(ParameterId("noSuchParameter")),
            Refusal::UnknownParameter,
        );
        adapter.refuse(
            UiAction::PersistMachineDefault(ParameterId("outputLevel")),
            Refusal::Invalid,
        );
        assert_eq!(
            adapter.accept(UiAction::PersistMachineDefault(ParameterId(
                "inputCalibrationLevel"
            ))),
            UiEffect::ParameterChanged
        );
    });
}

#[test]
fn ui_only_values_and_host_requests_are_accepted() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        assert_eq!(backend.preference("contract.key"), None);
        assert_eq!(
            adapter.accept(UiAction::SetPreference {
                key: "contract.key".to_owned(),
                value: true,
            }),
            UiEffect::Navigation
        );
        assert_eq!(backend.preference("contract.key"), Some(true));

        adapter.accept(UiAction::SetCalibrationDraft(Some("3.5".to_owned())));
        assert_eq!(backend.calibration_draft().as_deref(), Some("3.5"));
        adapter.accept(UiAction::SetCalibrationDraft(None));
        assert_eq!(backend.calibration_draft(), None);

        for request in [
            NavigationAction::OpenMicrophonePrivacy,
            NavigationAction::OpenPresetsFolder,
            NavigationAction::CopyLogs,
            NavigationAction::RevealLogs,
        ] {
            assert_eq!(
                adapter.accept(UiAction::Navigate(request)),
                UiEffect::Navigation
            );
        }
        for request in [
            DeviceAction::PlayTestTone,
            DeviceAction::OpenControlPanel,
            DeviceAction::OpenBluetoothMidi,
            DeviceAction::SetHearYourself(true),
            DeviceAction::SetOutputPair(0),
        ] {
            assert_eq!(
                adapter.accept(UiAction::Device(request)),
                UiEffect::DeviceChanged
            );
        }
    });
}

#[test]
fn a_real_adapter_does_not_offer_a_control_that_has_no_action() {
    for_each_adapter(|adapter| {
        // Only a stream can play a test tone, and no adapter has an automatic balance.
        assert_eq!(
            adapter.backend().supports(HostControl::TestTone),
            adapter.has(Gap::MockOffersEachControl) || !adapter.has(Gap::OnlyStandaloneHasAudio),
        );
        assert_eq!(
            adapter.backend().supports(HostControl::AutoBalance),
            adapter.has(Gap::MockOffersEachControl),
        );
    });
}

#[test]
fn a_model_check_refuses_bytes_that_cannot_compile() {
    for_each_adapter(|adapter| {
        let result = adapter
            .backend()
            .validate_model(&tone(1), ModelId(10), &bytes());
        assert_eq!(
            result.is_ok(),
            adapter.has(Gap::MockCompilesNoModel),
            "{result:?}"
        );
    });
}

#[test]
fn only_a_standalone_adapter_reports_a_device_and_chain_faults() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let block = adapter.load(0, 1);
        adapter.accept(UiAction::SetBlockParams {
            block_id: block,
            params: BlockParams::default(),
        });
        let faults = backend.chain_faults();
        if adapter.has(Gap::OnlyStandaloneHasAudio) {
            assert_eq!(backend.device_settings(), None);
            assert_eq!(faults, Vec::new());
            return;
        }
        assert!(
            backend
                .device_settings()
                .is_some_and(|device| device.banner.device_open)
        );
        adapter.accept(UiAction::Device(DeviceAction::SetHearYourself(true)));
        assert!(
            backend
                .device_settings()
                .is_some_and(|device| device.banner.hear_yourself)
        );
        assert_eq!(
            faults.iter().map(|fault| fault.kind).collect::<Vec<_>>(),
            vec![ChainFaultKind::MissingModel],
            "a tone with no bytes runs as bypass"
        );
    });
}

/// Saves a preset and returns its identity, which is active after the save.
fn save(adapter: &Adapter, name: &str) -> String {
    assert_eq!(
        adapter.accept(UiAction::SavePreset {
            name: name.to_owned(),
        }),
        UiEffect::PresetSaved
    );
    adapter
        .backend()
        .chain_state(0)
        .session
        .active_preset
        .clone()
        .expect("active preset")
        .id
}

#[test]
fn a_preset_rename_needs_a_name() {
    for_each_adapter(|adapter| {
        let id = save(adapter, "Named");
        for name in ["", "   "] {
            adapter.refuse(
                UiAction::RenamePreset {
                    id: id.clone(),
                    name: name.to_owned(),
                },
                Refusal::Invalid,
            );
        }
    });
}

#[test]
fn an_edit_of_another_preset_keeps_the_active_preset() {
    for_each_adapter(|adapter| {
        let other = save(adapter, "Other");
        save(adapter, "Active");
        let active = || {
            adapter
                .backend()
                .chain_state(0)
                .session
                .active_preset
                .clone()
        };
        let before = active().expect("active preset");

        adapter.accept(UiAction::RenamePreset {
            id: other.clone(),
            name: "Renamed".to_owned(),
        });
        assert_eq!(active().as_ref(), Some(&before));
        adapter.accept(UiAction::DeletePreset { id: other });
        assert_eq!(active().as_ref(), Some(&before));
    });
}

#[test]
fn a_preset_move_needs_a_distance() {
    for_each_adapter(|adapter| {
        let id = save(adapter, "Still");
        adapter.refuse(UiAction::MovePreset { id, delta: 0 }, Refusal::Invalid);
    });
}

#[test]
fn a_preset_move_at_the_edge_changes_nothing() {
    for_each_adapter(|adapter| {
        let id = save(adapter, "Only");
        let backend = adapter.backend();
        for delta in [-1, 1, -100] {
            let before = backend.chain_state(0);
            let receipt = backend
                .dispatch(UiAction::MovePreset {
                    id: id.clone(),
                    delta,
                })
                .expect("a move at the edge is accepted");
            assert_eq!(receipt.effect, UiEffect::None, "{delta}");
            assert_eq!(receipt.revision, before.revision, "{delta}");
            assert_eq!(backend.chain_state(0).revision, before.revision, "{delta}");
        }
    });
}

#[test]
fn a_preset_edit_reports_a_saved_preset() {
    for_each_adapter(|adapter| {
        let first = save(adapter, "First");
        let second = save(adapter, "Second");
        for action in [
            UiAction::RenamePreset {
                id: first.clone(),
                name: "Renamed".to_owned(),
            },
            UiAction::MovePreset {
                id: second.clone(),
                delta: -1,
            },
            UiAction::DeletePreset { id: first },
        ] {
            assert_eq!(adapter.accept(action), UiEffect::PresetSaved);
        }
    });
}

#[test]
fn a_device_configuration_reaches_the_preparation() {
    for_each_adapter(|adapter| {
        let before = adapter.backend().chain_state(0).preparation;
        let rate = if before.host_sample_rate == 96_000.0 {
            44_100.0
        } else {
            96_000.0
        };
        let frames = if before.max_host_frames == 256 {
            512
        } else {
            256
        };
        assert_eq!(
            adapter.accept(UiAction::Device(DeviceAction::Configure(
                AudioDeviceConfig {
                    sample_rate: Some(rate),
                    buffer_frames: Some(frames),
                    ..Default::default()
                }
            ))),
            UiEffect::DeviceChanged
        );
        if !adapter.has(Gap::OnlyStandaloneHasAudio) {
            // The format of the stream that opens replaces the request, so the device decides.
            return;
        }
        let preparation = adapter.backend().chain_state(0).preparation;
        assert_eq!(preparation.host_sample_rate, rate);
        assert_eq!(preparation.max_host_frames, frames as usize);
    });
}

#[test]
fn a_device_configuration_is_no_undo_step() {
    for_each_adapter(|adapter| {
        adapter.accept(UiAction::SetMultiCore(false));
        adapter.accept(UiAction::Undo);
        let before = adapter.backend().chain_state(0);
        assert!(!before.can_undo && before.can_redo);
        adapter.accept(UiAction::Device(DeviceAction::Configure(
            AudioDeviceConfig {
                buffer_frames: Some(256),
                ..Default::default()
            },
        )));
        let after = adapter.backend().chain_state(0);
        assert_eq!(after.can_undo, before.can_undo);
        assert_eq!(after.can_redo, before.can_redo);
    });
}

#[test]
fn a_device_configuration_with_no_usable_rate_or_size_is_refused() {
    for_each_adapter(|adapter| {
        for (sample_rate, buffer_frames) in
            [(Some(0.0), None), (Some(f64::NAN), None), (None, Some(0))]
        {
            adapter.refuse(
                UiAction::Device(DeviceAction::Configure(AudioDeviceConfig {
                    sample_rate,
                    buffer_frames,
                    ..Default::default()
                })),
                Refusal::Invalid,
            );
        }
    });
}

#[test]
fn a_cancelled_gesture_through_the_action_restores_the_value() {
    for_each_adapter(|adapter| {
        let start = adapter.value("outputLevel");
        for (value, gesture) in [
            (start, GesturePhase::Begin),
            (0.1, GesturePhase::Change),
            (0.1, GesturePhase::Cancel),
        ] {
            assert_eq!(
                adapter.accept(UiAction::SetParameter {
                    id: ParameterId("outputLevel"),
                    value: ParameterValue(value),
                    gesture,
                }),
                UiEffect::ParameterChanged
            );
        }
        assert_eq!(adapter.value("outputLevel"), start);
        assert!(!adapter.backend().chain_state(0).can_undo);
    });
}

#[test]
fn each_view_of_an_unchanged_session_shares_one_snapshot() {
    for_each_adapter(|adapter| {
        // The audio worker of a standalone adapter can set a new stream format at any time, and
        // that change correctly makes a new snapshot. So only the other adapters are exact here.
        if adapter.kind == Kind::Standalone {
            return;
        }
        let backend = adapter.backend();
        adapter.load(0, 1);
        let first = backend.chain_state(0);
        for known in [0, first.revision, u64::MAX] {
            let next = backend.chain_state(known);
            assert!(std::sync::Arc::ptr_eq(&first.chain, &next.chain));
            assert!(std::sync::Arc::ptr_eq(&first.session, &next.session));
        }
        adapter.load(1, 2);
        let after = backend.chain_state(0);
        assert!(!std::sync::Arc::ptr_eq(&first.chain, &after.chain));
        assert!(first.chain.left[1].is_insert, "a kept view does not change");
        assert!(!after.chain.left[1].is_insert);
    });
}

#[test]
fn each_field_of_the_next_view_shows_the_last_action() {
    for_each_adapter(|adapter| {
        let backend = adapter.backend();
        let fresh = |step: &str, shows: &dyn Fn(&ChainViewState) -> bool| {
            let view = backend.chain_state(0);
            assert!(shows(&view), "{step}");
            assert_eq!(view.session.chain, *view.chain, "{step}");
        };
        fresh("a new session", &|view| !view.can_undo && !view.can_redo);
        adapter.accept(UiAction::SetMultiCore(false));
        fresh("multicore", &|view| !view.multi_core && view.can_undo);
        adapter.accept(UiAction::SetNamFullSize(true));
        fresh("model size", &|view| view.nam_full_size);
        adapter.accept(UiAction::Undo);
        fresh("undo", &|view| !view.nam_full_size && view.can_redo);
        adapter.accept(UiAction::Redo);
        fresh("redo", &|view| view.nam_full_size && !view.can_redo);
        adapter.accept(UiAction::SetInputMode(InputMode::Right));
        fresh("input mode", &|view| {
            view.session.input_mode == InputMode::Right
                && view.preparation.input_mode == InputMode::Right
        });
        let id = save(adapter, "Lead");
        adapter.accept(UiAction::RenamePreset {
            id,
            name: "Solo".to_owned(),
        });
        fresh("a session-only rename", &|view| {
            view.session
                .active_preset
                .as_ref()
                .is_some_and(|preset| preset.name == "Solo")
        });
        adapter.accept(UiAction::Midi(MidiAction::SetMapping {
            target_id: "outputLevel".to_owned(),
            source: MidiSource::Cc,
            number: 7,
        }));
        fresh("a MIDI mapping", &|view| {
            view.session.midi_mappings.len() == 1
        });
    });
}
