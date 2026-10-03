#!/usr/bin/env bash
# Build the private Linux supervision tools. No Host s6 or shared skarnet
# libraries are needed at runtime. Zig supplies the target's static musl libc.
set -euo pipefail

target="${1:?Pass a Linux Rust target}"
case "$target" in
  x86_64-unknown-linux-gnu) cc_target=x86_64-linux-musl ;;
  aarch64-unknown-linux-gnu) cc_target=aarch64-linux-musl ;;
  *) echo "Unsupported s6 target: $target" >&2; exit 1 ;;
esac

# Publishing imports an already-built bundle through release-cargo.sh.
if [[ -n "${SELF_HOST_PREBUILT_DIR:-}" ]]; then
  exit 0
fi

[[ "$(zig version)" = 0.13.0 ]] || { echo "s6 releases require Zig 0.13.0" >&2; exit 1; }
s6_version=2.15.1.0
skalibs_version=2.15.1.0
s6_sha256=eab9c46e22b66b16135f9a05ec68a0ea287d9060b84d10defaaa2caad158ab52
skalibs_sha256=f9c905e74935c6fe911c7e344e3e89d5fbd2014c1a04650b524b15ce9b5635d1
tools=(s6-svscan s6-supervise s6-svscanctl s6-svc s6-svwait s6-svok s6-svstat s6-log s6-setlock s6-ftrigrd)
output="$PWD/target/$target/release/s6"
work=$(mktemp -d /tmp/self-host-s6.XXXXXX)
trap 'rm -rf "$work"' EXIT

fetch() {
  local package="$1" version="$2" sha256="$3" archive
  archive="$package-$version.tar.gz"
  curl --fail --location --silent --show-error --retry 3 \
    "https://skarnet.org/software/$package/$archive" -o "$work/$archive"
  (cd "$work" && printf '%s  %s\n' "$sha256" "$archive" | shasum -a 256 -c -)
  tar xzf "$work/$archive" -C "$work"
}

fetch skalibs "$skalibs_version" "$skalibs_sha256"
fetch s6 "$s6_version" "$s6_sha256"
export CC="zig cc -target $cc_target -mcpu=baseline"
export CFLAGS=-Os
(
  cd "$work/skalibs-$skalibs_version"
  ./configure --host="$cc_target" --prefix="$work/deps" --disable-shared \
    --with-sysdep-devurandom=yes --with-sysdep-posixspawnearlyreturn=no \
    --with-sysdep-procselfexe=/proc/self/exe --with-sysdep-selectinfinite=yes
  make -j"${SELF_HOST_BUILD_JOBS:-2}" AR='zig ar' RANLIB='zig ranlib'
  make install AR='zig ar' RANLIB='zig ranlib'
)
(
  cd "$work/s6-$s6_version"
  ./configure --host="$cc_target" --disable-execline --enable-static-libc \
    --enable-absolute-paths --bindir=/usr/libexec/self-host/s6 \
    --libexecdir=/usr/libexec/self-host/s6 \
    --with-sysdeps="$work/deps/lib/skalibs/sysdeps" \
    --with-include="$work/deps/include" --with-lib="$work/deps/lib"
  make -j"${SELF_HOST_BUILD_JOBS:-2}" AR='zig ar' RANLIB='zig ranlib' "${tools[@]}"
)

mkdir -p "$work/bundle/bin" "$work/bundle/licenses"
for tool in "${tools[@]}"; do
  install -m 755 "$work/s6-$s6_version/$tool" "$work/bundle/bin/$tool"
done
zig_lib=$(zig env | python3 -c 'import json, sys; print(json.load(sys.stdin)["lib_dir"])')
install -m 644 "$work/s6-$s6_version/COPYING" "$work/bundle/licenses/s6.txt"
install -m 644 "$work/skalibs-$skalibs_version/COPYING" "$work/bundle/licenses/skalibs.txt"
install -m 644 "$zig_lib/libc/musl/COPYRIGHT" "$work/bundle/licenses/musl.txt"
install -m 644 "$zig_lib/../LICENSE" "$work/bundle/licenses/zig.txt"
printf 's6 %s\nskalibs %s\nzig 0.13.0 (static musl)\ntarget %s\n' \
  "$s6_version" "$skalibs_version" "$target" > "$work/bundle/versions.txt"
mkdir -p "$(dirname "$output")"
rm -rf "$output"
mv "$work/bundle" "$output"
echo "Built private s6 bundle for $target"
