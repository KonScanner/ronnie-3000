//! Runs one private Reference persistence or local-loading probe.

use std::path::PathBuf;

use eyre::{WrapErr, eyre};
use ronie_oracle::state::{StateCommand, StateOracle};

fn main() -> eyre::Result<()> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let operation = arguments
        .next()
        .ok_or_else(|| eyre!("usage: reference-state <operation> <input> <output>"))?
        .into_string()
        .map_err(|_| eyre!("operation must be valid UTF-8"))?;
    let input = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| eyre!("missing input path"))?,
    );
    let output = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| eyre!("missing output path"))?,
    );
    if arguments.next().is_some() {
        return Err(eyre!("usage: reference-state <operation> <input> <output>"));
    }

    let input_bytes = std::fs::read(&input).wrap_err("could not read state-oracle input")?;
    let oracle = StateOracle::new()?;
    let output_bytes = oracle.run(&StateCommand::new(operation, input_bytes))?;
    std::fs::write(&output, output_bytes).wrap_err("could not write state-oracle output")?;
    Ok(())
}
