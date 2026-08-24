#!/usr/bin/env bash
#
# The console does not build its CSS; it vendors it. The committed file is a
# copy, so the only thing keeping it honest is this check: fetch the pinned
# upstream and fail if what is committed no longer matches.
#
# To take a new version, bump the pin below and re-run with --update.

set -euo pipefail

KISO_VERSION="0.2.0"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
update=false
[[ "${1:-}" == "--update" ]] && update=true

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

(cd "$work" && npm pack "@momoi-labs/kiso@$KISO_VERSION" >/dev/null 2>&1)
tar -xzf "$work"/momoi-labs-kiso-*.tgz -C "$work"

upstream="$work/package/tokens/build/tokens.css"
vendored="$repo_root/console/tokens.css"

if $update; then
  cp "$upstream" "$vendored"
  echo "updated console/tokens.css from kiso@$KISO_VERSION"
  exit 0
fi

if ! cmp -s "$upstream" "$vendored"; then
  echo "console/tokens.css has drifted from kiso@$KISO_VERSION:" >&2
  diff -u "$vendored" "$upstream" >&2 || true
  echo "Run scripts/check-vendored-css.sh --update to take the pinned copy." >&2
  exit 1
fi

echo "console/tokens.css matches kiso@$KISO_VERSION"
