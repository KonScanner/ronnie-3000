//! Catalog/local load targeting, cancellation, retry, and offline tests.

use std::{
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use ronie_core::{
    ChainSide, ModelBytes, ModelId,
    params::{ParameterId, ParameterValue},
};
use ronie_ui::{
    backend::{GesturePhase, MockBackend, UiAction, UiBackend, UiError},
    default::{LoadFlow, LoadPhase},
    testbed::{UiScenario, load_scenarios},
};

fn scenario(id: &str) -> UiScenario {
    load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .find(|scenario| scenario.id == id)
        .expect("scenario exists")
}

fn mock_backend(id: &str) -> Arc<MockBackend> {
    Arc::new(MockBackend::from_scenario(&scenario(id)))
}

fn tone() -> ronie_core::Tone {
    mock_backend("main-detail").chain_state(0).chain.left[0]
        .tone
        .clone()
        .expect("tone")
}

fn insert(backend: &MockBackend) -> String {
    backend
        .chain_state(0)
        .chain
        .left
        .last()
        .expect("insert")
        .id
        .0
        .clone()
}

#[test]
fn selected_insert_receives_the_loaded_tone_and_closes_browser() {
    let backend = mock_backend("main-empty");
    let target = insert(&backend);
    let mut flow = LoadFlow::new(backend.clone());
    flow.select(ChainSide::Left, &target);
    assert!(flow.browser_open());
    let generation = flow.begin_load(tone()).expect("load");
    assert_eq!(flow.phase(), LoadPhase::Loading);
    assert!(flow.complete_load(generation, Ok(tone())));
    assert_eq!(flow.phase(), LoadPhase::Ready);
    assert!(!flow.browser_open());
    assert!(
        backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .any(|block| block.tone.is_some())
    );
}

#[test]
fn cancel_and_replacement_reject_stale_completions() {
    let backend = mock_backend("main-empty");
    let target = insert(&backend);
    let mut flow = LoadFlow::new(backend.clone());
    flow.select(ChainSide::Left, &target);
    let first = flow.begin_load(tone()).expect("first");
    let second = flow.begin_load(tone()).expect("second");
    assert!(second > first);
    assert!(!flow.complete_load(first, Ok(tone())));
    flow.cancel();
    assert_eq!(flow.phase(), LoadPhase::Cancelled);
    assert!(!flow.complete_load(second, Ok(tone())));
    assert!(
        !backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .any(|block| block.tone.is_some())
    );
}

#[test]
fn failed_load_retries_after_refresh_without_repeating_backend_mutation() {
    let backend = mock_backend("main-empty");
    let target = insert(&backend);
    let mut flow = LoadFlow::new(backend.clone());
    flow.select(ChainSide::Left, &target);
    let generation = flow.begin_load(tone()).expect("load");
    assert!(flow.complete_load(generation, Err(UiError::Control("unauthorized".to_owned()))));
    assert_eq!(flow.phase(), LoadPhase::Failed);
    assert!(flow.refresh());
    let retry = flow.retry().expect("retry");
    assert!(retry > generation);
    let revision = backend.chain_state(0).revision;
    assert!(flow.complete_load(retry, Ok(tone())));
    assert_eq!(backend.chain_state(0).revision, revision + 1);
}

#[test]
fn offline_cached_load_succeeds_but_failed_uncached_load_offers_retry() {
    let backend = mock_backend("main-empty");
    let target = insert(&backend);
    let mut flow = LoadFlow::new(backend.clone());
    flow.select(ChainSide::Left, &target);
    flow.set_offline(true);
    let generation = flow.begin_load(tone()).expect("cached");
    assert!(flow.complete_cached(generation, tone()));
    assert_eq!(flow.phase(), LoadPhase::Ready);

    let backend = mock_backend("main-empty");
    let target = insert(&backend);
    let mut flow = LoadFlow::new(backend.clone());
    flow.select(ChainSide::Left, &target);
    flow.set_offline(true);
    let generation = flow.begin_load(tone()).expect("uncached");
    assert!(flow.complete_load(generation, Err(UiError::Control("offline".to_owned()))));
    assert_eq!(flow.phase(), LoadPhase::Failed);
}

#[test]
fn close_rejects_every_late_callback() {
    let backend = mock_backend("main-empty");
    let target = insert(&backend);
    let mut flow = LoadFlow::new(backend.clone());
    flow.select(ChainSide::Left, &target);
    let generation = flow.begin_load(tone()).expect("load");
    flow.close();
    assert!(!flow.complete_load(generation, Ok(tone())));
    assert!(!flow.complete_cached(generation, tone()));
    assert!(
        !backend
            .chain_state(0)
            .chain
            .left
            .iter()
            .any(|block| block.tone.is_some())
    );
}

#[test]
fn tones_without_models_are_rejected_before_any_backend_mutation() {
    let backend = mock_backend("main-empty");
    let target = insert(&backend);
    let mut flow = LoadFlow::new(backend.clone());
    flow.select(ChainSide::Left, &target);
    let mut empty = tone();
    empty.models.clear();
    let revision = backend.chain_state(0).revision;
    assert!(flow.begin_load(empty).is_err());
    assert_eq!(backend.chain_state(0).revision, revision);
}

#[test]
fn loading_scenarios_keep_load_state_explicit() {
    for scenario in load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenarios")
        .into_iter()
        .filter(|scenario| scenario.section == ronie_ui::testbed::UiSection::Loading)
    {
        let backend = mock_backend(&scenario.id);
        let target = insert(&backend);
        let mut flow = LoadFlow::new(backend);
        flow.select(ChainSide::Left, &target);
        if scenario.state["offlineAfterLoad"]
            .as_bool()
            .unwrap_or(false)
        {
            flow.set_offline(true);
        }
        let generation = flow.begin_load(tone()).expect("scenario load");
        assert_eq!(flow.phase(), LoadPhase::Loading);
        flow.cancel();
        assert_eq!(flow.phase(), LoadPhase::Cancelled);
        assert!(!flow.complete_load(generation, Ok(tone())));
    }
}

/// A backend whose hooks run at the steps of a publication.
#[derive(Default)]
struct Hooks {
    validate: Option<Box<dyn Fn() -> Result<(), UiError> + Send + Sync>>,
    dispatch: Option<Box<dyn Fn() + Send + Sync>>,
    settle: Option<Box<dyn Fn() + Send + Sync>>,
}

struct Probe {
    inner: Arc<MockBackend>,
    hooks: Hooks,
}

impl UiBackend for Probe {
    fn chain_state(&self, known_revision: u64) -> ronie_ui::backend::ChainViewState {
        self.inner.chain_state(known_revision)
    }

    fn meter_levels(&self) -> ronie_ui::backend::MeterSnapshot {
        self.inner.meter_levels()
    }

    fn dispatch(&self, action: UiAction) -> Result<ronie_ui::backend::UiReceipt, UiError> {
        if let Some(hook) = &self.hooks.dispatch {
            hook();
        }
        self.inner.dispatch(action)
    }

    fn validate_model(
        &self,
        _tone: &ronie_core::Tone,
        _model_id: ModelId,
        _bytes: &ModelBytes,
    ) -> Result<(), UiError> {
        self.hooks.validate.as_ref().map_or(Ok(()), |hook| hook())
    }

    fn settle_audio(&self) -> Result<(), UiError> {
        if let Some(hook) = &self.hooks.settle {
            hook();
        }
        Ok(())
    }

    fn parameter(&self, id: ParameterId) -> Option<ParameterValue> {
        self.inner.parameter(id)
    }

    fn set_parameter(
        &self,
        id: ParameterId,
        value: ParameterValue,
        gesture: GesturePhase,
    ) -> Result<(), UiError> {
        self.inner.set_parameter(id, value, gesture)
    }
}

/// One way to stop a load, with a name for the assertion text.
type NamedCancel = (&'static str, fn(&mut LoadFlow));

fn remote_tone() -> ronie_core::Tone {
    ronie_core::Tone::from_value(serde_json::json!({
        "id": 5,
        "title": "Remote",
        "format": "nam",
        "models": [{"id": 7, "name": "Amp", "model_url": "https://models.example/7.nam"}],
    }))
    .expect("tone")
}

fn bytes() -> ModelBytes {
    ModelBytes::from(b"model".to_vec())
}

fn publications(backend: &MockBackend) -> usize {
    backend
        .actions()
        .iter()
        .filter(|action| matches!(action, UiAction::LoadToneWithBytes { .. }))
        .count()
}

fn flow_with(hooks: Hooks) -> (LoadFlow, Arc<MockBackend>) {
    let inner = mock_backend("main-empty");
    let target = insert(&inner);
    let mut flow = LoadFlow::new(Arc::new(Probe {
        inner: inner.clone(),
        hooks,
    }));
    flow.select(ChainSide::Left, &target);
    (flow, inner)
}

#[test]
fn a_remote_load_starts_for_a_catalog_row_that_has_no_models() {
    let (mut flow, _backend) = flow_with(Hooks::default());
    let mut row = remote_tone();
    row.models.clear();

    let ticket = flow.begin_remote_load(row.clone()).expect("load");

    assert_eq!(ticket.generation(), flow.generation());
    assert_eq!(flow.phase(), LoadPhase::Loading);
    assert_eq!(flow.loading_tone_id(), Some(row.id));
}

#[test]
fn a_remote_load_needs_a_target() {
    let backend = mock_backend("main-empty");
    let mut flow = LoadFlow::new(backend);
    assert!(flow.begin_remote_load(remote_tone()).is_err());
}

#[test]
fn a_current_ticket_puts_the_bytes_in_the_selected_slot() {
    let (mut flow, backend) = flow_with(Hooks::default());
    let target = flow.pending_target().expect("target").block_id.clone();
    let ticket = flow.begin_remote_load(remote_tone()).expect("load");

    assert!(
        ticket
            .publish(remote_tone(), ModelId(7), bytes())
            .expect("publish")
    );

    let actions = backend.actions();
    let [
        UiAction::LoadToneWithBytes {
            target_insert_id,
            model_id,
            bytes: published,
            ..
        },
    ] = actions.as_slice()
    else {
        panic!("{} actions", actions.len());
    };
    assert_eq!(target_insert_id.0, target);
    assert_eq!(*model_id, ModelId(7));
    assert_eq!(published.as_ref(), b"model");
    assert_eq!(
        flow.phase(),
        LoadPhase::Loading,
        "the UI thread ends the load"
    );
    assert!(flow.finish_remote(ticket.generation()));
    assert_eq!(flow.phase(), LoadPhase::Ready);
    assert!(!flow.browser_open());
}

#[test]
fn a_ticket_that_lost_its_generation_publishes_nothing() {
    let cancels: [NamedCancel; 4] = [
        ("close", LoadFlow::close),
        ("cancel", LoadFlow::cancel),
        ("session change", LoadFlow::abandon),
        ("newer load", |flow| {
            flow.begin_remote_load(remote_tone()).expect("newer");
        }),
    ];
    for (name, cancel) in cancels {
        let (mut flow, backend) = flow_with(Hooks::default());
        let ticket = flow.begin_remote_load(remote_tone()).expect("load");

        cancel(&mut flow);

        assert!(
            !ticket
                .publish(remote_tone(), ModelId(7), bytes())
                .expect("a late completion is not an error"),
            "{name}"
        );
        assert_eq!(publications(&backend), 0, "{name}");
        assert!(!flow.finish_remote(ticket.generation()), "{name}");
        assert!(!flow.fail_load(ticket.generation()), "{name}");
    }
}

#[test]
fn a_ticket_that_passed_validation_still_loses_to_a_cancellation() {
    let cancels: [NamedCancel; 3] = [
        ("close", LoadFlow::close),
        ("session change", LoadFlow::abandon),
        ("newer load", |flow| {
            flow.begin_remote_load(remote_tone()).expect("newer");
        }),
    ];
    for (name, cancel) in cancels {
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let (mut flow, backend) = flow_with(Hooks {
            validate: Some(Box::new(move || {
                entered_tx.send(()).expect("test waits");
                release_rx.lock().expect("lock").recv().expect("release");
                Ok(())
            })),
            ..Hooks::default()
        });
        let ticket = flow.begin_remote_load(remote_tone()).expect("load");

        let publisher = std::thread::spawn(move || {
            ticket
                .publish(remote_tone(), ModelId(7), bytes())
                .expect("a late completion is not an error")
        });
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("the compile started");
        cancel(&mut flow);
        release.send(()).expect("release");

        assert!(!publisher.join().expect("publisher"), "{name}");
        assert_eq!(publications(&backend), 0, "{name}");
    }
}

#[test]
fn bytes_that_cannot_compile_keep_the_chain_and_the_load_can_retry() {
    let (mut flow, backend) = flow_with(Hooks {
        validate: Some(Box::new(|| Err(UiError::Control("not a model".to_owned())))),
        ..Hooks::default()
    });
    let revision = backend.chain_state(0).revision;
    let ticket = flow.begin_remote_load(remote_tone()).expect("load");

    assert!(ticket.publish(remote_tone(), ModelId(7), bytes()).is_err());

    assert_eq!(publications(&backend), 0);
    assert_eq!(backend.chain_state(0).revision, revision);
    assert!(flow.fail_load(ticket.generation()));
    assert_eq!(flow.phase(), LoadPhase::Failed);
    assert_eq!(flow.loading_tone_id(), None, "the card is not busy");
    let (retry, tone) = flow.retry_remote().expect("retry");
    assert!(retry.generation() > ticket.generation());
    assert_eq!(tone, remote_tone());
    assert_eq!(flow.phase(), LoadPhase::Loading);
}

#[test]
fn only_a_failed_load_can_retry_in_the_background() {
    let (mut flow, _backend) = flow_with(Hooks::default());
    assert!(flow.retry_remote().is_err(), "nothing was started");
    flow.begin_remote_load(remote_tone()).expect("load");
    assert!(flow.retry_remote().is_err(), "the load is still active");
}

#[test]
fn a_session_change_keeps_the_target_and_the_open_browser() {
    let (mut flow, _backend) = flow_with(Hooks::default());
    let ticket = flow.begin_remote_load(remote_tone()).expect("load");

    flow.abandon();

    assert_eq!(flow.phase(), LoadPhase::Idle);
    assert!(flow.browser_open());
    assert!(flow.pending_target().is_some());
    assert!(flow.generation() > ticket.generation());
}

#[test]
fn a_new_target_stops_the_load_of_the_old_target() {
    let (mut flow, backend) = flow_with(Hooks::default());
    let ticket = flow.begin_remote_load(remote_tone()).expect("load");

    flow.select(ChainSide::Right, "another-slot");

    assert!(
        !ticket
            .publish(remote_tone(), ModelId(7), bytes())
            .expect("late")
    );
    assert_eq!(publications(&backend), 0);
}

#[test]
fn revoking_an_old_ticket_does_not_stop_the_newer_load() {
    let (mut flow, backend) = flow_with(Hooks::default());
    let old = flow.begin_remote_load(remote_tone()).expect("old");
    let new = flow.begin_remote_load(remote_tone()).expect("new");

    old.revoke();

    assert!(
        new.publish(remote_tone(), ModelId(7), bytes())
            .expect("new")
    );
    assert_eq!(publications(&backend), 1);
}

#[test]
fn a_close_that_returned_is_never_followed_by_a_publication() {
    for _ in 0..100 {
        let closed = Arc::new(AtomicBool::new(false));
        let late = Arc::new(AtomicBool::new(false));
        let (seen_closed, seen_late) = (closed.clone(), late.clone());
        let (mut flow, _backend) = flow_with(Hooks {
            dispatch: Some(Box::new(move || {
                std::thread::yield_now();
                if seen_closed.load(Ordering::SeqCst) {
                    seen_late.store(true, Ordering::SeqCst);
                }
            })),
            ..Hooks::default()
        });
        let ticket = flow.begin_remote_load(remote_tone()).expect("load");
        let start = Arc::new(Barrier::new(2));
        let publisher_start = start.clone();
        let publisher = std::thread::spawn(move || {
            publisher_start.wait();
            ticket
                .publish(remote_tone(), ModelId(7), bytes())
                .expect("publish")
        });

        start.wait();
        flow.close();
        closed.store(true, Ordering::SeqCst);

        publisher.join().expect("publisher");
        assert!(!late.load(Ordering::SeqCst), "the close won, then the load");
    }
}

#[test]
fn a_close_waits_for_a_publication_that_already_passed_the_check() {
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let (mut flow, backend) = flow_with(Hooks {
        dispatch: Some(Box::new(move || {
            entered_tx.send(()).expect("test waits");
            release_rx.lock().expect("lock").recv().expect("release");
        })),
        ..Hooks::default()
    });
    let ticket = flow.begin_remote_load(remote_tone()).expect("load");
    let publisher = std::thread::spawn(move || {
        ticket
            .publish(remote_tone(), ModelId(7), bytes())
            .expect("publish")
    });
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("the dispatch started");

    let (closed_tx, closed) = mpsc::channel();
    let closer = std::thread::spawn(move || {
        flow.close();
        closed_tx.send(()).expect("test waits");
    });

    assert!(
        closed.recv_timeout(Duration::from_millis(50)).is_err(),
        "the close must order after the publication that it cannot stop"
    );
    release.send(()).expect("release");
    closed.recv().expect("the close ended");
    closer.join().expect("closer");
    assert!(publisher.join().expect("publisher"));
    assert_eq!(publications(&backend), 1);
}

#[test]
fn a_compile_does_not_stop_a_close() {
    for compile_in_settle in [false, true] {
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let park = move || {
            entered_tx.send(()).expect("test waits");
            release_rx.lock().expect("lock").recv().expect("release");
        };
        let hooks = if compile_in_settle {
            Hooks {
                settle: Some(Box::new(park)),
                ..Hooks::default()
            }
        } else {
            Hooks {
                validate: Some(Box::new(move || {
                    park();
                    Ok(())
                })),
                ..Hooks::default()
            }
        };
        let (mut flow, _backend) = flow_with(hooks);
        let ticket = flow.begin_remote_load(remote_tone()).expect("load");
        let publisher =
            std::thread::spawn(move || ticket.publish(remote_tone(), ModelId(7), bytes()));
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("the compile started");

        let (closed_tx, closed) = mpsc::channel();
        let closer = std::thread::spawn(move || {
            flow.close();
            closed_tx.send(()).expect("test waits");
        });

        closed
            .recv_timeout(Duration::from_secs(10))
            .expect("a frame must not wait for a compile");
        release.send(()).expect("release");
        closer.join().expect("closer");
        publisher.join().expect("publisher").expect("result");
    }
}

#[test]
fn the_debug_text_of_a_byte_action_holds_no_model_content() {
    let action = UiAction::LoadToneWithBytes {
        target_insert_id: ronie_core::BlockId("slot".to_owned()),
        tone: remote_tone(),
        model_id: ModelId(7),
        bytes: ModelBytes::from(vec![201; 8]),
    };

    let text = format!("{action:?}");

    assert!(!text.contains("201"), "{text}");
    assert!(text.contains("8 bytes"), "{text}");
}

#[test]
fn a_ticket_of_a_flow_that_no_longer_exists_publishes_nothing() {
    let (mut flow, backend) = flow_with(Hooks::default());
    let ticket = flow.begin_remote_load(remote_tone()).expect("load");

    drop(flow);

    assert!(
        !ticket
            .publish(remote_tone(), ModelId(7), bytes())
            .expect("late")
    );
    assert_eq!(publications(&backend), 0);
}

// ------------------------------------------------------------ The two IRs of a cab

/// A catalog IR pack as a block stores it after a download of `model`: only that model.
fn stored_cab(model: i64) -> ronie_core::Tone {
    ronie_core::Tone::from_value(serde_json::json!({
        "id": 7,
        "title": "Cab",
        "format": "ir",
        "models": [{
            "id": model,
            "name": format!("Mic {model}"),
            "model_url": format!("https://models.example/{model}.wav"),
        }],
    }))
    .expect("cab")
}

fn cab_slots(backend: &MockBackend, block: &ronie_core::BlockId) -> (Option<i64>, Option<i64>) {
    let chain = backend.chain_state(0).chain;
    let block = chain.block(block).expect("block");
    (
        block.active_model_id.map(|id| id.0),
        block.ir_b.map(|ir_b| ir_b.model_id.0),
    )
}

/// The switches of A and B share one generation, so a newer switch of either slot ends the
/// other. A ticket still names its slot: A's bytes never land in B, nor B's in A.
#[test]
fn switching_a_while_b_downloads_lands_each_in_its_slot() {
    use ronie_ui::services::{ModelSlot, ModelSwitchFlow};

    let backend = mock_backend("main-empty");
    let block = ronie_core::BlockId(insert(&backend));
    backend
        .dispatch(UiAction::LoadTone {
            target_insert_id: block.clone(),
            tone: stored_cab(71),
            model_id: ModelId(71),
        })
        .expect("cab");
    let flow = ModelSwitchFlow::new(backend.clone());

    // B starts to download, and then the user switches A.
    let b = flow.begin_slot(block.clone(), ModelSlot::B, None);
    let a = flow.begin_slot(block.clone(), ModelSlot::A, Some(ModelId(71)));
    assert_eq!((a.slot(), b.slot()), (ModelSlot::A, ModelSlot::B));
    assert!(
        !b.publish(stored_cab(72), ModelId(72), bytes())
            .expect("late B")
    );
    assert_eq!(cab_slots(&backend, &block), (Some(71), None));
    assert!(a.publish(stored_cab(73), ModelId(73), bytes()).expect("A"));
    assert_eq!(cab_slots(&backend, &block), (Some(73), None));

    // B again, with no switch after it, lands in B and leaves A.
    let b = flow.begin_slot(block.clone(), ModelSlot::B, None);
    assert!(b.publish(stored_cab(72), ModelId(72), bytes()).expect("B"));
    assert_eq!(cab_slots(&backend, &block), (Some(73), Some(72)));

    // A switch of A leaves B, and a B ticket that names another B publishes nothing.
    let a = flow.begin_slot(block.clone(), ModelSlot::A, Some(ModelId(73)));
    assert!(a.publish(stored_cab(71), ModelId(71), bytes()).expect("A"));
    assert_eq!(cab_slots(&backend, &block), (Some(71), Some(72)));
    let b = flow.begin_slot(block.clone(), ModelSlot::B, Some(ModelId(73)));
    assert!(
        !b.publish(stored_cab(74), ModelId(74), bytes())
            .expect("B of another B")
    );
    assert_eq!(cab_slots(&backend, &block), (Some(71), Some(72)));

    let kinds: Vec<&str> = backend
        .actions()
        .iter()
        .filter_map(|action| match action {
            UiAction::SelectModelWithBytes { model_id, .. } => {
                Some(if model_id.0 == 72 { "A got B" } else { "A" })
            }
            UiAction::SelectSecondModelWithBytes { model_id, .. } => {
                Some(if model_id.0 == 72 { "B" } else { "B got A" })
            }
            _ => None,
        })
        .collect();
    assert_eq!(kinds, ["A", "B", "A"]);
}
