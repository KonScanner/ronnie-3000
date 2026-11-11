//! Neutral full-chain scenario and private processor oracle tests.

use std::{collections::BTreeMap, fs, path::PathBuf};

use ronie_core::InputMode;
use ronie_dsp::PrepareSpec;
use ronie_oracle::{ChainOracle, ChainScenario, ChainStateChange, OracleError};
use serde_json::{Value, json};

fn scenario(callback_frames: Vec<usize>) -> ChainScenario {
    let frames = callback_frames.iter().sum();
    let input = (0..frames)
        .flat_map(|index| {
            let sample = ((index as f32 + 1.0) * 0.17).sin() * 0.5;
            [sample, sample]
        })
        .collect();
    ChainScenario {
        schema_version: 1,
        spec: PrepareSpec {
            host_sample_rate: 48_000.0,
            max_host_frames: 64,
            oversampling: 1,
            input_mode: InputMode::Stereo,
            stereo_input: true,
            stereo_output: true,
        },
        callback_frames,
        input,
        parameters: BTreeMap::new(),
        state_changes: Vec::new(),
        snapshot: json!({
            "name": "future preset",
            "futureField": { "nested": [1, true, null] }
        }),
    }
}

#[test]
fn scenario_round_trip_preserves_unknown_snapshot_fields() {
    let scenario = scenario(vec![31, 33]);

    let encoded = serde_json::to_string(&scenario).expect("scenario JSON");
    let decoded: ChainScenario = serde_json::from_str(&encoded).expect("scenario round trip");

    assert_eq!(decoded, scenario);
    assert_eq!(
        decoded.snapshot["futureField"]["nested"][1],
        Value::Bool(true)
    );
}

#[test]
fn every_reference_dsp_suite_has_an_unresolved_manifest_owner() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = fs::read_to_string(root.join("fixtures/chain-manifest.json"))
        .expect("chain manifest should exist");
    let rows: Value = serde_json::from_str(&manifest).expect("chain manifest should be valid JSON");
    let rows = rows.as_array().expect("manifest root should be an array");
    let required = [
        "auto_offset",
        "branch",
        "clipboard",
        "dsp",
        "dual_mono",
        "duplicate",
        "eq_chain_rate",
        "eq_post_routing",
        "factory_preset",
        "gate",
        "host_program",
        "ir_block_size",
        "ir_classification",
        "ir_temp_file",
        "local_load",
        "midi_map",
        "multicore",
        "outgain",
        "padless_wav",
        "pitch_shift",
        "preset",
        "processor",
        "spread",
        "state_cache",
        "swap_fade",
        "tone_refresh",
        "tuner",
        "worker_pool",
    ];

    assert_eq!(rows.len(), required.len());
    for suite in required {
        let row = rows
            .iter()
            .find(|row| row["suite"] == suite)
            .unwrap_or_else(|| panic!("missing chain manifest suite {suite}"));
        for field in ["owner", "expected", "tolerance", "fixture_sha256"] {
            assert!(
                row[field].as_str().is_some_and(|value| !value.is_empty()),
                "{suite} needs a nonempty {field}"
            );
        }
        assert_eq!(row["status"], "unresolved");
    }
}

#[test]
fn invalid_scenarios_fail_before_starting_a_private_process() {
    let mut scenario = scenario(vec![2, 2]);
    scenario.input.pop();
    let oracle = ChainOracle::from_executable(PathBuf::from("/not/a/real/runner"));

    let result = oracle.render(&scenario);

    assert!(matches!(result, Err(OracleError::InvalidCase(_))));
}

#[test]
#[ignore = "requires the private full-chain Reference runner"]
fn private_empty_chain_is_repeatable_across_callback_slicing() {
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => {
            eprintln!("skipping only the private full-chain oracle case");
            return;
        }
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let first = oracle
        .render(&scenario(vec![64, 32]))
        .expect("first render");
    let second = oracle
        .render(&scenario(vec![31, 33, 32]))
        .expect("second render");

    assert_eq!(first.channels, 2);
    assert_eq!(first.frames, 96);
    assert_eq!(first.samples.len(), 192);
    assert_eq!(first.samples, second.samples);
    assert!(first.samples.iter().any(|sample| *sample != 0.0));
    assert_eq!(first.latency.samples, 0);
    assert_eq!(first.latency.tail_seconds, 2.0);
    assert_eq!(first.diagnostic_state["stereoOutput"], Value::Bool(true));
}

#[test]
#[ignore = "requires the private full-chain Reference runner"]
fn private_runner_applies_indexed_state_changes() {
    let oracle = match ChainOracle::new() {
        Ok(oracle) => oracle,
        Err(OracleError::NotConfigured) => return,
        Err(error) => panic!("invalid private oracle configuration: {error}"),
    };
    let mut scenario = scenario(vec![48, 48]);
    scenario.state_changes.push(ChainStateChange {
        callback_index: 1,
        parameters: BTreeMap::from([("inputLevel".to_owned(), 0.25)]),
        reprepare: false,
    });

    let render = oracle.render(&scenario).expect("state-change render");

    assert_eq!(render.parameters.get("inputLevel"), Some(&0.25));
}
