# Native lifecycle validation

N3 passed fresh Linux HTTP API, N1 identity and N2 supervision checks on a
disposable Host. No production state or traffic changed.

## Native API

`tests/native_api_linux.rs` passed all three ignored tests with real s6,
cgroup v2, fresh dedicated accounts, file-backed Platform State and an HTTP
listener. Docker is an unused fake boundary in this native fixture.

The tests checked:

- Non-root uid, empty supplementary groups, zero capabilities and
  `NoNewPrivs`, plus CPU, memory and task limits.
- Create, start, stop, restart, definition updates, Variable changes, task
  audit outcomes, observed status and the SSE log API.
- Stopped updates without a launch, daemon reconnect without an extra start,
  interrupted running work and requeued work that had never started.
- A private loopback Web Target, an unpublished worker and startup failure
  for a listener bound to all addresses.
- Removal of supervision and the dedicated account. Data remained intact in
  a root-owned `0700` home.

## N1 and N2

The same fresh Linux run passed all ten `native_linux` tests and the one
`native_supervision_linux` test. These cover privilege enforcement for every
launch purpose, account provisioning, filesystem access, cgroup limits,
detached descendant cleanup, crash recovery, protected supervision files,
log rotation and stopped intent.

```sh
cargo test --test native_api_linux --test native_linux --test native_supervision_linux -- --ignored --test-threads=1
```

The native worktree passed Linux and macOS Clippy. The integrated snapshot
passed all 14 privileged tests again with fresh fixtures. Its macOS library
suite passed 422 tests after building the console. `git diff --check` and
the manual unslop review passed for the edited operating guide.
This checkout has no `bin/validate-prose` command; plain punctuation was
checked directly.

## Integrated daemon

The combined daemon passed the native Web Target test through its real HTTPS
proxy. The request verified the fixture LAN CA and returned the expected
HTTP 200 body. The Platform allocated no Docker Host port.

`examples/native_api_reboot.rs` creates fresh Applications and requires
exactly two starts for the running Application and one for the stopped
Application after the boot id changes. Existing or accumulated counters
cannot pass that check. The fresh integrated check passed with starts=2 and
starts=1, retained data, non-root identities and zero capabilities. Its
result is recorded in the
[wave 3 validation report](wave-3-validation.md).

Native console configuration, terminal and metrics remain outside N3.
