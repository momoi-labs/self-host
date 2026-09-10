#!/bin/sh
# Provisions the volume once and starts T3 Code. Every later boot finds the
# marker and only starts the server, so this is safe to run on every restart.
set -eu

home="${HOME:-/data/home}"
t3home="${T3CODE_HOME:-/data/t3home}"
marker="${home}/.devenv-provisioned"
key="${home}/.ssh/id_ed25519"

# Codex warns on every start when CODEX_HOME is missing, so create it here
# rather than leave a warning in the log that means nothing.
mkdir -p "$home" "$t3home" /data/repos "${home}/.ssh" "${CODEX_HOME:-${home}/.codex}"
chmod 700 "${home}/.ssh"

if [ ! -e "$marker" ]; then
	echo "devenv: provisioning ${home} for the first time"

	# Born here, and it stays here. A key handed to the container would be a
	# key that exists somewhere else too.
	[ -e "$key" ] || ssh-keygen -t ed25519 -N '' -C "devenv@$(hostname)" -f "$key" >/dev/null

	git config --global gpg.format ssh
	git config --global user.signingkey "${key}.pub"
	git config --global commit.gpgsign true
	git config --global init.defaultBranch main

	date -u +%Y-%m-%dT%H:%M:%SZ > "$marker"
fi

# Outside the marker on purpose: an identity set after the volume already
# exists has to arrive, and writing the same two values again costs nothing.
[ -z "${DEVENV_GIT_NAME:-}" ] || git config --global user.name "$DEVENV_GIT_NAME"
[ -z "${DEVENV_GIT_EMAIL:-}" ] || git config --global user.email "$DEVENV_GIT_EMAIL"

# Printed on every boot: enrolling this key on GitHub is a manual step, and the
# log is where the Operator looks for it.
echo "devenv: ssh public key: $(cat "${key}.pub")"

exec t3 serve \
	--mode web \
	--host "${DEVENV_T3_HOST:-0.0.0.0}" \
	--port "${DEVENV_T3_PORT:-3773}" \
	--no-browser \
	--base-dir "$t3home"
