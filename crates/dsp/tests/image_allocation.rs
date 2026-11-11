//! Prepared image and output allocation tests.
#![allow(unsafe_code)]

use ronie_dsp::{
    PlanarBuffer,
    image::{Align, AlignParams, ImageControls, ImageStage, Spread, SpreadParams},
    output::{OutputControls, OutputStage},
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
fn prepared_image_and_output_processing_do_not_allocate() {
    let mut spread = Spread::new(48_000.0, 512).expect("Spread preparation");
    spread.set_target(SpreadParams::default(), true);
    let mut align = Align::new(48_000.0, 512).expect("Align preparation");
    align.set_target(AlignParams::default(), true);
    let mut image = ImageStage::new(48_000.0, 512).expect("image preparation");
    let mut output = OutputStage::new(48_000.0, 512).expect("output preparation");
    let mut spread_data = [0.2; 1024];
    let mut align_data = [0.2; 1024];
    let mut image_data = [0.2; 1024];
    let mut output_data = [0.2; 1024];
    let controls = ImageControls {
        spread_enabled: true,
        align_enabled: true,
        auto_balance_listening: true,
        ..Default::default()
    };
    let mut spread_result = Ok(());
    let mut align_result = Ok(());
    let mut image_result = Ok(());
    let mut output_result = Ok(());
    let mut tap_samples = 0;

    let counts = counts(|| {
        spread_result = spread
            .process(&mut PlanarBuffer::new(&mut spread_data, 2, 512).expect("Spread buffer"));
        align_result =
            align.process(&mut PlanarBuffer::new(&mut align_data, 2, 512).expect("Align buffer"));
        image_result = image.process(
            &mut PlanarBuffer::new(&mut image_data, 2, 512).expect("image buffer"),
            controls,
            |left, right| tap_samples += left.len() + right.len(),
        );
        output_result = output.process(
            &mut PlanarBuffer::new(&mut output_data, 2, 512).expect("output buffer"),
            OutputControls::default(),
            |_, _| {},
        );
    });

    assert!(spread_result.is_ok(), "{spread_result:?}");
    assert!(align_result.is_ok(), "{align_result:?}");
    assert!(image_result.is_ok(), "{image_result:?}");
    assert!(output_result.is_ok(), "{output_result:?}");
    assert_eq!(tap_samples, 1024);
    assert_eq!(counts, (0, 0));
}
