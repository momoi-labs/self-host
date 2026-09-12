# Development environments

Development environments are Incus system containers. They run Ubuntu,
systemd, SSH, mise and T3 as native Linux processes. The Platform manages
only instances named `sf-dev-<environment-id>` and carrying its ownership
marker.

## Mac with Colima

Install the host clients:

```sh
brew install colima incus
colima start self-host-incus --runtime incus --template=false \
  --mount none --network-address --activate=false \
  --cpus 2 --memory 4 --disk 20
```

Configure an Incus client remote for this profile. Keep this client config
separate from any other Incus setup:

```sh
export INCUS_CONF="$HOME/.config/incus-self-host"
mkdir -p "$INCUS_CONF"
incus remote add self-host "unix://$HOME/.colima/self-host-incus/incus.sock"
incus remote switch self-host
incus version
```

Set `SELF_HOST_INCUS_REMOTE=self-host`. Set
`SELF_HOST_INCUS_ADDRESS` to the address shown by `colima list` for the
`self-host-incus` profile, such as `192.168.64.3` after verifying your own
output. The address must be reachable from the client running SSH and must be
usable for Incus proxy listeners. `--network-address` gives the outer Colima
VM a host-reachable address.

The runtime uses `SELF_HOST_INCUS_BIN` when set. Otherwise it uses
`/opt/homebrew/bin/incus` on macOS when that path exists, then `incus` from
`PATH`. `SELF_HOST_INCUS_POOL` selects an Incus storage pool. The default pool
must support the requested root disk size.

Set `SELF_HOST_ENVIRONMENT_HOST` when the SSH client must first connect to a
gateway or VPN host. The runtime then prints SSH and web tunnel commands that
use that host. Keep the tunnel over the existing VPN. It does not depend on
the Platform HTTP proxy, but the Host and Incus server must be running.

## Linux

Install Incus and make its daemon reachable by the Platform's service account.
The account needs access to the configured Incus remote, the `images` Ubuntu
remote must be available, and the selected storage pool must support root
disk sizing. Configure `SELF_HOST_INCUS_REMOTE`,
`SELF_HOST_INCUS_ADDRESS`, and optionally `SELF_HOST_INCUS_POOL` as above.
The Linux setup and its service account permissions are currently untested.

The runner uses `images:ubuntu/24.04`. The observed image fingerprint is
recorded in Platform State after creation. The alias is mutable, so the
fingerprint is useful for inspection and does not make future creation
reproducible.

## Local console harness

Run the harness with:

```sh
cargo run --example environments-local
```

Open `http://127.0.0.1:13721/console` and sign in with
`local-environments`. The harness uses a file state directory under the
system temporary directory unless `SELF_HOST_ENVIRONMENTS_STATE_DIR` is set.
Environment actions use the real Incus runner. Application actions use fake
Docker and cannot change the Host's production workloads.
The local harness does not replace an end-to-end Incus smoke test on the
target Host.

For a memory-only console demo, append `?demo=environments#environments` to
the console URL. It does not call the Incus runner.

After connecting over SSH, run
`t3 pair --base-dir /home/dev/.local/state/t3` and paste the one-time token
into the T3 page opened through the tunnel. Sign in to Claude or Codex through
their CLI inside the environment. Pairing tokens and provider credentials
do not belong in the saved recipe or verification commands.

## Lifecycle and data

Create starts one owned system container, applies CPU, memory and root disk
limits, configures SSH and installs the selected recipe. Repeating create
inspects the owned instance first, so an interrupted create can continue.
Start, stop and restart preserve the instance disk. Bootstrap and update run
the mise recipe, checks and T3 service again. Update does not upgrade Ubuntu,
resize the container, or uninstall tools removed from the recipe.

Repositories, credentials and T3 state stay on the instance disk. Delete
requires the environment name and removes that disk with the container. A
failed deletion remains visible for retry. SSH and tunnels remain usable when
the Platform process is stopped, provided the Host and Incus server remain up.
