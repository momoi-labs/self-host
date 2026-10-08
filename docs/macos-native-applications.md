# Native applications on macOS

The macOS MVP runs each Application as a dedicated non-administrator account.
The Platform API stays under the Operator. A protected helper creates accounts
and controls the existing s6 supervisor through a restricted sudo command.

The privileged fixture passes in CI and on a macOS 26.6.2 Apple Silicon Mac.
The console, LAN and reboot acceptance below must pass before publishing a
release with macOS native support.

## Install

Use the complete macOS release archive with `install.sh`. Administrator
authorization installs these files once:

| Component | Location |
| --- | --- |
| Protected helper | `/Library/PrivilegedHelperTools/dev.momoi.self-host` |
| Private s6 tools | `/Library/PrivilegedHelperTools/dev.momoi.self-host.s6` |
| Restricted Operator grant | `/private/etc/sudoers.d/self-host-native` |
| Root scan LaunchDaemon | `/Library/LaunchDaemons/dev.momoi.self-host.native.plist` |
| Service definitions and logs | `/Library/Application Support/self-host/native` |
| Application data | `/Library/Application Support/self-host/native-data/<id>` |

The sudoers rule permits only `native-control`, with no extra command-line
arguments. It does not authorize a root shell. The helper validates its JSON
request, derives its paths and account name, and clears each child's inherited
environment. Application accounts have no administrator or sudo grant.

The installer rejects symlinked or writable runtime directories. It does not
adopt an existing account without the expected marker, home, group and login
policy. Application deletion stops the service, secures retained data as root
with mode `0700` and retires the account: its records stay with a retired
marker, so the UID is never reused and nothing can run as it. A new
Application needs a new id rather than adopting retained data. `uninstall.sh`
removes accounts, the supervisor, helper and grant, while retaining protected
native data and logs.

Deletion retires rather than deletes because macOS only deletes a user when
the responsible process has Full Disk Access, which the Platform daemon
cannot be granted durably
([#168](https://github.com/momoi-labs/self-host/issues/168)). From a
terminal, macOS asks the terminal app for that access instead, which is how
`uninstall.sh` and the fixture below delete live and retired accounts.

## Supported behavior

Use the existing Native process form for dependencies, setup, a foreground
command, Variables and an optional HTTP port. HTTP listeners must bind to
`127.0.0.1`; socket readiness also checks the account and active service group.
The existing proxy and DNS publish the Web Target on the LAN. Unpublished
workers use the same lifecycle without an HTTP listener.

Setup runs under s6 and the Application Account, including mise installation.
Reapplying an unchanged recipe reuses its receipt. A stopped Application stays
stopped after configuration changes. An interrupted setup requires reapplying
the configuration. Restarting the API reconnects to the independent supervisor.

The console hides native resource limits, metrics, terminal access and managed
PostgreSQL connections on macOS. Direct API requests for unsupported controls
are rejected. Git source builds, GUI applications and commands that detach
with `setsid` or move children into another process group are outside the MVP.
Process groups do not provide Linux's cgroup whole-tree or resource guarantees.
Darwin also has no verified equivalent of Linux `no_new_privs` in this launcher.

## Validate on a disposable Mac

The fixture refuses to overwrite an existing native installation. It asks for
administrator authorization, installs only native supervision, runs integration
tests as the Operator, and removes its accounts and files on exit. It does not
configure Docker, DNS, the Platform daemon or virtual machines. Removing an
account prompts for Full Disk Access for the terminal app; allow it, or the
fixture waits.

```sh
cd console && npm ci && npm run build && cd ..
cargo build
./scripts/release-s6.sh aarch64-apple-darwin
SELF_HOST_NATIVE_TEST=1 bash scripts/test-native-macos.sh \
  target/debug/self-host target/aarch64-apple-darwin/release/s6
```

Use `x86_64-apple-darwin` on an Intel Mac. Release builds use a native runner
for each architecture. CI includes the privileged fixture on Apple Silicon;
an Intel build alone does not establish runtime acceptance.

The fixture checks distinct non-root identities, supplementary groups, failed
root recovery, private files, mise setup, Variables, log redaction, crash
recovery, child cleanup, stop/start/restart, account retirement, that a
retired id is refused and its number never reused, and retained data.

## Validate the console and LAN release

1. Start the real console with `cargo run -- serve` after installing the native
   runtime. Open the console and choose Deploy application, then Native process.
2. Add Node 24, a Variable named `MVP_MESSAGE`, and this foreground command:
   `node -e 'require("node:http").createServer((q,r)=>r.end(process.env.MVP_MESSAGE)).listen(18765,"127.0.0.1")'`.
   Set the HTTP port to `18765` and deploy. Expect Running and an `sf-app-*`
   account in Summary. Resource limit inputs and Terminal must be absent.
3. Open the Hostname from another LAN device. Expect the Variable's value.
   Change the Variable and save. Expect the new value after the task completes.
4. Open Logs, then use Stop, Start and Restart. Expect each task to complete,
   the HTTP endpoint to follow that state, and no service children after Stop.
5. Leave one Application running and another stopped. Restart the Platform,
   then reboot the Mac without logging in. Expect both intents to survive and
   the running Application's Hostname to answer from another LAN device.
6. Delete the fixtures. Expect each task to complete without any prompt,
   `dscl . -read /Users/sf-app-<id> RealName` to answer
   `self-host retired Application <id>`, `pgrep -U <uid>` to find nothing,
   and the retained data to be owned by root with mode `0700`. A new native
   Application gets a different account number.

Record the macOS version, architecture and results before release. Do not
publish based only on compilation or the unprivileged s6 smoke test.

## Inspect failures

```sh
sudo launchctl print system/dev.momoi.self-host.native
sudo tail -100 '/Library/Application Support/self-host/native/scan.log'
```

The API reports helper installation or authorization failures as task errors.
The capability response at `/native/capabilities` lets the console explain a
missing helper before deploy. The macOS s6 build applies a pinned skalibs
`select` readability patch; `scripts/test-s6-runtime.py` reproduces the upstream
shutdown failure and verifies its correction.
