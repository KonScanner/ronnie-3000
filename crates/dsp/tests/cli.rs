//! NAM CLI integration tests.

use ronie_core as _;
use ronie_dsp as _;
use ronie_oracle as _;
use serde_json as _;
use std::{path::PathBuf, process::Command};

fn temporary_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ronie-cli-{}-{name}", std::process::id()))
}

fn render(
    model: &std::path::Path,
    output: &std::path::Path,
    block_size: usize,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ronie-nam"))
        .args([
            "render",
            "--model",
            model.to_str().expect("UTF-8 model path"),
            "--frames",
            "1024",
            "--block-size",
            &block_size.to_string(),
            "--seed",
            "1",
            "--output",
            output.to_str().expect("UTF-8 output path"),
        ])
        .output()
        .expect("render command should run")
}

#[test]
fn render_writes_finite_little_endian_f64_and_is_repeatable() {
    let model = temporary_path("model.nam");
    let first = temporary_path("first.raw");
    let second = temporary_path("second.raw");
    std::fs::write(
        &model,
        ronie_oracle::synthetic::synthetic_a2_json(8).expect("synthetic JSON"),
    )
    .expect("write model");

    for output in [&first, &second] {
        let result = render(&model, output, 64);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    let first_bytes = std::fs::read(&first).expect("first output");
    let second_bytes = std::fs::read(&second).expect("second output");
    assert_eq!(first_bytes.len(), 8_192);
    assert_eq!(first_bytes, second_bytes);
    for chunk in first_bytes.as_chunks::<8>().0.iter() {
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(chunk);
        assert!(f64::from_le_bytes(bytes).is_finite());
    }

    let _ = std::fs::remove_file(model);
    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}

#[test]
fn render_rejects_zero_block_size() {
    let model = temporary_path("invalid-model.nam");
    let output = temporary_path("invalid.raw");
    std::fs::write(
        &model,
        ronie_oracle::synthetic::synthetic_a2_json(8).expect("synthetic JSON"),
    )
    .expect("write model");

    let result = render(&model, &output, 0);

    assert!(!result.status.success());
    assert!(!output.exists());
    let _ = std::fs::remove_file(model);
}
