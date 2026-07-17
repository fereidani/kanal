#!/bin/sh
set -e

cargo +nightly fmt
cargo +nightly clippy
# verify every feature combination builds
cargo check --no-default-features
cargo check --features std-mutex
cargo check --all-features
cargo test --all-features
