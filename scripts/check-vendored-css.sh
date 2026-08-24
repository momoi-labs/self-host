#!/usr/bin/env bash
#
# The console does not build its CSS; it vendors it. The committed files are
# copies, so the only thing keeping them honest is this check: fetch the
# pinned upstream and fail if what is committed no longer matches.
#
# To take a new version, bump the pin below and re-run with --update.

set -euo pipefail

KISO_VERSION="0.4.0"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
update=false
[[ "${1:-}" == "--update" ]] && update=true

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

(cd "$work" && npm pack "@momoi-labs/kiso@$KISO_VERSION" >/dev/null 2>&1)
tar -xzf "$work"/momoi-labs-kiso-*.tgz -C "$work"

# Paths of the package's own exports: ./tokens.css and ./ui.css.
declare -A upstream=(
  [tokens]="$work/package/tokens/build/tokens.css"
  [ui]="$work/package/kiso/ui.css"
)

status=0
for asset in tokens ui; do
  vendored="$repo_root/console/$asset.css"

  if $update; then
    cp "${upstream[$asset]}" "$vendored"
    echo "updated console/$asset.css"
  elif ! cmp -s "${upstream[$asset]}" "$vendored"; then
    echo "console/$asset.css has drifted from kiso@$KISO_VERSION:" >&2
    diff -u "$vendored" "${upstream[$asset]}" >&2 || true
    status=1
  fi
done

if [[ $status -ne 0 ]]; then
  echo "Run scripts/check-vendored-css.sh --update to take the pinned copies." >&2
  exit 1
fi

$update || echo "vendored CSS matches kiso@$KISO_VERSION"
