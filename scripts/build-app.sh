#!/usr/bin/env bash
set -euo pipefail

cargo build --release -p editable
cargo run --release -p editable-build-tools -- icon
cargo run --release -p editable-build-tools -- bundle

