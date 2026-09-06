#!/usr/bin/env bash
#
# Installs self-host. On macOS it also prepares the Host to run the Platform
# unattended: a Docker runtime, Compose, the Platform Infra, and two launchd
# LaunchDaemons so everything returns after a reboot without anyone logging
# in (ADR-0013). See docs/macos-host.md for what this expects of the Mac.
#
# Environment:
#   INSTALL_DIR              where the binary goes            (default /usr/local/bin)
#   SELF_HOST_DNS            DNS suffix for Hostnames         (default home.lan)
#   SELF_HOST_IP             LAN address of this Host         (default detected)
#   SELF_HOST_RUNTIME        colima | existing                (default: existing if
#                            `docker info` already works, colima otherwise)
#   SELF_HOST_COLIMA_CPU     CPUs for the Colima VM           (default 4)
#   SELF_HOST_COLIMA_MEMORY  GiB of memory for the Colima VM  (default 6)
#   SELF_HOST_BINARY_ONLY    set to 1 to install the binary and stop
#
set -euo pipefail

REPO="momoi-labs/self-host"
BINARY="self-host"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
DNS_SUFFIX="${SELF_HOST_DNS:-home.lan}"
HOST_IP="${SELF_HOST_IP:-}"
RUNTIME="${SELF_HOST_RUNTIME:-}"
COLIMA_CPU="${SELF_HOST_COLIMA_CPU:-4}"
COLIMA_MEMORY="${SELF_HOST_COLIMA_MEMORY:-6}"

PLATFORM_LABEL="dev.momoi.self-host"
COLIMA_LABEL="dev.momoi.self-host.colima"
DAEMON_DIR="/Library/LaunchDaemons"

main() {
	local version="${1:-latest}"

	install_binary "$version"

	if [ "${SELF_HOST_BINARY_ONLY:-}" = "1" ]; then
		exit 0
	fi

	case "$(detect_os)" in
		darwin) bootstrap_macos ;;
		linux)
			echo
			echo "Next: self-host init && self-host serve"
			echo "Unattended startup on Linux is not set up by this installer."
			;;
	esac
}

# ── Binary ───────────────────────────────────────────────────────────

install_binary() {
	local version="$1"
	local os arch target archive url tmpdir

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

# ── macOS Bootstrap ──────────────────────────────────────────────────

bootstrap_macos() {
	local operator home
	operator="$(id -un)"
	home="$HOME"

	if [ "$operator" = "root" ]; then
		echo "run the installer as the Operator, not as root; it asks for sudo where it needs it" >&2
		exit 1
	fi

	echo
	echo "=== Preparing this Mac as a self-host Host ==="
	echo "Operator: $operator"
	echo

	choose_runtime
	if [ "$RUNTIME" = "colima" ]; then
		prepare_colima "$operator" "$home"
	fi

	wait_for_docker

	init_platform

	configure_resolver

	trust_ca

	install_platform_daemon "$operator" "$home"

	echo
	echo "=== Done ==="
	echo
	echo "The Platform is supervised by launchd and starts at boot without a login."
	echo "  Console:   https://admin.${DNS_SUFFIX} (this Host already trusts the CA)"
	echo "  Logs:      $home/Library/Logs/self-host/"
	echo "  Status:    sudo launchctl print system/${PLATFORM_LABEL}"
	echo
	echo "Validate unattended startup now: reboot the Mac and open the console"
	echo "from another machine before anyone logs in. See docs/macos-host.md."
}

choose_runtime() {
	if [ -n "$RUNTIME" ]; then
		return
	fi
	if docker info >/dev/null 2>&1; then
		RUNTIME="existing"
		echo "Docker already answers; keeping the runtime that is there."
		echo "Unattended startup then depends on that runtime starting before login,"
		echo "which Docker Desktop and OrbStack do not do. Set SELF_HOST_RUNTIME=colima"
		echo "to install Colima under a LaunchDaemon instead."
	else
		RUNTIME="colima"
	fi
}

require_brew() {
	if command -v brew >/dev/null 2>&1; then
		return
	fi
	if [ -x /opt/homebrew/bin/brew ]; then
		eval "$(/opt/homebrew/bin/brew shellenv)"
		return
	fi
	echo "Homebrew is required to install the Docker runtime. Install it first:" >&2
	echo '  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"' >&2
	exit 1
}

# Colima runs the Docker daemon in a Lima VM. A LaunchDaemon that runs as
# the Operator (UserName) starts it at boot, before any login, and launchd
# restarts it if it exits. The gRPC port forwarder is what carries UDP 53
# from CoreDNS in the VM to the LAN; the default ssh forwarder is TCP only.
prepare_colima() {
	local operator="$1" home="$2" brew_prefix

	require_brew
	brew_prefix="$(brew --prefix)"

	echo "installing colima, docker and docker-compose with Homebrew..."
	brew list colima >/dev/null 2>&1 || brew install colima
	brew list docker >/dev/null 2>&1 || brew install docker
	brew list docker-compose >/dev/null 2>&1 || brew install docker-compose

	mkdir -p "$home/.docker/cli-plugins"
	ln -sf "$brew_prefix/opt/docker-compose/bin/docker-compose" "$home/.docker/cli-plugins/docker-compose"

	write_colima_template "$home"

	# A Colima started by hand belongs to a login session. Hand it to launchd.
	if colima status >/dev/null 2>&1; then
		echo "stopping the Colima instance started outside launchd..."
		colima stop || true
	fi

	mkdir -p "$home/Library/Logs/self-host"

	echo "installing LaunchDaemon ${COLIMA_LABEL} (requires sudo)..."
	sudo tee "$DAEMON_DIR/${COLIMA_LABEL}.plist" >/dev/null <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>${COLIMA_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>${brew_prefix}/bin/colima</string>
		<string>start</string>
		<string>--foreground</string>
	</array>
	<key>UserName</key>
	<string>${operator}</string>
	<key>GroupName</key>
	<string>staff</string>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>EnvironmentVariables</key>
	<dict>
		<key>HOME</key>
		<string>${home}</string>
		<key>PATH</key>
		<string>${brew_prefix}/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
	</dict>
	<key>StandardOutPath</key>
	<string>${home}/Library/Logs/self-host/colima.log</string>
	<key>StandardErrorPath</key>
	<string>${home}/Library/Logs/self-host/colima.log</string>
</dict>
</plist>
EOF
	sudo chown root:wheel "$DAEMON_DIR/${COLIMA_LABEL}.plist"
	sudo chmod 644 "$DAEMON_DIR/${COLIMA_LABEL}.plist"
	reload_daemon "$COLIMA_LABEL"
}

# Colima reads ~/.colima/_templates/default.yaml when it creates a profile.
# An existing profile keeps its own file, so the keys that matter are
# patched there too.
write_colima_template() {
	local home="$1" template profile
	template="$home/.colima/_templates/default.yaml"
	profile="$home/.colima/default/colima.yaml"

	mkdir -p "$(dirname "$template")"
	cat >"$template" <<EOF
cpu: ${COLIMA_CPU}
memory: ${COLIMA_MEMORY}
disk: 100
runtime: docker
vmType: vz
mountType: virtiofs
portForwarder: grpc
autoActivate: true
EOF

	if [ -f "$profile" ]; then
		echo "patching the existing Colima profile ($profile)..."
		sed -i '' -E 's/^portForwarder:.*/portForwarder: grpc/' "$profile"
		grep -q '^portForwarder:' "$profile" || echo 'portForwarder: grpc' >>"$profile"
	fi
}

wait_for_docker() {
	local i
	echo "waiting for Docker..."
	for i in $(seq 1 90); do
		if docker info >/dev/null 2>&1; then
			echo "Docker is available."
			return
		fi
		sleep 2
	done
	echo "Docker did not become available in 3 minutes." >&2
	if [ "$RUNTIME" = "colima" ]; then
		echo "See $HOME/Library/Logs/self-host/colima.log" >&2
	fi
	exit 1
}

# `init` writes the CA and the API key; running it again would mint new ones
# and break every machine that trusts the old CA. A CLI config is what a
# finished init leaves behind, so its presence is the signal to skip.
init_platform() {
	if [ -f "$HOME/.config/self-host/config.json" ]; then
		echo "the Platform is already initialized; keeping its keys and certificates."
		return
	fi
	local args=(init --dns "$DNS_SUFFIX")
	if [ -n "$HOST_IP" ]; then
		args+=(--host-ip "$HOST_IP")
	fi
	echo "bootstrapping the Platform..."
	"$INSTALL_DIR/$BINARY" "${args[@]}"
}

# /etc/resolver/<suffix> makes this Mac resolve its own Hostnames through
# CoreDNS. Other machines point at the Host IP as their DNS server instead.
configure_resolver() {
	local ip
	ip="$(host_ip)"
	echo "configuring /etc/resolver/${DNS_SUFFIX} -> ${ip} (requires sudo)..."
	sudo mkdir -p /etc/resolver
	echo "nameserver ${ip}" | sudo tee "/etc/resolver/${DNS_SUFFIX}" >/dev/null
}

host_ip() {
	if [ -n "$HOST_IP" ]; then
		echo "$HOST_IP"
		return
	fi
	# The interface the default route leaves through is the LAN one.
	local iface
	iface="$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')"
	if [ -n "$iface" ]; then
		ipconfig getifaddr "$iface" 2>/dev/null && return
	fi
	echo "127.0.0.1"
}

# The Platform serves the console and every Application over HTTPS, signed by
# the CA that `init` created. Trusting it here is what makes the Host's own
# browser open https://admin.<suffix> without a warning; Consumers trust it
# with `self-host trust-ca --from ... --fingerprint ...`.
trust_ca() {
	echo "trusting the Platform CA on this Host (requires sudo)..."
	"$INSTALL_DIR/$BINARY" trust-ca
}

# The Platform itself, supervised by launchd as the Operator's user so that
# it finds the same Docker context, config directory and Compose projects
# the Operator sees. It waits for Docker and PostgreSQL on its own.
install_platform_daemon() {
	local operator="$1" home="$2" path
	path="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

	mkdir -p "$home/Library/Logs/self-host"

	echo "installing LaunchDaemon ${PLATFORM_LABEL} (requires sudo)..."
	sudo tee "$DAEMON_DIR/${PLATFORM_LABEL}.plist" >/dev/null <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>${PLATFORM_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>${INSTALL_DIR}/${BINARY}</string>
		<string>serve</string>
	</array>
	<key>UserName</key>
	<string>${operator}</string>
	<key>GroupName</key>
	<string>staff</string>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>5</integer>
	<key>EnvironmentVariables</key>
	<dict>
		<key>HOME</key>
		<string>${home}</string>
		<key>PATH</key>
		<string>${path}</string>
		<key>RUST_LOG</key>
		<string>info</string>
	</dict>
	<key>StandardOutPath</key>
	<string>${home}/Library/Logs/self-host/self-host.log</string>
	<key>StandardErrorPath</key>
	<string>${home}/Library/Logs/self-host/self-host.log</string>
</dict>
</plist>
EOF
	sudo chown root:wheel "$DAEMON_DIR/${PLATFORM_LABEL}.plist"
	sudo chmod 644 "$DAEMON_DIR/${PLATFORM_LABEL}.plist"
	reload_daemon "$PLATFORM_LABEL"
}

reload_daemon() {
	local label="$1"
	sudo launchctl bootout "system/${label}" >/dev/null 2>&1 || true
	sudo launchctl bootstrap system "$DAEMON_DIR/${label}.plist"
	sudo launchctl enable "system/${label}"
	echo "${label} loaded."
}

main "$@"
