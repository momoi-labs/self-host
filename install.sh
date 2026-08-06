#!/usr/bin/env bash
set -euo pipefail

REPO="momoi-labs/self-host"
BINARY="self-host"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

main() {
	local version="${1:-latest}"
	local os arch target archive url

	os=$(detect_os)
	arch=$(detect_arch)

	case "$os-$arch" in
		darwin-amd64)  target="x86_64-apple-darwin" ;;
		darwin-arm64)  target="aarch64-apple-darwin" ;;
		linux-amd64)   target="x86_64-unknown-linux-gnu" ;;
		linux-arm64)   target="aarch64-unknown-linux-gnu" ;;
		*)
			echo "unsupported platform: $os/$arch" >&2
			exit 1
			;;
	esac

	if [ "$version" = "latest" ]; then
		version=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
		if [ -z "$version" ]; then
			echo "failed to determine latest version" >&2
			exit 1
		fi
	fi

	archive="${BINARY}_${version}_${target}.tar.gz"
	url="https://github.com/${REPO}/releases/download/${version}/${archive}"

	echo "downloading ${BINARY} ${version} for ${target}..."
	tmpdir=$(mktemp -d)
	trap 'rm -rf "$tmpdir"' EXIT

	curl -fsSL "$url" -o "$tmpdir/$archive"
	tar xzf "$tmpdir/$archive" -C "$tmpdir"

	if [ ! -w "$INSTALL_DIR" ]; then
		echo "installing to ${INSTALL_DIR} (requires sudo)..."
		sudo install -m 755 "$tmpdir/$BINARY" "$INSTALL_DIR/$BINARY"
	else
		install -m 755 "$tmpdir/$BINARY" "$INSTALL_DIR/$BINARY"
	fi

	echo "${BINARY} ${version} installed to ${INSTALL_DIR}/${BINARY}"
}

detect_os() {
	case "$(uname -s)" in
		Darwin) echo "darwin" ;;
		Linux)  echo "linux" ;;
		*)
			echo "unsupported OS: $(uname -s)" >&2
			exit 1
			;;
	esac
}

detect_arch() {
	case "$(uname -m)" in
		x86_64|amd64) echo "amd64" ;;
		aarch64|arm64) echo "arm64" ;;
		*)
			echo "unsupported arch: $(uname -m)" >&2
			exit 1
			;;
	esac
}

main "$@"
