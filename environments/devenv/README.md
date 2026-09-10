# Development environment

One environment per person on the Host, with [T3 Code](https://t3.codes)
inside it, reached through the Platform's proxy. The design and its test
results are in [remote development
environments](../../docs/research/remote-development-environments.md);
this directory is that design as files.

## Two parts that must not merge

| | Image | Volume |
| --- | --- | --- |
| Contents | T3 Code, the agent CLIs, Git, the toolchains | Repositories, T3 Code state, SSH key, credentials |
| Where | `Dockerfile`, shared by everyone | `/data`, one per person |
| Replaced by | A rebuild | Nothing; it is the thing to back up |
| Credentials | Never | Provisioned on first boot |

A credential in the image is a credential shared between people, which
defeats the separation the per-person environment exists to provide.

## What is in the image

The base is `jdxcode/mise`, which is the official Rust image with mise on
top: Debian 13, mise with its shims already on `PATH`, Rust 1.97.1, Node
24.21.0 and Python. Those are the versions pg-probe's `rust-toolchain.toml`
and this repository's `mise.toml` ask for, and the build fails if a base
bump moves Rust out from under them.

`mise.toml` here is the rest: Zig, cargo-zigbuild, GoReleaser and `gh`.
Adding a tool is a line in that file. On top of it, `rustup` gets clippy,
rustfmt and both musl targets, apt gets `sqlite3`, `ripgrep`, `tmux` and
`gosu`, and npm gets T3 Code, Claude Code, Codex and opencode, each pinned.

Toolchains live in the image, where a rebuild replaces them. What is worth
carrying between containers goes to the volume: `HOME`, the Cargo crate
cache and the npm prefix all resolve under `/data`. `RUSTUP_HOME` and
mise's data directory deliberately do not.

Two things this arrangement needed, both found by running it:

- `MISE_TRUSTED_CONFIG_PATHS` is set in the image. mise's trust is per user
  state, the image is built by root and run by someone else, and without it
  every config is ignored and every shim answers `No version is set`.
- `/etc/profile.d/devenv.sh` repeats the environment. A login shell rebuilds
  `PATH` from `/etc/profile`, and the terminal T3 Code opens is a login
  shell.

## Build it on the Host

The Platform refuses `build:` in a Compose Application, so the image is built
by hand where it will run. From this directory, on the Workstation:

```sh
rsync -a ./ home-server:~/tmp/devenv/
ssh home-server 'cd ~/tmp/devenv && docker build -t momoi/devenv:0.1.0 .'
```

Bump the tag in `compose.yml` when the image changes. Rebuilding under the
same tag is not enough on its own: `self-host apps stop` and `start` restart
the container that exists, and only a recreate picks up a new image.

## Deploy it

```sh
self-host apps add --name devenv \
	--compose-file ~/tmp/devenv/compose.yml \
	--web-service devenv --web-port 3773
```

The environment then answers on `devenv.<suffix>` through the Platform, and
on port 3773 of the Host directly for when the Platform is not running.

Set the Git identity once, per person:

```sh
self-host apps env set devenv DEVENV_GIT_NAME='Your Name'
self-host apps env set devenv DEVENV_GIT_EMAIL='you@example.com'
```

## Enrol a device

The token printed at startup expires in five minutes, which is useless on a
Host that stays up for weeks. Mint one deliberately, and give it time to be
typed:

```sh
docker exec -u dev sf-app-<id>-devenv t3 pair --base-dir /data/t3home --ttl 2h --label iphone
```

An expired token is reported by the client as an invalid credential, not as
an expired one, so a rejected pairing usually means the clock, not the token.

## First boot

The first boot generates an SSH key on the volume and configures Git to sign
commits with it. Every later boot finds the marker and only starts the
server. The public key is printed on every start:

```sh
self-host logs devenv | grep 'ssh public key'
```

Enrol it once, from a machine with `gh` authenticated:

```sh
gh ssh-key add key.pub --title devenv
gh signing-key add key.pub --title devenv
```

Provider authentication for the agent CLIs is one browser step per person,
after which the token sits on the volume. Codex serves its login callback on
`localhost:1455`, so that port has to be reachable from wherever the browser
runs.

## What the limits cost

`compose.yml` gives the environment 6 CPUs and 12 GiB of the VM's 8 and 24.
Measured inside it, on 10 September 2026: `cargo test` on this repository
compiled 223 crates in 17 seconds from a clean target directory, and the
container's peak memory was 4.77 GiB.

That headroom is why the VM was resized first. On the 6 GiB it had before,
shared with the Operator's other Applications, that peak had nowhere to go.

Note that `nproc` and `free` inside the container report the VM's 8 CPUs and
24 GiB, not the limits. Cargo sizes its job pool from what it sees, so a
heavier project may need `CARGO_BUILD_JOBS` set to match.

## What this does not solve

Backups. A named volume lives inside the Colima VM's disk image, where Time
Machine does not see it, and copying a live SQLite database with an open
write ahead log is not a backup. Uncommitted work and thread history sit
outside every existing backup until that is built. Push often.
