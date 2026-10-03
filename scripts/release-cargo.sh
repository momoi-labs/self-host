#!/usr/bin/env bash
# Import matrix-built artifacts without compiling in the publishing job.
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

if [[ "$target" == *-linux-gnu ]]; then
  bundle="$SELF_HOST_PREBUILT_DIR/$target/s6"
  for tool in s6-svscan s6-supervise s6-svscanctl s6-svc s6-svwait s6-svok s6-svstat s6-log s6-setlock s6-ftrigrd; do
    if [[ ! -f "$bundle/bin/$tool" || ! -x "$bundle/bin/$tool" ]]; then
      echo "Missing executable s6 artifact: $bundle/bin/$tool" >&2
      exit 1
    fi
  done
  for license in s6 skalibs musl zig; do
    [[ -s "$bundle/licenses/$license.txt" ]] || { echo "Missing s6 license: $license" >&2; exit 1; }
  done
  grep -qxF "target $target" "$bundle/versions.txt" || { echo "Wrong or missing s6 target metadata" >&2; exit 1; }
fi

mkdir -p "target/$target/release"
cp "$source_binary" "target/$target/release/self-host"
if [[ "$target" == *-linux-gnu ]]; then
  rm -rf "target/$target/release/s6"
  cp -R "$bundle" "target/$target/release/s6"
fi
