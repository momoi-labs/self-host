#!/usr/bin/env bash
#
# Removes what install.sh left on a macOS Host, so the next install starts
# from nothing. This is the cleanup between validation runs
# (docs/macos-host.md), not a polite uninstaller: the Applications, their
# volumes, the Platform CA and the bridged network for machines go with it.
# What the Platform created is `self-host uninstall`'s to remove; this script
# runs it, then removes what install.sh itself set up.
#
# The Colima VM survives by default. Rebuilding it costs minutes, nothing in
# it belongs to the Platform, and the installer patches the profile it finds.
#
# Environment:
#   INSTALL_DIR             where the binary went          (default /usr/local/bin)
#   SELF_HOST_DELETE_VM     set to 1 to delete the Colima VM too
#   SELF_HOST_FORCE         set to 1 to skip the confirmation
#   SELF_HOST_DATA_DIR      the Platform's data directory  (default: the one
#                           the installer gave the LaunchDaemon, or
#                           ~/.config/self-host)
#
set -euo pipefail

BINARY="self-host"
WATCHDOG="self-host-colima"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

PLATFORM_LABEL="dev.momoi.self-host"
COLIMA_LABEL="dev.momoi.self-host.colima"
VMNET_LABEL="dev.momoi.self-host.vmnet"
DAEMON_DIR="/Library/LaunchDaemons"

SOCKET_VMNET_PREFIX="/opt/socket_vmnet"
VMNET_SOCKET="/var/run/self-host-vmnet.sock"

DATA_DIR=""

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

	# A data directory that was given goes to the uninstall as given, even at the
	# default path: given, it holds the machines' disks too.
	local given
	given="$(data_dir)"
	DATA_DIR="${given:-$HOME/.config/self-host}"
	if [ -n "$given" ]; then
		export SELF_HOST_DATA_DIR="$given"
	fi
	confirm

	# Until the end, the LaunchDaemon's plist stays: it names the data
	# directory, so a run that stops halfway, for Full Disk Access say, can
	# be run again and find it.
	if [ -n "$given" ]; then
		trap 'status=$?; [ "$status" -eq 0 ] || echo "Run this again with SELF_HOST_DATA_DIR=\"${DATA_DIR}\" to finish." >&2' EXIT
	fi

	# The daemon stops first. launchd would otherwise restart it between the
	# steps below, and `serve` brings the Platform Infra back up.
	stop_daemon "$PLATFORM_LABEL"
	# The Platform's own uninstall needs Docker and the native helper, so
	# both are still up.
	uninstall_platform
	remove_native
	remove_daemon "$COLIMA_LABEL"
	delete_vm
	remove_vmnet
	remove_daemon "$PLATFORM_LABEL"

	remove_binary
	remove_logs

	summary
}

# The data directory the Platform was given: SELF_HOST_DATA_DIR, or the one
# the installer gave the LaunchDaemon, read before that plist goes. Empty
# when it uses the default places.
data_dir() {
	local plist="$DAEMON_DIR/${PLATFORM_LABEL}.plist" found="${SELF_HOST_DATA_DIR:-}"
	if [ -z "$found" ] && [ -f "$plist" ]; then
		found="$(/usr/libexec/PlistBuddy -c "Print :EnvironmentVariables:SELF_HOST_DATA_DIR" "$plist" 2>/dev/null || true)"
	fi
	echo "$found"
}

confirm() {
	if [ "${SELF_HOST_FORCE:-}" = "1" ]; then
		return
	fi

	echo "This removes from this Mac:"
	echo "  - everything the Platform created: Applications with their data and images,"
	echo "    Virtual machines, native Applications and their accounts, the CA in the"
	echo "    System Keychain, its /etc/resolver files and ${DATA_DIR}"
	echo "  - every LaunchDaemon the installer wrote, so nothing starts at boot"
	echo "  - the bridged network for machines: socket_vmnet in ${SOCKET_VMNET_PREFIX} and its socket"
	echo "  - the native helper, supervisor and sudoers grant"
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

stop_daemon() {
	local label="$1"

	if [ ! -f "$DAEMON_DIR/${label}.plist" ]; then
		return
	fi
	echo "stopping LaunchDaemon ${label} (requires sudo)..."
	sudo launchctl bootout "system/${label}" >/dev/null 2>&1 || true
}

# What the Platform created, `self-host uninstall` removes: Applications,
# their data and images, Virtual machines, native Applications and their
# accounts, the data directory, the CA and the resolver files. It needs
# Docker, which is why the Colima daemon is still up at this point.
uninstall_platform() {
	if [ ! -x "$INSTALL_DIR/$BINARY" ]; then
		echo "no ${BINARY} in ${INSTALL_DIR}; skipping what the Platform created."
		return
	fi
	if ! docker info >/dev/null 2>&1; then
		echo "Docker does not answer, so the Platform's containers and data stay." >&2
		echo "Start the runtime, then run this script again." >&2
		exit 1
	fi
	"$INSTALL_DIR/$BINARY" uninstall --force
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

remove_native() {
	local helper=/Library/PrivilegedHelperTools/dev.momoi.self-host
	remove_daemon dev.momoi.self-host.native
	sudo rm -f /private/etc/sudoers.d/self-host-native "$helper"
	sudo rm -rf /Library/PrivilegedHelperTools/dev.momoi.self-host.s6 \
		/Library/PrivilegedHelperTools/dev.momoi.self-host.s6-licenses
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

# The bridged network for machines: the daemon, the socket it opened and the
# binary the installer built. Only the binary and the directories that leaves
# empty; anything else under the prefix is not the installer's to delete.
remove_vmnet() {
	remove_daemon "$VMNET_LABEL"

	if [ -e "$VMNET_SOCKET" ]; then
		echo "removing ${VMNET_SOCKET} (requires sudo)..."
		sudo rm -f "$VMNET_SOCKET"
	fi

	if [ -d "$SOCKET_VMNET_PREFIX" ]; then
		echo "removing ${SOCKET_VMNET_PREFIX}/bin/socket_vmnet (requires sudo)..."
		sudo rm -f "$SOCKET_VMNET_PREFIX/bin/socket_vmnet"
		sudo rmdir "$SOCKET_VMNET_PREFIX/bin" "$SOCKET_VMNET_PREFIX" 2>/dev/null || true
	fi
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
