#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo run -- init --dns prototype.lan --api-key local
cargo run -- trust-ca
cargo run -- serve
