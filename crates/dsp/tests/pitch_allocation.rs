//! Prepared input-stage allocation tests.
#![allow(unsafe_code)]

use ronie_dsp::{
    PlanarBuffer,
    input::{InputControls, InputStage, NoiseGate, PitchParams, PitchShifter, PitchWindow},
};

thread_local! {
    static ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DEALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: every method forwards the original contract and only adds counters.
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the layout is forwarded unchanged.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        DEALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the pointer and layout are the allocator's original pair.
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn counts(operation: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    operation();
    (
        ALLOCATIONS.with(std::cell::Cell::get),
        DEALLOCATIONS.with(std::cell::Cell::get),
    )
}

#[test]
fn prepared_input_processing_and_parameter_changes_do_not_allocate() {
    let mut pitch = PitchShifter::new(48_000.0, 512).expect("pitch preparation");
    pitch.set_enabled(true);
    pitch.set_params(PitchParams {
        semitones: -12.0,
        tonality_hz: 2000.0,
        window: PitchWindow::Ms30,
    });
    let mut gate = NoiseGate::new(48_000.0).expect("gate preparation");
    let mut stage = InputStage::new(48_000.0, 512).expect("input preparation");
    let mut stereo = [0.2; 32];
    let mut mono = vec![0.1; 4096];
    let mut small = [0.25; 16];
    let mut pitch_result = Ok(());
    let mut large_result = Ok(());
    let mut gate_result = Ok(());
    let mut stage_result = Ok(());

    let counts = counts(|| {
        pitch_result =
            pitch.process(&mut PlanarBuffer::new(&mut stereo, 2, 16).expect("stereo buffer"));
        large_result =
            pitch.process(&mut PlanarBuffer::new(&mut mono, 1, 4096).expect("large buffer"));
        pitch.set_params(PitchParams {
            semitones: 24.0,
            tonality_hz: 0.0,
            window: PitchWindow::Ms20,
        });
        gate_result = gate.process(&mut PlanarBuffer::new(&mut small, 1, 16).expect("gate buffer"));
        stage_result = stage.process(
            &mut PlanarBuffer::new(&mut small, 1, 16).expect("input buffer"),
            InputControls {
                pitch_enabled: true,
                ..Default::default()
            },
        );
    });

    assert!(pitch_result.is_ok(), "{pitch_result:?}");
    assert!(large_result.is_ok(), "{large_result:?}");
    assert!(gate_result.is_ok(), "{gate_result:?}");
    assert!(stage_result.is_ok(), "{stage_result:?}");
    assert_eq!(counts, (0, 0));
}

#[test]
fn sixteen_frame_search_cost_is_bounded_and_measured() {
    let mut pitch = PitchShifter::new(48_000.0, 512).expect("pitch preparation");
    pitch.set_enabled(true);
    pitch.set_params(PitchParams {
        semitones: -12.0,
        ..Default::default()
    });
    let mut seed = 17_u32;
    let mut durations = Vec::with_capacity(3000);
    let started = std::time::Instant::now();
    for _ in 0..3000 {
        let mut data = [0.0; 16];
        for sample in &mut data {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *sample = (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.4;
        }
        let callback_started = std::time::Instant::now();
        pitch
            .process(&mut PlanarBuffer::new(&mut data, 1, 16).expect("small buffer"))
            .expect("small callback");
        durations.push(callback_started.elapsed());
    }
    let elapsed = started.elapsed();
    let budget = if cfg!(debug_assertions) {
        std::time::Duration::from_millis(2)
    } else {
        std::time::Duration::from_micros(100)
    };
    // The system can stop the test thread in any callback, so the slowest few say more about the
    // machine than about the search. A search that is not spread out is slow once in each splice,
    // about one callback in 100 at the default window, so this percentile still finds it.
    durations.sort_unstable();
    let percentile = durations[durations.len() * 995 / 1000];
    let longest = durations[durations.len() - 1];
    assert!(
        percentile < budget,
        "99.5% of the callbacks took up to {percentile:?}, over {budget:?}; the longest took \
         {longest:?}, and one second of audio took {elapsed:?}",
    );
}
