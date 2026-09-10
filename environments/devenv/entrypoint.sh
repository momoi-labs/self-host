#!/bin/sh
# Runs as root for as long as it takes to hand /data to the user, then never
# again. A named volume is created root-owned, and a container that dropped
# privileges in the image would find its own home unwritable on first boot.
set -eu

user="${DEVENV_USER:-dev}"

# WORKDIR creates /data/repos as root on every start, before anything here
# runs, so this is not only about a volume the container has never seen.
for dir in /data /data/repos; do
	mkdir -p "$dir"
	if [ "$(stat -c %U "$dir")" != "$user" ]; then
		chown "$user:$user" "$dir"
	fi
done

exec gosu "$user" /usr/local/bin/devenv-first-boot "$@"
