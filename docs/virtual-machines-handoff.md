# Continue the virtual machines prototype

The console calls these resources "Virtual machines". The implemented runner
creates Ubuntu 24.04 Incus system containers with a shared Linux kernel.
The Operator accepted this boundary. See [ADR-0022](adr/0022-development-environments-use-incus.md).

## Current implementation

The normal `cargo run -- serve` includes the real `/environments` API and
Incus runner. The detail screen has Configuration, Operation and Connect to
console tabs. Recipes reuse the development image tool editor and install
through mise. systemd runs T3 as `dev`.

Create, bootstrap, start, stop, restart, update and confirmed deletion work.
Configuration changes are saved before an explicit Apply update action.
Operation state persists, and interrupted operations can be retried.

## Start on another computer

```sh
npm --prefix console ci
npm --prefix console run build
cargo test --no-fail-fast
```

Configure Incus using [the setup guide](development-environments.md). The
Platform account needs an Incus client and access to the selected server.
The most recent local create attempt failed because the Linux machine had
no Incus executable. Retrying after configuration should resume that operation.

`just setup-local` initializes `prototype.lan`, trusts its CA and starts the
normal server. It uses this computer's Platform state. On Linux, the built
binary needs permission to bind ports 53, 80 and 443. The tested command was
`sudo setcap cap_net_bind_service=+ep target/debug/self-host`; rebuilding can
remove it. With the server running, `cargo run -- setup-dns` configures Host
DNS. The local setup script uses the development API key `local`.

The optional `cargo run --example environments-local` harness uses separate
temporary state and no DNS listener. Its console is on port 13721 with key
`local-environments`. `?demo=environments#environments` enables simulated UI
actions. The preferred next step is to simplify testing through normal `serve`.

## Existing remote experiment

On `home-server`, Colima profile `self-host-incus-test` hosts Incus separately
from production. The last validated environment was `t3-incus-prototype`.
The remote name was `colima-self-host-incus-test`, and the outer VM address
was `192.168.64.3`. Recheck `colima list` before reusing that address.

An isolated checkout on that Mac was `/Users/seba/self-host-environment-prototype`.
Its API harness listened on loopback port 13721. SSH tunnels on the original
Linux computer exposed it on port 13722 and T3 on port 43824. These are temporary
processes, not installed production services or portable access URLs.

Git transfers source and documentation. It does not transfer Platform state
under `~/.config/self-host`, Incus disks, SSH private keys, trusted CAs, client
remote configuration or provider sign-ins. Use the new computer's SSH key and
configure access there. Never add private keys or personal runtime state to Git.

## Remaining work

* Configure and validate native Linux Incus through the normal server.
* Finish agent sign-in and run a real authenticated task in T3.
* Simplify local startup, including DNS permissions and Incus prerequisites.
* The base OS is fixed to Ubuntu 24.04. Other distributions are not implemented.
* Docker nesting was only a separate feasibility test and is not enabled by
  this runner. Running this project's Docker workloads inside a new environment
  needs separate validation.

The real Mac lifecycle, persistence, failure retry, SSH independence and browser
pairing results are in [the validation report](research/incus-development-environments.md).
