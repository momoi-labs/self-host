#!/usr/bin/env bash
#
# The console does not build its CSS; it vendors it. The committed files are
# copies, so the only thing keeping them honest is this check: fetch the
# pinned upstream and fail if what is committed no longer matches.
#
# To take a new version, bump the pin below and re-run with --update.

set -euo pipefail

# Tokens ship on npm. Blocks are excluded from that package on purpose, so
# ui.css comes from the blueprint repo — pinned to a commit and not a tag,
# because blocks are illustrations and move between token releases.
KISO_VERSION="0.2.0"
BLUEPRINT_COMMIT="d1e90b37c9e267aa429fa5037483661cabee455c"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
update=false
[[ "${1:-}" == "--update" ]] && update=true

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

(cd "$work" && npm pack "@momoi-labs/kiso@$KISO_VERSION" >/dev/null 2>&1)
tar -xzf "$work"/momoi-labs-kiso-*.tgz -C "$work"
cp "$work/package/tokens/build/tokens.css" "$work/tokens.upstream.css"

curl -fsSL \
  "https://raw.githubusercontent.com/momoi-labs/blueprint/$BLUEPRINT_COMMIT/kiso/blocks/ui.css" \
  -o "$work/ui.upstream.css"

status=0
for asset in tokens ui; do
  upstream="$work/$asset.upstream.css"
  vendored="$repo_root/console/$asset.css"

  if $update; then
    cp "$upstream" "$vendored"
    echo "updated console/$asset.css"
  elif ! cmp -s "$upstream" "$vendored"; then
    echo "console/$asset.css has drifted from upstream:" >&2
    diff -u "$vendored" "$upstream" >&2 || true
    status=1
  fi
done

if [[ $status -ne 0 ]]; then
  echo "Run scripts/check-vendored-css.sh --update to take the pinned copies." >&2
  exit 1
fi

$update || echo "vendored CSS matches kiso@$KISO_VERSION"
