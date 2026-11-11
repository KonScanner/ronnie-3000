# Verification fixtures

Public tests generate deterministic synthetic models and use isolated temporary directories. They do not download private assets or require an account.

## Optional external oracles

Additional differential tests can compare against operator-supplied baseline sources and builds:

```bash
export RONIE_REFERENCE_ROOT=/path/to/private/baseline
export RONIE_REFERENCE_BUILD=/path/to/private/build
```

These legacy configuration keys are retained for existing setups. Both paths must be present. Missing configuration skips private-only tests; incomplete configuration is an error. Private paths, credentials, models, and identifiers stay outside tracked files.

`manifest.json` records the public fixture inventory and expected hashes for private source roles. Generated public models receive their hashes when their generators are implemented.

## Default UI verification

Ronnie has its own colors and logo, so the visual gate compares Ronnie with an earlier capture of Ronnie. Save the local baseline one time, and save it again after you inspect an intended change:

```bash
script/check-ui.sh --save-baseline
script/check-ui.sh --suite ronnie-baseline
```

The images stay in the ignored `.ui-baseline` directory, and `commit.txt` there records their commit. The gate captures every scenario on a private display, so no window opens on your desktop and no audio device opens. It runs the selftest and applies the limits below. A small change can pass these limits, so the gate also lists each scenario with a changed pixel. Read that list. Captures are stable, so each entry is a real change. The report is `target/ui-suite/report.json`.

An optional layout check compares Ronnie with an external testbed build that is not part of this repository:

```bash
RONIE_REFERENCE_UI_EXE=/path/to/testbed script/check-ui.sh --suite reference-default
```

Use it only to look for a moved or missing control; colors and branding differ, so the check is expected to fail. It also runs an audio smoke test that opens real audio devices. Global mismatch must not exceed 1.9%, and the worst 64 px tile must not exceed 24%. The wordmark box is the only excluded area. `RONIE_REFERENCE_IMAGE_DIR` supplies local fixture artwork for captures with no network access. No private paths, credentials, or captured images are tracked.

## Milestone verification

Run the complete public gate and all tests, including private cases when configured:

```bash
script/check.sh
cargo test --workspace -- --include-ignored
cargo bench -p ronie-dsp --bench a2 --bench full_chain
cargo test -p ronie-dsp --release --test performance
```

Generate and render a public Standard model:

```bash
cargo run -p ronie-oracle --bin generate-fixture -- --channels 8 --output /tmp/ronie-a2-standard.nam
cargo run -p ronie-dsp --bin ronie-nam -- render --model /tmp/ronie-a2-standard.nam --frames 1024 --block-size 64 --seed 1 --output /tmp/ronie-a2.raw
```

The render writes 8,192 bytes of finite little-endian `f64` samples. `ronie-nam bench` retains every callback in each run and prints the median of run medians.

Current coverage and unresolved items are in `differential-manifest.json` and `chain-manifest.json`. The local A2 example passed all 35 stream/block cases on fast and generic paths. The processing primary benchmark measured a 59.006 µs median of run medians; all 40 matrix cases improved against the pre-optimization commit, with a worst time ratio of 0.857. Raw callback and environment evidence is in `processing-benchmark-evidence.json`. Exact official-fixture verification remains operator-dependent, and every chain-manifest row remains unresolved. Miri is unavailable in the pinned Rust 1.98 toolchain. These results do not establish UI, host, plugin, or full-application compatibility.

## Standalone acceptance

Run the deterministic local user scenarios with:

```bash
cargo test -p ronie-app --test standalone_end_to_end -- --nocapture
```

The tests use isolated temporary roots and public synthetic models. `standalone-manifest.json` records the covered restart, offline, MIDI, and unknown-field behavior plus the remaining factory-content, live-service, hardware, Flow, and external-differential gates. Local acceptance is not proof of full compatibility.

## Loading and persistence

`loading-manifest.json` assigns every acquisition, stash, HTTP/OAuth, codec, state, and preset behavior to an implementation task and baseline source/test path. A Rust-only round trip does not make a row verified. A `fixture_coverage` field names the local tests of a row, and `remaining_evidence` names what the row still needs. Neither field changes the status of the row.

External oracles provide partial coverage for preset filename rules and isolated v2/legacy tree acceptance. They do not yet prove complete full-file, collision, legacy-header, or two-way unknown-field behavior, so every loading row remains `unresolved`. Complete independent evidence still requires official model/WAV/local/stash assets, codec/session/legacy/offline/preset-store/host-program vectors, secret-redacted HTTP/OAuth recordings, and interrupted filesystem/network cases. The official A2 compile/cache closure and Rust end-to-end tests do not establish DSP, UI, host, or full-application compatibility.

The state runner uses a three-argument protocol: operation, input file, output file. Header/body encode operations consume legacy `ValueTree` XML and produce its binary form; decode operations reverse that transform. `preset-name` consumes and produces UTF-8. `token-merge` consumes a JSON object with `initial` and `external` strings and writes only to the private output path. `local-load` accepts a `LocalLoadTest` filter and requires `RONIE_REFERENCE_TEST_EXE` to select the already-built private test executable.

Build the private runner after configuring both paths above:

```bash
script/build-state-oracle.sh
```

The build checks the approved baseline commit and locates the required framework in the private source/build tree. It never links baseline code into a Ronnie product artifact. Standard error and private output contents stay out of tracked evidence.

Task 1 selects these Rust dependencies for later milestone tasks: `url` for parsed origins, `secrecy` for redacting/zeroizing token storage, `base64` for PKCE and binary fields, `quick-xml` for settings, `sha2` for content identity, and `tempfile` for isolated writes. Networking uses Tokio with only `macros`, `rt-multi-thread`, `sync`, `time`, `net`, `io-util`, and `fs`; reqwest disables default features and selects `rustls`, `json`, `query`, `stream`, and `gzip`. Native TLS and the Tokio `full` feature are not enabled. Versions are locked in `Cargo.lock` and checked by `cargo deny` and `cargo audit`.
