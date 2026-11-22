#!/bin/sh
# Run from any directory. Kani, cargo-audit, curl, openssl, Python 3 and Rust are required.
set -eu
cd "$(dirname "$0")/.."
cargo fmt --check
rustfmt --edition 2021 --check verification/check.rs
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
build_dir=$(cargo metadata --no-deps --locked --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
python3 tests/compare.py "$build_dir/release/rill"
python3 tests/downloads.py "$build_dir/release/rill"
kani verification/check.rs --output-format terse
python3 verification/mutations.py
cargo audit --deny warnings
