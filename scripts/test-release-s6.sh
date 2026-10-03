#!/usr/bin/env bash
# Run only in a fresh Linux test container, as root, without system s6.
set -euo pipefail
[[ "${SELF_HOST_RELEASE_TEST:-}" = 1 && "$(id -u)" = 0 ]] || {
  echo "Use SELF_HOST_RELEASE_TEST=1 in a disposable root container" >&2; exit 1;
}
bundle="${1:?Pass the built s6 bundle}"
installer="${2:?Pass install.sh}"
if [[ -e /usr/libexec/self-host/s6 ]] || command -v s6-svscan >/dev/null; then
  echo "Use a fresh container without s6 installed" >&2; exit 1
fi
work=$(mktemp -d)
scanner=""
cleanup() {
  if [[ -n "$scanner" ]]; then
    /usr/libexec/self-host/s6/s6-svscanctl -t "$work/scan" || true
    wait "$scanner" || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# Keep downloads and sudo outside this test. The installer still extracts and
# installs the release itself, including ownership, modes and license notices.
mkdir -p "$work/download" "$work/stubs"
cp -R "$bundle" "$work/download/s6"
printf '#!/bin/sh\nexit 0\n' > "$work/download/self-host"
chmod 755 "$work/download/self-host"
tar czf "$work/release.tar.gz" -C "$work/download" self-host s6
export SELF_HOST_TEST_ARCHIVE="$work/release.tar.gz"
cat > "$work/stubs/curl" <<'EOF'
#!/bin/sh
while [ "$#" -gt 0 ]; do
  if [ "$1" = -o ]; then cp "$SELF_HOST_TEST_ARCHIVE" "$2"; exit; fi
  shift
done
exit 1
EOF
printf '#!/bin/sh\nexec "$@"\n' > "$work/stubs/sudo"
printf '#!/bin/sh\nexit 0\n' > "$work/stubs/setcap"
chmod 755 "$work/stubs/"*

# A partial download must not replace any installed files.
mv "$work/download/s6/bin/s6-ftrigrd" "$work/s6-ftrigrd"
tar czf "$work/incomplete.tar.gz" -C "$work/download" self-host s6
if PATH="$work/stubs:$PATH" SELF_HOST_TEST_ARCHIVE="$work/incomplete.tar.gz" \
  SELF_HOST_BINARY_ONLY=1 bash "$installer" v0.0.0 > "$work/rejected.log" 2>&1; then
  echo "Installer accepted an incomplete bundle" >&2; exit 1
fi
[[ ! -e /usr/libexec/self-host/s6 && ! -e /usr/local/bin/self-host ]]
PATH="$work/stubs:$PATH" SELF_HOST_BINARY_ONLY=1 bash "$installer" v0.0.0
for path in /usr/libexec/self-host/s6/*; do
  [[ "$(stat -c %u:%g:%a "$path")" = 0:0:755 ]]
done
for license in s6 skalibs musl zig; do
  cmp "$bundle/licenses/$license.txt" "/usr/share/licenses/self-host-bin/s6/$license.txt"
done
cmp "$bundle/versions.txt" /usr/share/licenses/self-host-bin/s6/versions.txt

# Run with only the normal Host PATH. Compiled-in paths must find both
# s6-supervise and s6-ftrigrd in the private bundle, including after a crash.
s6=/usr/libexec/self-host/s6
mkdir -p "$work/scan/app/log" "$work/logs"
printf '3\n' > "$work/scan/app/notification-fd"
touch "$work/scan/app/down"
cat > "$work/scan/app/run" <<'EOF'
#!/bin/sh
exec /usr/libexec/self-host/s6/s6-setlock ./lock /bin/sh -c '
  echo $$ >> starts
  echo bundled-s6-ready
  printf "\n" >&3
  exec sleep 300
'
EOF
printf '#!/bin/sh\nexec %s/s6-log t s4096 n2 %q\n' "$s6" "$work/logs" > "$work/scan/app/log/run"
chmod 755 "$work/scan/app/run" "$work/scan/app/log/run"
env -i PATH=/usr/bin:/bin "$s6/s6-svscan" "$work/scan" &
scanner=$!
for ((i=0; i<100; i++)); do
  if "$s6/s6-svok" "$work/scan/app"; then break; fi
  sleep 0.05
done
"$s6/s6-svc" -u "$work/scan/app"
"$s6/s6-svwait" -U -t 5000 "$work/scan/app"
[[ "$("$s6/s6-svstat" -o up,ready "$work/scan/app")" = 'true true' ]]
first_pid=$("$s6/s6-svstat" -o pid "$work/scan/app")
kill -KILL "$first_pid"
for ((i=0; i<100; i++)); do
  if [[ "$(wc -l < "$work/scan/app/starts")" -ge 2 ]]; then break; fi
  sleep 0.05
done
[[ "$(wc -l < "$work/scan/app/starts")" -ge 2 ]]
"$s6/s6-svwait" -U -t 5000 "$work/scan/app"
"$s6/s6-svc" -d "$work/scan/app"
"$s6/s6-svwait" -D -t 5000 "$work/scan/app"
"$s6/s6-svscanctl" -t "$work/scan"
wait "$scanner"
scanner=""
grep -q bundled-s6-ready "$work/logs/current"
echo "Bundled s6: install, readiness, crash recovery, stop and logs passed"
