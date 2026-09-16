#!/usr/bin/env bash
#
# Installs self-host. On macOS it also prepares the Host to run the Platform
# unattended: a Docker runtime, Compose, the Platform Infra, a bridged network
# for Virtual machines, and launchd LaunchDaemons so everything returns after
# a reboot without anyone logging in (ADR-0013). See docs/macos-host.md for
# what this expects of the Mac.
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
# The Colima LaunchDaemon runs this instead of colima; see install_colima_watchdog.
WATCHDOG="self-host-colima"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
DNS_SUFFIX="${SELF_HOST_DNS:-home.lan}"
HOST_IP="${SELF_HOST_IP:-}"
RUNTIME="${SELF_HOST_RUNTIME:-}"
COLIMA_CPU="${SELF_HOST_COLIMA_CPU:-4}"
COLIMA_MEMORY="${SELF_HOST_COLIMA_MEMORY:-6}"

# Resolvers for the Colima VM. Any non-empty list frees port 53 on the Mac;
# these match the upstreams the Platform forwards to (ADR-0017).
COLIMA_DNS="1.1.1.1, 1.0.0.1"

PLATFORM_LABEL="dev.momoi.self-host"
COLIMA_LABEL="dev.momoi.self-host.colima"
VMNET_LABEL="dev.momoi.self-host.vmnet"
DAEMON_DIR="/Library/LaunchDaemons"

# socket_vmnet gives Lima a bridged vmnet interface over a Unix socket
# (ADR-0025). Built from source at this commit; the prefix is upstream's,
# chosen because only root can write there and the daemon runs as root.
SOCKET_VMNET_REPO="https://github.com/lima-vm/socket_vmnet"
SOCKET_VMNET_COMMIT="a061a8133f5f27d5e99da5dfb95d64d04fd4364a"
SOCKET_VMNET_PREFIX="/opt/socket_vmnet"
VMNET_SOCKET="/var/run/self-host-vmnet.sock"

# Whether `trust-ca` succeeded; the closing summary tells the Operator what
# this Host's own browser will do.
CA_TRUSTED=""

# Whether this run built socket_vmnet. A running daemon keeps the old binary
# until launchd starts it again, so a build forces a reload.
VMNET_REBUILT=""

# Where the downloaded archive is unpacked. The EXIT trap that removes it
# fires after install_binary has returned, so this cannot be a local.
tmpdir=""

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
			echo "Neither is the bridged network for Virtual machines: that is"
			echo "vmnet, which is macOS only. Lima on Linux bridges another way."
			;;
	esac
}

# ── Binary ───────────────────────────────────────────────────────────

install_binary() {
	local version="$1"
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
		# A Mac with Homebrew in /opt/homebrew has no /usr/local/bin at all,
		# and `install` does not create the directory it writes into.
		sudo install -d -m 755 "$INSTALL_DIR"
		sudo install -m 755 "$tmpdir/$BINARY" "$INSTALL_DIR/$BINARY"
	else
		install -m 755 "$tmpdir/$BINARY" "$INSTALL_DIR/$BINARY"
	fi

	if [ "$os" = "linux" ]; then
		if ! command -v setcap >/dev/null 2>&1; then
			echo "install libcap (libcap2-bin on Debian/Ubuntu), then run:" >&2
			echo "  sudo setcap cap_net_bind_service=+ep ${INSTALL_DIR}/${BINARY}" >&2
			exit 1
		fi
		# DNS now binds port 53 in this process, as the Operator.
		sudo setcap cap_net_bind_service=+ep "$INSTALL_DIR/$BINARY"
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

	install_vmnet "$home"

	echo
	echo "=== Done ==="
	echo
	echo "The Platform is supervised by launchd and starts at boot without a login."
	if [ -n "$CA_TRUSTED" ]; then
		echo "  Console:   https://admin.${DNS_SUFFIX} (this Host already trusts the CA)"
	else
		echo "  Console:   https://admin.${DNS_SUFFIX} (untrusted CA here; see the warning above)"
	fi
	echo "  Machines:  bridged on $(lan_interface) through ${VMNET_SOCKET}"
	echo "  Logs:      $home/Library/Logs/self-host/"
	echo "  Status:    sudo launchctl print system/${PLATFORM_LABEL}"
	echo "             sudo launchctl print system/${VMNET_LABEL}"
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
# restarts it if it exits. DNS runs on the Host and needs no VM forwarding.
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
	write_lima_override "$home"
	install_colima_watchdog

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
		<string>${INSTALL_DIR}/${WATCHDOG}</string>
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
		<key>COLIMA</key>
		<string>${brew_prefix}/bin/colima</string>
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
#
# Without an explicit mount the VM shares nothing of the Mac, and Docker
# creates every bind mount as an empty directory inside the VM, so a Compose
# Application with a bind mount sees an empty directory instead of the Host
# path.
#
# Naming DNS resolvers gives the VM its own, and turns Lima's host resolver
# off with them — colima sets `hostResolver.enabled` to
# `len(network.dns) == 0`. Nothing in the Platform wants the
# `host.docker.internal` mapping that resolver injected any more. This is not
# what frees port 53; see write_lima_override.
#
# An existing profile keeps its own file, so both land there too.
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
autoActivate: true
network:
  dns: [${COLIMA_DNS}]
mounts:
  - location: ${home}
    writable: true
EOF

	if [ -f "$profile" ]; then
		patch_colima_profile_dns "$profile"
	fi

	if [ -f "$profile" ] && ! grep -q "location: ${home}\$" "$profile"; then
		if grep -qE '^[[:space:]]*-[[:space:]]*location:' "$profile"; then
			# Mounts the Operator chose. Where their list ends is a guess, so
			# say what is missing instead of rewriting it.
			echo "the Colima profile mounts something else; add" >&2
			echo "  - location: ${home}" >&2
			echo "    writable: true" >&2
			echo "to the mounts in $profile, or bind mounts will be empty." >&2
		else
			echo "adding ${home} to the existing Colima profile ($profile)..."
			sed -i '' -E '/^mounts:[[:space:]]*(null|\[\])?[[:space:]]*$/d' "$profile"
			cat >>"$profile" <<EOF
mounts:
  - location: ${home}
    writable: true
EOF
		fi
	fi
}

# The Colima guest runs dnsmasq on port 53, and Lima republishes a guest
# listener on the same port of the Mac. That is what holds `TCP *:53` there,
# ahead of the Platform, with no container publishing anything — and colima
# has no setting for it. Turning Lima's host resolver off does not touch it:
# the port stays taken either way.
#
# Lima reads `$LIMA_HOME/_config/override.yaml` ahead of each instance's own
# file, and port-forward rules are matched in that order, so a rule that
# ignores guest port 53 wins. Only that port: Applications are still reached
# through forwarded ports like anything else.
# launchd's KeepAlive restarts a job when its process exits, and
# `colima start --foreground` does not exit when the VM stops. A `colima stop`,
# or a VM that dies on its own, leaves a live process supervising nothing while
# launchd sees a healthy job — the Host keeps DNS, the console and the API,
# which need no Docker, and silently loses every Application until someone
# notices.
#
# launchd stays the supervisor. What it lacks is a probe: nothing asks the VM
# whether it is up. This watchdog asks, and exits when the answer is no, which
# is the signal KeepAlive already knows how to act on.
install_colima_watchdog() {
	local script
	script="$(mktemp)"

	cat >"$script" <<'WATCHDOG'
#!/bin/sh
# Written by the self-host installer. A watchdog, not a supervisor: launchd
# restarts the job, this only decides when it should. `colima start
# --foreground` outlives the VM it started, so asking the VM whether it is up
# is what gives KeepAlive something to act on.
set -u

COLIMA="${COLIMA:-colima}"
READY_TIMEOUT="${SELF_HOST_COLIMA_READY_TIMEOUT:-300}"
POLL="${SELF_HOST_COLIMA_POLL:-15}"

"$COLIMA" start --foreground &
child=$!

# launchd stops the job with SIGTERM. Take the VM down with it, or the next
# start finds a VM running and this wrapper supervising nothing.
trap 'kill "$child" 2>/dev/null; wait "$child" 2>/dev/null; exit 0' HUP INT TERM

waited=0
until "$COLIMA" status >/dev/null 2>&1; do
	if ! kill -0 "$child" 2>/dev/null; then
		wait "$child"
		exit $?
	fi
	if [ "$waited" -ge "$READY_TIMEOUT" ]; then
		echo "the Colima VM did not come up in ${READY_TIMEOUT}s" >&2
		kill "$child" 2>/dev/null
		exit 1
	fi
	sleep 5
	waited=$((waited + 5))
done

echo "the Colima VM is up; watching it every ${POLL}s"

while "$COLIMA" status >/dev/null 2>&1; do
	if ! kill -0 "$child" 2>/dev/null; then
		wait "$child"
		exit $?
	fi
	sleep "$POLL"
done

echo "the Colima VM is gone; exiting so launchd starts it again" >&2
kill "$child" 2>/dev/null
wait "$child" 2>/dev/null
exit 1
WATCHDOG

	echo "installing ${INSTALL_DIR}/${WATCHDOG}..."
	if [ -w "$INSTALL_DIR" ]; then
		install -m 755 "$script" "$INSTALL_DIR/$WATCHDOG"
	else
		sudo install -m 755 "$script" "$INSTALL_DIR/$WATCHDOG"
	fi
	rm -f "$script"
}

write_lima_override() {
	local home="$1" override
	override="$home/.colima/_lima/_config/override.yaml"

	# An override the Operator wrote is theirs. Say what is missing.
	if [ -f "$override" ] && ! grep -q "self-host installer" "$override"; then
		echo "${override} already exists; add a portForwards rule that ignores" >&2
		echo "guest port 53, or Lima takes the port the Platform serves DNS on." >&2
		return
	fi

	mkdir -p "$(dirname "$override")"
	cat >"$override" <<'EOF'
# Written by the self-host installer. Lima republishes the Colima guest's
# dnsmasq on port 53 of the Mac, where the Platform serves DNS.
portForwards:
  - guestPort: 53
    ignore: true
  - guestIP: "0.0.0.0"
    guestPort: 53
    ignore: true
EOF
}

# A profile Colima already created keeps its own `network.dns`. Empty is the
# default, and it is what makes Lima hold the port; a list the Operator chose
# already frees it and is left alone.
patch_colima_profile_dns() {
	local profile="$1"

	if grep -qE '^[[:space:]]{2}dns:[[:space:]]*(null|\[\])?[[:space:]]*$' "$profile"; then
		echo "pointing the Colima profile at ${COLIMA_DNS} so it releases port 53..."
		sed -i '' -E "s@^([[:space:]]{2})dns:[[:space:]]*(null|\[\])?[[:space:]]*\$@\1dns: [${COLIMA_DNS}]@" "$profile"
	elif ! grep -qE '^[[:space:]]{2}dns:' "$profile"; then
		echo "could not find 'network.dns' in ${profile}." >&2
		echo "Set it to a resolver list, 'dns: [${COLIMA_DNS}]' for one, or Lima" >&2
		echo "keeps TCP port 53 and the Platform cannot serve DNS on it." >&2
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
# the Platform DNS. Other machines point at the Host IP as their DNS server instead.
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
	local iface
	iface="$(lan_interface)"
	if [ -n "$iface" ]; then
		ipconfig getifaddr "$iface" 2>/dev/null && return
	fi
	echo "127.0.0.1"
}

# The interface the default route leaves through is the LAN one.
lan_interface() {
	route -n get default 2>/dev/null | awk '/interface:/{print $2}'
}

# The Platform serves the console and every Application over HTTPS, signed by
# the CA that `init` created. Trusting it here is what makes the Host's own
# browser open https://admin.<suffix> without a warning; Consumers trust it
# with `self-host trust-ca --from ... --fingerprint ...`.
#
# The System Keychain refuses this where no one can authorize it, over SSH for
# one. That is a browser warning on this Host, not a reason to leave the Mac
# without the daemon the installer exists to set up, so the install carries on.
trust_ca() {
	echo "trusting the Platform CA on this Host (requires sudo)..."
	if "$INSTALL_DIR/$BINARY" trust-ca; then
		CA_TRUSTED=1
		return
	fi
	echo "could not trust the CA on this Host; continuing." >&2
	echo "The daemon and CLI can operate headlessly; the CLI uses the local CA." >&2
	echo "For browser trust, run '${BINARY} trust-ca' in Terminal in the Mac's" >&2
	echo "graphical session and approve the system prompt. Retrying over SSH" >&2
	echo "or through a root LaunchDaemon does not provide that authorization." >&2
	echo "Consumers must trust the CA on their own machines." >&2
}

# A Virtual machine is a host on the LAN (ADR-0025). socket_vmnet opens a
# bridged vmnet interface on the Host's LAN interface and hands it to Lima
# over a Unix socket. vmnet wants root, so the daemon runs as root under a
# LaunchDaemon that is always up, and the Platform never needs sudo. Bridged
# mode leaves port 53 alone; it is shared mode that hands it to mDNSResponder.
#
# Running this again on an installed Host changes nothing and says so.
install_vmnet() {
	local home="$1" iface
	iface="$(lan_interface)"
	case "$iface" in
		"")
			echo "no default route, so there is no LAN interface to bridge machines onto." >&2
			echo "Connect the Mac to the LAN and run the installer again." >&2
			exit 1
			;;
		en*) ;;
		*)
			# A VPN's utun, for one. vmnet cannot bridge it, and a daemon
			# that tries opens the socket, fails and comes back every ten
			# seconds, which looks up from a distance.
			echo "the default route leaves through ${iface}, which is not a LAN interface." >&2
			echo "Disconnect the VPN, or whatever holds the route, and run the installer again." >&2
			exit 1
			;;
	esac

	build_socket_vmnet
	install_vmnet_daemon "$iface" "$home"
	wait_for_vmnet
}

# From source at a pinned commit. The Lima project does not recommend the
# Homebrew package: its binary sits where any account with administrator
# rights can replace it, and launchd runs it as root. The build runs as the
# Operator; only the install needs sudo. The binary reports the commit it was
# built from, which is how the next run knows this build is already in place.
build_socket_vmnet() {
	local bin="$SOCKET_VMNET_PREFIX/bin/socket_vmnet" src
	if [ -x "$bin" ] && [ "$("$bin" --version 2>/dev/null)" = "$SOCKET_VMNET_COMMIT" ]; then
		echo "socket_vmnet ${SOCKET_VMNET_COMMIT:0:7} is already in ${SOCKET_VMNET_PREFIX}; nothing to build."
		return
	fi

	require_command_line_tools
	src="$tmpdir/socket_vmnet"

	echo "building socket_vmnet ${SOCKET_VMNET_COMMIT:0:7} from source..."
	git init -q "$src"
	git -C "$src" fetch -q --depth 1 "$SOCKET_VMNET_REPO" "$SOCKET_VMNET_COMMIT"
	git -C "$src" checkout -q FETCH_HEAD
	make -C "$src" socket_vmnet VERSION="$SOCKET_VMNET_COMMIT" >/dev/null

	echo "installing ${bin} (requires sudo)..."
	sudo install -d -m 755 -o root -g wheel "$SOCKET_VMNET_PREFIX/bin"
	sudo install -m 755 -o root -g wheel "$src/socket_vmnet" "$bin"
	VMNET_REBUILT=1
}

# `cc` and `make` on a Mac without the Command Line Tools are stubs that open
# a dialog nobody sees over SSH. Homebrew brings the tools with it; a Host
# that kept its own Docker runtime may never have had Homebrew.
require_command_line_tools() {
	if xcode-select -p >/dev/null 2>&1; then
		return
	fi
	echo "the Command Line Tools are required to build socket_vmnet. Install them first:" >&2
	echo "  xcode-select --install" >&2
	exit 1
}

# The socket is group staff, mode 0770: root opens it, and the Operator, who
# is in staff, connects to it. Rewritten and reloaded only when the plist
# would differ, since a reload drops every machine off the bridge.
install_vmnet_daemon() {
	local iface="$1" home="$2" plist desired
	plist="$DAEMON_DIR/${VMNET_LABEL}.plist"
	desired="$tmpdir/${VMNET_LABEL}.plist"

	cat >"$desired" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>${VMNET_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>${SOCKET_VMNET_PREFIX}/bin/socket_vmnet</string>
		<string>--vmnet-mode=bridged</string>
		<string>--vmnet-interface=${iface}</string>
		<string>--socket-group=staff</string>
		<string>${VMNET_SOCKET}</string>
	</array>
	<key>UserName</key>
	<string>root</string>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>StandardOutPath</key>
	<string>${home}/Library/Logs/self-host/vmnet.log</string>
	<key>StandardErrorPath</key>
	<string>${home}/Library/Logs/self-host/vmnet.log</string>
</dict>
</plist>
EOF

	if [ -z "$VMNET_REBUILT" ] && cmp -s "$desired" "$plist" 2>/dev/null; then
		echo "checking LaunchDaemon ${VMNET_LABEL} (requires sudo)..."
		if [ -n "$(vmnet_pid)" ]; then
			echo "LaunchDaemon ${VMNET_LABEL} is already running, bridged on ${iface}; nothing to change."
			return
		fi
	fi

	mkdir -p "$home/Library/Logs/self-host"

	echo "installing LaunchDaemon ${VMNET_LABEL}, bridged on ${iface} (requires sudo)..."
	sudo install -m 644 -o root -g wheel "$desired" "$plist"
	reload_daemon "$VMNET_LABEL"
}

# The socket file is not proof of anything: socket_vmnet opens it before it
# asks vmnet for the interface, and leaves it behind when that fails. A
# daemon that cannot bridge exits at once and KeepAlive brings it back, so
# the proof is one process that is still there a moment later.
wait_for_vmnet() {
	local i pid
	for i in $(seq 1 10); do
		pid="$(vmnet_pid)"
		if [ -n "$pid" ] && [ -S "$VMNET_SOCKET" ]; then
			sleep 2
			if [ "$(vmnet_pid)" = "$pid" ]; then
				echo "socket_vmnet is up at ${VMNET_SOCKET}."
				return
			fi
		fi
		sleep 1
	done
	echo "socket_vmnet is not staying up on ${VMNET_SOCKET}." >&2
	echo "See $HOME/Library/Logs/self-host/vmnet.log and" >&2
	echo "  sudo launchctl print system/${VMNET_LABEL}" >&2
	exit 1
}

# The pid launchd holds for the daemon; empty when it is not running.
vmnet_pid() {
	sudo launchctl print "system/${VMNET_LABEL}" 2>/dev/null | awk '/^\tpid = /{print $3}'
}

# The Platform itself, supervised by launchd as the Operator's user so that
# it finds the same Docker context, config directory and Compose projects
# the Operator sees. DNS starts before it waits for Docker and PostgreSQL.
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
