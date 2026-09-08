#!/usr/bin/env bash
#
# Removes what install.sh left on a macOS Host, so the next install starts
# from nothing. This is the cleanup between validation runs
# (docs/macos-host.md), not a polite uninstaller: the Applications, their
# volumes and the Platform CA go with it.
#
# The Colima VM survives by default. Rebuilding it costs minutes, nothing in
# it belongs to the Platform, and the installer patches the profile it finds.
#
# Environment:
#   INSTALL_DIR             where the binary went          (default /usr/local/bin)
#   SELF_HOST_DNS           DNS Suffix, if dns.json is gone (default home.lan)
#   SELF_HOST_DELETE_VM     set to 1 to delete the Colima VM too
#   SELF_HOST_FORCE         set to 1 to skip the confirmation
#
set -euo pipefail

BINARY="self-host"
WATCHDOG="self-host-colima"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
CA_NAME="Self-Host LAN CA"

PLATFORM_LABEL="dev.momoi.self-host"
COLIMA_LABEL="dev.momoi.self-host.colima"
DAEMON_DIR="/Library/LaunchDaemons"

SUFFIX=""

# Homebrew's bin is not on a non-login PATH, and this runs over SSH as often
# as in Terminal. Without this, `docker` and `colima` look absent and the
# cleanup quietly leaves the containers and the VM behind.
for dir in /opt/homebrew/bin /usr/local/bin; do
	if [ -d "$dir" ]; then
		case ":$PATH:" in
			*":$dir:"*) ;;
			*) PATH="$PATH:$dir" ;;
		esac
	fi
done
export PATH

main() {
	if [ "$(uname -s)" != "Darwin" ]; then
		echo "this cleanup undoes the macOS bootstrap; nothing to do here." >&2
		exit 1
	fi

	SUFFIX="$(dns_suffix)"
	confirm

	# The daemon goes first. launchd would otherwise restart it between the
	# steps below, and `serve` brings the Platform Infra back up.
	remove_daemon "$PLATFORM_LABEL"
	reset_platform
	remove_daemon "$COLIMA_LABEL"
	delete_vm

	remove_resolver
	remove_ca
	remove_binary
	remove_logs

	summary
}

# The Platform records the DNS Suffix in dns.json. Read it before the reset
# deletes the configuration directory it lives in.
dns_suffix() {
	local file="$HOME/.config/self-host/dns.json" found=""

	if [ -f "$file" ]; then
		found="$(sed -n 's/.*"dns_suffix"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$file" | head -1)"
	fi
	echo "${found:-${SELF_HOST_DNS:-home.lan}}"
}

confirm() {
	if [ "${SELF_HOST_FORCE:-}" = "1" ]; then
		return
	fi

	echo "This removes from this Mac:"
	echo "  - both LaunchDaemons, so nothing starts at boot"
	echo "  - every Platform and Application container, volume and network"
	echo "  - ${HOME}/.config/self-host, including the CA and the API key"
	echo "  - the CA from the System Keychain, so browsers warn again"
	echo "  - /etc/resolver/${SUFFIX}"
	echo "  - ${INSTALL_DIR}/${BINARY}, ${INSTALL_DIR}/${WATCHDOG} and the logs"
	if [ "${SELF_HOST_DELETE_VM:-}" = "1" ]; then
		echo "  - the Colima VM and everything else inside it"
	else
		echo "The Colima VM stays. Set SELF_HOST_DELETE_VM=1 to remove it."
	fi

	local answer=""
	if [ -t 0 ]; then
		printf "Continue? [y/N] "
		read -r answer
	elif [ -c /dev/tty ]; then
		printf "Continue? [y/N] "
		read -r answer </dev/tty
	else
		echo "no terminal to confirm on; set SELF_HOST_FORCE=1 to proceed." >&2
		exit 1
	fi

	case "$answer" in
		y|Y) ;;
		*) echo "Cancelled."; exit 1 ;;
	esac
}

remove_daemon() {
	local label="$1"

	if [ ! -f "$DAEMON_DIR/${label}.plist" ]; then
		return
	fi
	echo "removing LaunchDaemon ${label} (requires sudo)..."
	sudo launchctl bootout "system/${label}" >/dev/null 2>&1 || true
	sudo rm -f "$DAEMON_DIR/${label}.plist"
}

# `reset` is the Platform's own: containers, volumes, networks and the
# configuration directory. It needs Docker, which is why the Colima daemon is
# still up at this point.
reset_platform() {
	if [ ! -x "$INSTALL_DIR/$BINARY" ]; then
		echo "no ${BINARY} in ${INSTALL_DIR}; skipping the Platform reset."
		return
	fi
	if ! docker info >/dev/null 2>&1; then
		echo "Docker does not answer, so containers and volumes stay behind." >&2
		echo "Start the runtime, run '${BINARY} reset --force', then run this" >&2
		echo "script again." >&2
		return
	fi
	echo "removing containers, volumes, networks and configuration..."
	"$INSTALL_DIR/$BINARY" reset --force
	rm -rf "$HOME/.config/self-host"
}

delete_vm() {
	if [ "${SELF_HOST_DELETE_VM:-}" != "1" ]; then
		return
	fi
	if ! command -v colima >/dev/null 2>&1; then
		echo "colima not found, so the VM stays. Delete it with 'colima delete'." >&2
		return
	fi
	echo "deleting the Colima VM..."
	colima delete --force || true
}

remove_resolver() {
	if [ -f "/etc/resolver/${SUFFIX}" ]; then
		echo "removing /etc/resolver/${SUFFIX} (requires sudo)..."
		sudo rm -f "/etc/resolver/${SUFFIX}"
	fi

	# A suffix the Mac was set up with earlier is not ours to guess at.
	local others
	others="$(ls -A /etc/resolver 2>/dev/null || true)"
	if [ -n "$others" ]; then
		echo "/etc/resolver still holds: ${others}" >&2
		echo "Remove by hand any that belonged to an earlier DNS Suffix." >&2
	fi
}

# `init` mints a CA per install, so a Mac that was set up more than once has
# more than one to delete.
remove_ca() {
	local hashes sha
	hashes="$(security find-certificate -c "$CA_NAME" -a -Z /Library/Keychains/System.keychain 2>/dev/null | awk '/^SHA-1 hash:/{print $3}')"

	if [ -z "$hashes" ]; then
		return
	fi
	echo "removing '${CA_NAME}' from the System Keychain (requires sudo)..."
	for sha in $hashes; do
		sudo security delete-certificate -Z "$sha" /Library/Keychains/System.keychain || true
	done
}

# The binary and the Colima watchdog the installer wrote beside it.
remove_binary() {
	local name
	for name in "$BINARY" "$WATCHDOG"; do
		[ -f "$INSTALL_DIR/$name" ] || continue
		echo "removing ${INSTALL_DIR}/${name}..."
		if [ -w "$INSTALL_DIR" ]; then
			rm -f "$INSTALL_DIR/$name"
		else
			sudo rm -f "$INSTALL_DIR/$name"
		fi
	done
}

remove_logs() {
	rm -rf "$HOME/Library/Logs/self-host"
}

summary() {
	echo
	echo "The Host is clean. What this did not touch:"
	if [ "${SELF_HOST_DELETE_VM:-}" != "1" ]; then
		echo "  - the Colima VM and its profile (SELF_HOST_DELETE_VM=1 removes them)"
	fi
	echo "  - Homebrew and the colima, docker and docker-compose packages"
	echo "  - the CA on any Consumer that trusted it; remove it there too"
	echo "  - the DNS server other machines point at, if you changed the router"
}

main "$@"
