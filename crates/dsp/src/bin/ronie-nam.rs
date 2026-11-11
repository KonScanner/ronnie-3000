//! NAM rendering and benchmark command-line interface.

use clap::{Parser, Subcommand};
use eyre::{Result, WrapErr, eyre};
use ronie_core::{NamDocument, parse_nam};
use ronie_dsp::{
    bench::{BenchmarkConfig, BenchmarkEvidence, measure_benchmark},
    build_model,
};
use serde_json as _;
use std::{
    fs::File,
    io::{BufWriter, Write},
    num::{NonZeroU64, NonZeroUsize},
    path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(name = "ronie-nam", about = "Render and benchmark NAM models")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Render deterministic 220 Hz and 1,230 Hz input as raw f64.
    Render {
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        frames: NonZeroUsize,
        #[arg(long)]
        block_size: NonZeroUsize,
        #[arg(long)]
        seed: u64,
        #[arg(long)]
        output: PathBuf,
    },
    /// Measure callback timing with per-run statistics.
    Bench {
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        seconds: NonZeroU64,
        #[arg(long)]
        iterations: NonZeroUsize,
        #[arg(long)]
        block_size: NonZeroUsize,
        #[arg(long)]
        evidence_output: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Render {
            model,
            frames,
            block_size,
            seed,
            output,
        } => render(&model, frames.get(), block_size.get(), seed, &output),
        Commands::Bench {
            model,
            seconds,
            iterations,
            block_size,
            evidence_output,
        } => bench(
            &model,
            seconds.get(),
            iterations.get(),
            block_size.get(),
            evidence_output.as_deref(),
        ),
    }
}

fn render(
    model_path: &Path,
    frames: usize,
    block_size: usize,
    seed: u64,
    output_path: &Path,
) -> Result<()> {
    let document = load_document(model_path)?;
    let mut engine = build_model(&document).wrap_err("building render engine")?;
    let block_size = NonZeroUsize::new(block_size).ok_or_else(|| eyre!("block size is zero"))?;
    engine
        .prepare(48_000.0, block_size, false)
        .wrap_err("preparing render engine")?;

    let input = generate_input(frames, seed);
    let mut rendered = vec![0.0; frames];
    for (start, chunk) in input.chunks(block_size.get()).enumerate() {
        let begin = start * block_size.get();
        let end = begin + chunk.len();
        engine
            .process(chunk, &mut rendered[begin..end])
            .wrap_err("processing render callback")?;
    }

    let file = File::create(output_path)
        .wrap_err_with(|| format!("creating output file {}", output_path.display()))?;
    let mut writer = BufWriter::new(file);
    for sample in rendered {
        writer
            .write_all(&sample.to_le_bytes())
            .wrap_err("writing rendered samples")?;
    }
    writer.flush().wrap_err("flushing rendered samples")?;
    Ok(())
}

fn bench(
    model_path: &Path,
    seconds: u64,
    iterations: usize,
    block_size: usize,
    evidence_output: Option<&Path>,
) -> Result<()> {
    let document = load_document(model_path)?;
    let mut config = BenchmarkConfig::primary();
    config.run_seconds = seconds;
    config.measured_runs = iterations;
    config.block_size = block_size;
    let input: Vec<f32> = generate_input(block_size, 1)
        .into_iter()
        .map(|sample| sample as f32)
        .collect();
    let report = measure_benchmark(&document, &config, &input).wrap_err("running benchmark")?;

    for (index, run) in report.runs.iter().enumerate() {
        let statistics = run.statistics;
        println!(
            "run {}: callbacks={} min_us={:.6} p50_us={:.6} p99_us={:.6} p99_9_us={:.6} max_us={:.6} realtime_factor={:.6}",
            index + 1,
            run.timing.callbacks.len(),
            statistics.min_us,
            statistics.p50_us,
            statistics.p99_us,
            statistics.p99_9_us,
            statistics.max_us,
            statistics.realtime_factor
        );
    }
    println!(
        "median_of_run_medians_us={:.6}",
        report.median_of_run_medians_us
    );
    if let Some(path) = evidence_output {
        let evidence = BenchmarkEvidence::from_report(&report);
        let json =
            serde_json::to_vec_pretty(&evidence).wrap_err("serializing benchmark evidence")?;
        std::fs::write(path, json)
            .wrap_err_with(|| format!("writing benchmark evidence {}", path.display()))?;
    }
    Ok(())
}

fn load_document(path: &Path) -> Result<NamDocument> {
    let metadata = std::fs::metadata(path)
        .wrap_err_with(|| format!("reading model metadata {}", path.display()))?;
    if metadata.len() > 50 * 1024 * 1024 {
        return Err(eyre!("model exceeds the 50 MiB input limit"));
    }
    let bytes =
        std::fs::read(path).wrap_err_with(|| format!("reading model {}", path.display()))?;
    parse_nam(&bytes).wrap_err_with(|| format!("parsing model {}", path.display()))
}

fn generate_input(frames: usize, seed: u64) -> Vec<f64> {
    let offset = seed % 48_000;
    (0..frames)
        .map(|index| {
            let phase = std::f64::consts::TAU * (index as u64 + offset) as f64 / 48_000.0;
            0.5 * ((220.0 * phase).sin() + (1_230.0 * phase).sin())
        })
        .collect()
}
