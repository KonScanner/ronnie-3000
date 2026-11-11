//! Release performance acceptance for the primary A2 contract.

use std::{
    num::{NonZeroU8, NonZeroUsize},
    time::Instant,
};

use ronie_dsp::{
    PhaseEngine,
    bench::{BenchmarkConfig, BenchmarkEvidence, measure_benchmark},
    compile_model,
};
use ronie_oracle::synthetic::synthetic_a2_document;

#[test]
fn primary_a2_standard_meets_the_66_microsecond_gate() {
    if !cfg!(debug_assertions) {
        let document = synthetic_a2_document(8).expect("synthetic document");
        let config = BenchmarkConfig::primary();
        let preparation_started = Instant::now();
        let compiled = compile_model(&document).expect("compiled model");
        let mut engine = PhaseEngine::new(
            &compiled,
            NonZeroU8::new(1).expect("factor"),
            NonZeroU8::new(1).expect("voices"),
        )
        .expect("phase engine");
        engine
            .prepare(48_000.0, NonZeroUsize::new(64).expect("frames"), true)
            .expect("preparation");
        let load_prepare_us = preparation_started.elapsed().as_secs_f64() * 1e6;
        drop(engine);

        let input = vec![0.25; 64];
        let report = measure_benchmark(&document, &config, &input).expect("benchmark");
        println!(
            "primary median of run medians: {:.3} µs",
            report.median_of_run_medians_us
        );
        assert_eq!(report.runs.len(), 10);
        assert!(
            report
                .runs
                .iter()
                .all(|run| !run.timing.callbacks.is_empty())
        );
        if let Ok(path) = std::env::var("RONIE_BENCHMARK_EVIDENCE") {
            let mut evidence = serde_json::to_value(BenchmarkEvidence::from_report(&report))
                .expect("evidence serialization");
            evidence["contract"] = serde_json::json!({
                "sample_rate": 48_000,
                "channels": 1,
                "frames": 64,
                "oversampling": 1,
                "voices": 1,
                "execution": "serial",
                "load_prepare_us": load_prepare_us
            });
            evidence["environment"] = environment_evidence();
            std::fs::write(
                path,
                serde_json::to_vec_pretty(&evidence).expect("evidence JSON"),
            )
            .expect("evidence output");
        }
        assert!(
            report.median_of_run_medians_us <= 66.0,
            "median of run medians was {:.3} µs",
            report.median_of_run_medians_us
        );
    }
}

fn environment_evidence() -> serde_json::Value {
    let resident_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|text| text.split_whitespace().nth(1).map(str::to_owned));
    let load_average = std::fs::read_to_string("/proc/loadavg").ok();
    let hardware = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("model name"))
                .map(str::to_owned)
        });
    serde_json::json!({
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "compiler": std::env::var("RONIE_RUSTC_VERSION").unwrap_or_else(|_| "unknown".into()),
        "hardware": hardware,
        "resident_pages": resident_pages,
        "load_average": load_average,
        "scalar_fallback": "Nano and non-AVX paths retained"
    })
}
