#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# The fast checks come first. Clippy also does all the work of cargo check.
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
# The tests use the ci profile (see Cargo.toml). cargo-nextest runs the test programs at the same
# time. It does not run doc tests.
if cargo nextest --version >/dev/null 2>&1; then
    cargo nextest run --workspace --no-fail-fast --cargo-profile ci
    cargo test --workspace --doc --profile ci
else
    cargo test --workspace --profile ci
fi
cargo deny check
