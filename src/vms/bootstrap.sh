#!/usr/bin/env bash
# Run as root inside the machine, either by cloud-init during its first boot
# or piped into a shell for an update. Everything printed here is the account
# of what happened: cloud-init keeps it in /var/log/cloud-init-output.log, and
# an update streams it straight back to the Host.
set -Eeuo pipefail
umask 077
: "${SELF_HOST_MISE_TOML_B64:?missing mise config}"
# A machine may have neither a key nor a service: the console opens its own
# terminal, and a workspace with nothing to serve is still a workspace.
SELF_HOST_SSH_PUBLIC_KEY_B64="${SELF_HOST_SSH_PUBLIC_KEY_B64:-}"
SELF_HOST_SETUP_B64="${SELF_HOST_SETUP_B64:-}"
SELF_HOST_COMMAND_B64="${SELF_HOST_COMMAND_B64:-}"
SELF_HOST_WEB_PORT="${SELF_HOST_WEB_PORT:-0}"

readonly config_dir=/etc/self-host-environment
readonly dev_user=dev
readonly dev_home=/home/dev
readonly log_file=/var/log/self-host-environment-bootstrap.log
install -d -m 0755 "$config_dir"
touch "$log_file"
chmod 0600 "$log_file"
# A copy stays in the machine, so a run can be read from inside as well.
exec > >(tee -a "$log_file") 2>&1
step=prepare
stage=$(mktemp -d "$config_dir/staging.XXXXXX")
chmod 0755 "$stage"
committed=false
previous=false
if test -f "$config_dir/mise.toml"; then
    previous=true
    cp "$config_dir/mise.toml" "$stage/previous-mise.toml"
    cp "$config_dir/command" "$stage/previous-command"
fi
cleanup() {
    result=$?
    if (( result != 0 )); then
        if "$committed" && "$previous"; then
            cp "$stage/previous-mise.toml" "$config_dir/mise.toml"
            cp "$stage/previous-command" "$config_dir/command"
            systemctl restart self-host-environment.service || true
        fi
        printf 'SF_STEP failed:%s\n' "$step"
    fi
    rm -rf -- "$stage"
    exit "$result"
}
trap cleanup EXIT
progress() { step="$1"; printf 'SF_STEP %s\n' "$step"; }

progress system-packages
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ca-certificates curl git openssh-server build-essential python3 procps \
    unzip xz-utils pkg-config

progress user-and-ssh
if ! id "$dev_user" >/dev/null 2>&1; then
    useradd --create-home --home-dir "$dev_home" --shell /bin/bash "$dev_user"
fi
# The machine is the Operator's own workspace and they are its only user.
# Without this the shell can never install anything: dev has no password, so
# a sudo prompt is not something anyone could answer.
printf '%s ALL=(ALL) NOPASSWD:ALL\n' "$dev_user" > "$stage/sudoers"
visudo -cqf "$stage/sudoers"
install -o root -g root -m 0440 "$stage/sudoers" /etc/sudoers.d/self-host-environment
install -d -o "$dev_user" -g "$dev_user" -m 0700 "$dev_home/.ssh"
for directory in .local .local/state .local/share .cache .config .config/mise \
    .local/state/mise .local/share/mise .cache/mise; do
    install -d -o "$dev_user" -g "$dev_user" -m 0750 "$dev_home/$directory"
done
chown -R "$dev_user:$dev_user" "$dev_home/.local/state/mise" \
    "$dev_home/.local/share/mise" "$dev_home/.cache/mise" "$dev_home/.config/mise"
install -d -o "$dev_user" -g "$dev_user" -m 0750 "$dev_home/repos" "$dev_home/.local/state/t3"
# Keep manually added keys. Add the requested public key once, if there is one.
touch "$dev_home/.ssh/authorized_keys"
if test -n "$SELF_HOST_SSH_PUBLIC_KEY_B64"; then
    printf '%s' "$SELF_HOST_SSH_PUBLIC_KEY_B64" | base64 --decode > "$stage/public-key"
    if ! grep -qxFf "$stage/public-key" "$dev_home/.ssh/authorized_keys"; then
        cat "$stage/public-key" >> "$dev_home/.ssh/authorized_keys"
        printf '\n' >> "$dev_home/.ssh/authorized_keys"
    fi
fi
chown "$dev_user:$dev_user" "$dev_home/.ssh/authorized_keys"
chmod 0600 "$dev_home/.ssh/authorized_keys"
ssh-keygen -A
systemctl enable --now ssh

progress mise
if ! test -x /usr/local/bin/mise; then
    curl --proto '=https' --proto-redir '=https' --fail --silent --show-error \
        --location https://mise.run --output "$stage/install-mise.sh"
    MISE_INSTALL_PATH=/usr/local/bin/mise sh "$stage/install-mise.sh"
fi
printf '%s' "$SELF_HOST_MISE_TOML_B64" | base64 --decode > "$stage/mise.toml"
printf '%s' "$SELF_HOST_COMMAND_B64" | base64 --decode > "$stage/command"
chmod 0644 "$stage/mise.toml"
as_dev() {
    runuser -u "$dev_user" -- env HOME="$dev_home" \
        PATH=/usr/local/bin:/usr/bin:/bin MISE_CONFIG_FILE="$stage/mise.toml" \
        MISE_YES=1 "$@"
}
cd "$dev_home"
as_dev mise trust "$stage/mise.toml"
progress tools
as_dev mise install --yes

# What mise has no package for. The same commands a custom image runs between
# its dependencies and its checks, run here for the same reason: as root, with
# network, and with the tools mise just installed already on PATH.
progress setup
printf '%s' "$SELF_HOST_SETUP_B64" | base64 --decode > "$stage/setup"
# The last command carries no trailing newline, so read it on the failed read.
while IFS= read -r command || test -n "$command"; do
    test -n "${command// /}" || continue
    printf '+ %s\n' "$command"
    env PATH="$dev_home/.local/share/mise/shims:/usr/local/bin:/usr/bin:/bin" \
        HOME=/root bash -c "$command"
done < "$stage/setup"

progress checks
python3 - "$stage/mise.toml" "$dev_home" <<'PY'
import os, signal, subprocess, sys, tomllib
config, home = sys.argv[1:]
with open(config, 'rb') as f:
    checks = tomllib.load(f).get('tasks', {}).get('check', {}).get('run', [])
for command in checks:
    process = subprocess.Popen(['runuser', '-u', 'dev', '--', 'env', 'HOME=' + home,
                    'PATH=/usr/local/bin:/usr/bin:/bin', 'MISE_CONFIG_FILE=' + config,
                    'MISE_YES=1', 'mise', 'exec', '--', 'bash', '-c', command],
                   start_new_session=True)
    try:
        result = process.wait(timeout=60)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        raise
    if result:
        raise subprocess.CalledProcessError(result, 'environment verification')
PY
as_dev mise ls --json > "$stage/versions.json"

progress service
install -m 0644 "$stage/mise.toml" "$config_dir/mise.toml"
install -o root -g "$dev_user" -m 0640 "$stage/command" "$config_dir/command"
committed=true
runuser -u "$dev_user" -- env HOME="$dev_home" MISE_YES=1 \
    /usr/local/bin/mise trust "$config_dir/mise.toml"
cat > /etc/profile.d/self-host-environment.sh <<'PROFILE'
if [ "$(id -un)" = dev ]; then
    export MISE_CONFIG_FILE=/etc/self-host-environment/mise.toml
    export PATH=/home/dev/.local/share/mise/shims:/home/dev/.local/bin:/usr/local/bin:$PATH
fi
PROFILE
chmod 0644 /etc/profile.d/self-host-environment.sh
runuser -u "$dev_user" -- env HOME="$dev_home" \
    MISE_CONFIG_FILE="$config_dir/mise.toml" /usr/local/bin/mise reshim
cat > /usr/local/bin/self-host-environment-run <<'RUNNER'
#!/bin/sh
set -eu
export MISE_CONFIG_FILE=/etc/self-host-environment/mise.toml
exec /usr/local/bin/mise exec -- /bin/bash -c "$(cat /etc/self-host-environment/command)"
RUNNER
chmod 0755 /usr/local/bin/self-host-environment-run
cat > /etc/systemd/system/self-host-environment.service <<'UNIT'
[Unit]
Description=Self-host development environment
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=dev
Group=dev
Environment=HOME=/home/dev
Environment=PATH=/home/dev/.local/share/mise/shims:/home/dev/.local/bin:/usr/local/bin:/usr/bin:/bin
WorkingDirectory=/home/dev/repos
ExecStart=/usr/local/bin/self-host-environment-run
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT
chmod 0644 /etc/systemd/system/self-host-environment.service
systemctl daemon-reload
if test -s "$config_dir/command"; then
    systemctl enable self-host-environment.service
    systemctl restart self-host-environment.service
else
    # Nothing to run. Leave the unit in place so adding a command later is a
    # restart rather than a reinstall.
    systemctl disable --now self-host-environment.service 2>/dev/null || true
fi

progress health
if test -s "$config_dir/command" && test "$SELF_HOST_WEB_PORT" -gt 0; then
    healthy=false
    for attempt in {1..30}; do
        if curl -fsS --max-time 2 "http://127.0.0.1:$SELF_HOST_WEB_PORT/" > /dev/null; then
            healthy=true
            break
        fi
        sleep 1
    done
    "$healthy"
fi
install -m 0644 "$stage/versions.json" "$config_dir/versions.json"
progress ready
