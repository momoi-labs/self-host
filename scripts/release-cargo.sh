#!/usr/bin/env bash
# GoReleaser's Rust tool hook: place a matrix-built binary where the Rust
# builder expects Cargo's output. Never compile in the publishing job.
set -euo pipefail

: "${SELF_HOST_PREBUILT_DIR:?Set SELF_HOST_PREBUILT_DIR to the downloaded build artifacts}"
target=""
for arg in "$@"; do
  case "$arg" in
    --target=*) target="${arg#--target=}" ;;
  esac
done

case "$target" in
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|x86_64-apple-darwin|aarch64-apple-darwin) ;;
  *) echo "Unsupported or missing release target: $target" >&2; exit 1 ;;
esac

source_binary="$SELF_HOST_PREBUILT_DIR/$target/self-host"
if [[ ! -f "$source_binary" || ! -x "$source_binary" ]]; then
  echo "Missing executable release artifact: $source_binary" >&2
  exit 1
fi

mkdir -p "target/$target/release"
cp "$source_binary" "target/$target/release/self-host"
