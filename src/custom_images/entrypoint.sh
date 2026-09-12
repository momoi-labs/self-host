#!/bin/sh
# Docker starts this as root so an attached volume can be handed to dev. The
# configured Application command always runs as dev after this small setup.
set -eu

user=dev
uid=$(id -u "$user")
gid=$(id -g "$user")
marker=/data/.self-host-dev-ownership-v1

mkdir -p /data

# Volumes created by older images may contain root-owned state. Migrate once,
# then keep a root-owned marker so an ordinary dev process cannot forge it.
# Do not follow a link a process in the dev-writable volume could have left.
if [ ! -f "$marker" ] || [ -L "$marker" ] || [ "$(stat -c '%u:%g:%a' "$marker")" != "0:0:600" ]; then
    chown -R "$uid:$gid" /data
    temporary_marker=$(mktemp /data/.self-host-dev-ownership-v1.XXXXXX)
    chown root:root "$temporary_marker"
    chmod 0600 "$temporary_marker"
    mv -fT "$temporary_marker" "$marker"
fi

# Docker can create WORKDIR as root after the migration. Correct only the
# runtime directories on later starts instead of traversing the whole volume.
for dir in /data/home /data/t3home /data/repos; do
    mkdir -p "$dir"
    if [ "$(stat -c '%u:%g' "$dir")" != "$uid:$gid" ]; then
        chown "$uid:$gid" "$dir"
    fi
done

exec gosu "$user" "$@"
