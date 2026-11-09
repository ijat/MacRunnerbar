#!/bin/bash
# scripts/gate.sh - the gate, run once per tip before merge: Check + boundaries + secrets.
# Needs `gitleaks` (brew install gitleaks).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
scripts/check-boundaries.sh
scripts/check-secrets.sh
echo "gate ok"
