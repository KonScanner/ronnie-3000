//! Public synthetic A2 fixture generator.

use clap::Parser;
use eyre::{Result, WrapErr};
use ronie_oracle::synthetic::synthetic_a2_json;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "generate-fixture",
    about = "Generate a public synthetic A2 model"
)]
struct Args {
    #[arg(long)]
    channels: usize,
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let json = synthetic_a2_json(args.channels).wrap_err("generating synthetic model")?;
    std::fs::write(&args.output, json)
        .wrap_err_with(|| format!("writing fixture {}", args.output.display()))?;
    Ok(())
}
