#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_DIR="/srv/devshare/rFerram"
TARGET_CRATE="${1:---all}"

cd "${WORKSPACE_DIR}"

echo "==> [1/4] Checking Rust formatting (cargo fmt --all -- --check)..."
cargo fmt --all -- --check

echo "==> [2/4] Running workspace Clippy linter (cargo clippy --workspace --all-targets --locked -- -D warnings)..."
cargo clippy --workspace --all-targets --locked -- -D warnings

if [[ "${TARGET_CRATE}" == "--all" ]]; then
    echo "==> [3/4] Running full workspace test suite (cargo test --workspace --all-targets --locked)..."
    cargo test --workspace --all-targets --locked
else
    echo "==> [3/4] Running targeted crate tests for '${TARGET_CRATE}'..."
    cargo test -p "${TARGET_CRATE}" --all-targets --locked
fi

echo "==> [4/4] Running rferram-server smoke test..."
RFERRAM_SMOKE_TEST=1 RFERRAM_BIND_ADDR="127.0.0.1:0" cargo run -p rferram-server --locked

echo "==> Quality Gate PASSED successfully."
