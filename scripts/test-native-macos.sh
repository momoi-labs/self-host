#!/usr/bin/env bash
# Installs only the native runtime on an otherwise unused, disposable Mac.
set -euo pipefail
[[ "${SELF_HOST_NATIVE_TEST:-}" = 1 && "$(uname -s)" = Darwin && "$(id -u)" != 0 ]] || {
  echo "Run as the Operator with SELF_HOST_NATIVE_TEST=1 on a disposable Mac" >&2; exit 1;
}
binary="${1:?Pass the built self-host binary}"
bundle="${2:?Pass its production-prefix s6 bundle}"
repository=$(cd "$(dirname "$0")/.." && pwd)
native_root="/Library/Application Support/self-host"
helper=/Library/PrivilegedHelperTools/dev.momoi.self-host
label=dev.momoi.self-host.native
for path in "$native_root" "$helper" "$helper.s6" "$helper.s6-licenses" "/Library/LaunchDaemons/$label.plist" /private/etc/sudoers.d/self-host-native; do
  if [[ -e "$path" || -L "$path" ]]; then echo "Refusing to overwrite existing Host path: $path" >&2; exit 1; fi
done
work=$(mktemp -d)
cp "$binary" "$work/self-host"
cp -R "$bundle" "$work/s6"
# Read function definitions without running downloads, Docker, DNS or VM setup.
source <(sed '$d' "$repository/install.sh")
tmpdir="$work"
cleanup() {
  local result=$?
  if [[ "$result" != 0 ]]; then
    sudo find "$native_root/native/logs" "$native_root/native/scan.log" -type f \
      -exec sh -c 'for f; do echo "== $f"; tail -100 "$f"; done' sh {} + 2>/dev/null || true
    ps -axo user,pid,pgid,ppid,stat,etime,command | grep -E 'sf-app|s6-' | grep -v grep || true
  fi
  if [[ -f "$helper" ]]; then
    sudo "$helper" native-uninstall || { echo "Account cleanup failed. Preserve this fixture for inspection." >&2; return 1; }
  fi
  sudo launchctl bootout "system/$label" >/dev/null 2>&1 || true
  sudo rm -f "/Library/LaunchDaemons/$label.plist" /private/etc/sudoers.d/self-host-native "$helper"
  sudo rm -rf "$helper.s6" "$helper.s6-licenses" "$native_root"
  rm -rf "$work"
  return "$result"
}
trap cleanup EXIT
target=aarch64-apple-darwin
if [[ "$(uname -m)" = x86_64 ]]; then target=x86_64-apple-darwin; fi
install_bundled_s6 "$target"
install_native_macos "$(id -un)"
for ((attempt=0; attempt<100; attempt++)); do
  if sudo test -p "$native_root/native/services/.s6-svscan/control"; then break; fi
  sleep 0.05
done
cd "$repository"
python3 scripts/test-s6-runtime.py "$work/s6"
cargo test --test native_macos -- --ignored --nocapture
