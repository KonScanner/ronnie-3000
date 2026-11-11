//! Native UI layout, image, and scenario benchmarks.

use criterion::{Criterion, black_box};
use eframe::egui::{Color32, ColorImage};
use ronie_ui::{
    default::{BannerStore, ModalScrim},
    testbed::{BLOCKED_FIXTURES, load_scenarios, run_all},
    visual::compare_images,
};

fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    let scenarios = load_scenarios(include_str!("../../../fixtures/ui-scenarios.json"))
        .expect("scenario fixtures");

    criterion.bench_function("neutral-scenarios-103", |bencher| {
        bencher.iter(|| {
            let report = run_all(black_box(&scenarios)).expect("scenario selftest");
            assert_eq!(report.failed, BLOCKED_FIXTURES.len());
            assert_eq!(report.passed + report.failed, scenarios.len());
        });
    });

    let source = ColorImage::new([512, 512], vec![Color32::from_rgb(48, 48, 48); 512 * 512]);
    criterion.bench_function("modal-half-resolution-blur-512", |bencher| {
        bencher.iter(|| {
            let blurred = ModalScrim::blur_snapshot(black_box(&source));
            assert_eq!(blurred.size, [256, 256]);
        });
    });

    let reference = ColorImage::new([256, 256], vec![Color32::BLACK; 256 * 256]);
    let candidate = reference.clone();
    criterion.bench_function("visual-compare-256", |bencher| {
        bencher.iter(|| {
            let comparison = compare_images(black_box(&reference), black_box(&candidate), None)
                .expect("visual comparison");
            assert_eq!(comparison.mismatched, 0);
        });
    });

    let mut banners = BannerStore::new();
    let scenario = scenarios
        .iter()
        .find(|scenario| scenario.id == "banner-buffer")
        .expect("buffer banner");
    let device = ronie_ui::default::AudioDeviceState::from_ui_value(&scenario.state["device"])
        .expect("device state");
    criterion.bench_function("banner-priority-evaluation", |bencher| {
        bencher.iter(|| {
            banners.evaluate(black_box(Some(&device)), black_box(1_000));
            assert!(banners.active().is_some());
        });
    });

    criterion.final_summary();
}
