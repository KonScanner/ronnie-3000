use ronie_core::{DspError, DspResult, NamDocument};
use serde::Serialize;
use std::{
    num::{NonZeroU8, NonZeroUsize},
    time::{Duration, Instant},
};

use crate::{PhaseEngine, compile_model};

/// Callback durations for one measured run.
#[derive(Debug, Clone)]
pub struct TimingRun {
    /// Every callback duration in callback order.
    pub callbacks: Vec<Duration>,
}

/// Statistics derived from one run's callbacks.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RunStatistics {
    /// Minimum callback microseconds.
    pub min_us: f64,
    /// Median callback microseconds.
    pub p50_us: f64,
    /// 99th percentile callback microseconds.
    pub p99_us: f64,
    /// 99.9th percentile callback microseconds.
    pub p99_9_us: f64,
    /// Maximum callback microseconds.
    pub max_us: f64,
    /// Callback audio duration divided by p50 processing duration.
    pub realtime_factor: f64,
}

/// Raw and derived evidence for one run.
#[derive(Debug, Clone)]
pub struct RunReport {
    /// Raw callback durations.
    pub timing: TimingRun,
    /// Derived run statistics.
    pub statistics: RunStatistics,
}

/// Run-preserving benchmark evidence.
#[derive(Debug, Clone)]
pub struct BenchmarkReport {
    /// Every measured run in execution order.
    pub runs: Vec<RunReport>,
    /// Median of per-run callback medians in microseconds.
    pub median_of_run_medians_us: f64,
}

/// Serializable raw evidence for one run.
#[derive(Debug, Clone, Serialize)]
pub struct RunEvidence {
    /// Derived run statistics.
    pub statistics: RunStatistics,
    /// Every callback duration in microseconds, in callback order.
    pub callback_us: Vec<f64>,
}

/// Serializable run-preserving benchmark evidence.
#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkEvidence {
    /// Every measured run in execution order.
    pub runs: Vec<RunEvidence>,
    /// Median of per-run callback medians in microseconds.
    pub median_of_run_medians_us: f64,
}

impl BenchmarkEvidence {
    /// Converts a benchmark report without discarding raw observations.
    #[must_use]
    pub fn from_report(report: &BenchmarkReport) -> Self {
        let runs = report
            .runs
            .iter()
            .map(|run| RunEvidence {
                statistics: run.statistics,
                callback_us: run
                    .timing
                    .callbacks
                    .iter()
                    .map(|duration| duration.as_secs_f64() * 1e6)
                    .collect(),
            })
            .collect();
        Self {
            runs,
            median_of_run_medians_us: report.median_of_run_medians_us,
        }
    }
}

/// Execution schedule for a benchmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Inline serial callbacks.
    Serial,
}

/// Benchmark configuration.
#[derive(Debug, Clone, Copy)]
pub struct BenchmarkConfig {
    /// Model sample rate in Hz.
    pub sample_rate: f64,
    /// Planar channel count.
    pub channels: usize,
    /// Oversampling factor.
    pub factor: u8,
    /// Voice count.
    pub voices: u8,
    /// Frames per callback and channel.
    pub block_size: usize,
    /// Number of measured runs.
    pub measured_runs: usize,
    /// Untimed warmup duration in seconds.
    pub warmup_seconds: u64,
    /// Duration of each measured run in seconds.
    pub run_seconds: u64,
    /// Whether to prewarm during preparation.
    pub prewarm: bool,
    /// Execution schedule.
    pub execution: ExecutionMode,
}

impl BenchmarkConfig {
    /// Primary full-tier 48 kHz, mono, 64-frame, 1x, serial contract.
    #[must_use]
    pub const fn primary() -> Self {
        Self {
            sample_rate: 48_000.0,
            channels: 1,
            factor: 1,
            voices: 1,
            block_size: 64,
            measured_runs: 10,
            warmup_seconds: 1,
            run_seconds: 1,
            prewarm: true,
            execution: ExecutionMode::Serial,
        }
    }
}

/// Derives run-preserving statistics from callback durations.
///
/// Percentiles use linear interpolation at `(len - 1) * p / 100`.
///
/// # Errors
///
/// Rejects empty runs, empty callbacks, invalid rates and block sizes, and zero p50 durations.
pub fn summarize_runs(
    runs: &[TimingRun],
    sample_rate: f64,
    block_size: usize,
) -> DspResult<BenchmarkReport> {
    if runs.is_empty() || !sample_rate.is_finite() || sample_rate <= 0.0 || block_size == 0 {
        return Err(DspError::InvalidBenchmarkInput);
    }

    let mut reports = Vec::with_capacity(runs.len());
    for run in runs {
        if run.callbacks.is_empty() {
            return Err(DspError::InvalidBenchmarkInput);
        }
        let mut samples: Vec<f64> = run
            .callbacks
            .iter()
            .map(|duration| duration.as_secs_f64() * 1e6)
            .collect();
        samples.sort_by(f64::total_cmp);
        let p50_us = percentile(&samples, 50.0);
        if p50_us <= 0.0 {
            return Err(DspError::InvalidBenchmarkInput);
        }
        let statistics = RunStatistics {
            min_us: samples[0],
            p50_us,
            p99_us: percentile(&samples, 99.0),
            p99_9_us: percentile(&samples, 99.9),
            max_us: samples[samples.len() - 1],
            realtime_factor: (block_size as f64 / sample_rate) / (p50_us / 1e6),
        };
        reports.push(RunReport {
            timing: run.clone(),
            statistics,
        });
    }

    let mut medians: Vec<f64> = reports
        .iter()
        .map(|report| report.statistics.p50_us)
        .collect();
    medians.sort_by(f64::total_cmp);
    Ok(BenchmarkReport {
        median_of_run_medians_us: percentile(&medians, 50.0),
        runs: reports,
    })
}

/// Measures one warmup period and the configured measured runs.
///
/// # Errors
///
/// Returns model, configuration, or callback errors.
pub fn measure_benchmark(
    document: &NamDocument,
    config: &BenchmarkConfig,
    input: &[f32],
) -> DspResult<BenchmarkReport> {
    let callback_samples = config
        .block_size
        .checked_mul(config.channels)
        .ok_or(DspError::InvalidBenchmarkInput)?;
    if input.len() != callback_samples
        || config.measured_runs == 0
        || config.run_seconds == 0
        || config.execution != ExecutionMode::Serial
    {
        return Err(DspError::InvalidBenchmarkInput);
    }
    let compiled = compile_model(document)?;
    let mut engine = PhaseEngine::new(
        &compiled,
        NonZeroU8::new(config.factor).ok_or(DspError::InvalidBenchmarkInput)?,
        NonZeroU8::new(config.voices).ok_or(DspError::InvalidBenchmarkInput)?,
    )?;
    engine.prepare(
        config.sample_rate,
        NonZeroUsize::new(config.block_size).ok_or(DspError::InvalidBenchmarkInput)?,
        config.prewarm,
    )?;

    for _ in 0..config.warmup_seconds {
        let _ = measure_run(&mut engine, input, config.run_seconds, config.channels)?;
    }
    let mut runs = Vec::with_capacity(config.measured_runs);
    for _ in 0..config.measured_runs {
        runs.push(measure_run(
            &mut engine,
            input,
            config.run_seconds,
            config.channels,
        )?);
    }
    summarize_runs(&runs, config.sample_rate, config.block_size)
}

fn measure_run(
    engine: &mut PhaseEngine,
    input: &[f32],
    seconds: u64,
    channels: usize,
) -> DspResult<TimingRun> {
    let mut buffer = input.to_vec();
    let started = Instant::now();
    let mut callbacks = Vec::new();
    while started.elapsed().as_secs() < seconds {
        let callback_started = Instant::now();
        engine.process_planar(&mut buffer, channels, false)?;
        callbacks.push(callback_started.elapsed());
    }
    Ok(TimingRun { callbacks })
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let position = (sorted.len() - 1) as f64 * percentile / 100.0;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f64;
    sorted[lower] + (sorted[upper] - sorted[lower]) * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(values: &[u64]) -> TimingRun {
        TimingRun {
            callbacks: values
                .iter()
                .map(|value| Duration::from_micros(*value))
                .collect(),
        }
    }

    #[test]
    fn bench_percentiles_use_linear_interpolation() {
        let report =
            summarize_runs(&[timing(&[1, 2, 3, 4])], 48_000.0, 64).expect("valid timing run");
        let statistics = &report.runs[0].statistics;

        assert_eq!(statistics.min_us, 1.0);
        assert_eq!(statistics.p50_us, 2.5);
        assert_eq!(statistics.p99_us, 3.97);
        assert!((statistics.p99_9_us - 3.997).abs() < 1e-12);
        assert_eq!(statistics.max_us, 4.0);
        assert_eq!(statistics.realtime_factor, (64.0 / 48_000.0) / 0.000_002_5);
    }

    #[test]
    fn bench_median_of_run_medians_preserves_raw_runs() {
        let report = summarize_runs(&[timing(&[1, 2]), timing(&[3, 4])], 48_000.0, 64)
            .expect("valid timing runs");

        assert_eq!(report.median_of_run_medians_us, 2.5);
        assert_eq!(report.runs.len(), 2);
        assert_eq!(report.runs[0].timing.callbacks[0], Duration::from_micros(1));
        assert_eq!(report.runs[0].timing.callbacks[1], Duration::from_micros(2));
        assert_eq!(report.runs[1].timing.callbacks[0], Duration::from_micros(3));
        assert_eq!(report.runs[1].timing.callbacks[1], Duration::from_micros(4));
    }

    #[test]
    fn bench_rejects_empty_runs_and_invalid_inputs() {
        assert!(summarize_runs(&[], 48_000.0, 64).is_err());
        assert!(summarize_runs(&[timing(&[])], 48_000.0, 64).is_err());
        assert!(summarize_runs(&[timing(&[1])], 0.0, 64).is_err());
        assert!(summarize_runs(&[timing(&[1])], f64::NAN, 64).is_err());
        assert!(summarize_runs(&[timing(&[1])], 48_000.0, 0).is_err());
        assert!(summarize_runs(&[timing(&[0])], 48_000.0, 64).is_err());
    }

    #[test]
    fn bench_evidence_preserves_ordered_raw_callback_microseconds() {
        let report = summarize_runs(&[timing(&[1, 2]), timing(&[3, 4])], 48_000.0, 64)
            .expect("valid timing runs");

        let evidence = BenchmarkEvidence::from_report(&report);

        assert_eq!(evidence.median_of_run_medians_us, 2.5);
        assert_eq!(evidence.runs[0].callback_us, vec![1.0, 2.0]);
        assert_eq!(evidence.runs[1].callback_us, vec![3.0, 4.0]);
        assert_eq!(evidence.runs[0].statistics.p50_us, 1.5);
        assert_eq!(evidence.runs[1].statistics.p50_us, 3.5);
    }

    #[test]
    fn bench_rejects_callback_size_overflow() {
        let document = ronie_oracle::synthetic::synthetic_a2_document(8).expect("synthetic model");
        let mut config = BenchmarkConfig::primary();
        config.channels = 2;
        config.block_size = usize::MAX;

        let result = measure_benchmark(&document, &config, &[]);

        assert!(matches!(result, Err(DspError::InvalidBenchmarkInput)));
    }

    #[test]
    fn bench_primary_config_matches_reference_contract() {
        let config = BenchmarkConfig::primary();

        assert_eq!(config.sample_rate, 48_000.0);
        assert_eq!(config.channels, 1);
        assert_eq!(config.factor, 1);
        assert_eq!(config.voices, 1);
        assert_eq!(config.block_size, 64);
        assert_eq!(config.measured_runs, 10);
        assert_eq!(config.warmup_seconds, 1);
        assert_eq!(config.run_seconds, 1);
        assert!(config.prewarm);
        assert_eq!(config.execution, ExecutionMode::Serial);
    }
}
