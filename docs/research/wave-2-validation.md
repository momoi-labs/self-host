# Wave 2 validation

W1, N2 and P1 are implemented and integrated on the base containing PR #161,
commit `a084efaf4ff489c664194f989992034c4489d4ea`. Validation ran on
2026-09-30. All fixtures used disposable state and synthetic data. No
production changes or releases were made.

## Scope and review files

| Task | Result | Main files |
| --- | --- | --- |
| W1 | Persisted loopback routes, segment matching, longest-prefix precedence, explicit prefix stripping and hot updates. Conflicts return 409. Management and ACME paths remain reserved. | `src/routes.rs`, `src/proxy.rs`, `docs/routing.md` |
| N2 | Protected s6 services, N1 launches, readiness, restart recovery, descendant cleanup, rotating logs and boot delegation. | `src/native/supervision.rs`, `src/native/{cgroup,launch}.rs`, `assets/linux/self-host-native.service`, `install.sh`, `tests/native_supervision_linux.rs`, `examples/native_reboot.rs` |
| P1 | Explicit external-volume mapping, safe file/directory binds, private provider networks, consumer grants and health reporting for unpublished services. | `src/compose_app.rs`, `src/compose_app/storage.rs`, `src/connectivity.rs`, `src/docker.rs`, `tests/stateful_docker.rs` |

Shared integration covers `src/{store,file_store,apps,lib,tasks,main}.rs`,
DNS ownership in `src/dns_records.rs`, fixture fields in `src/terminal.rs`,
and API persistence/concurrency tests in `tests/file_store_api.rs`.
Contracts and operating instructions are in `docs/deployment-contracts.md`,
`docs/native-applications.md` and `docs/compose-applications.md`.

N2 resolved the privilege decision before adding supervision.
[ADR-0031](../adr/0031-application-accounts-are-execution-identities-and-cgroups-are-resource-controls.md)
retains privileged daemon/supervisor setup. Every Application command,
including readiness, goes through N1 and drops to its dedicated non-root
account before execution. A separate privileged helper remains deferred.

## Automated checks

The integrated checkout passed:

| Check | Result |
| --- | --- |
| Console `npm ci && npm run build` | Passed. Existing bundle-size warning only. No console source changes. |
| macOS `cargo test --locked` | 446 passed. Linux-only fixtures are excluded on macOS. |
| Linux `cargo test --locked` | 444 passed. The 13 privileged fixtures were run separately below. |
| Linux N1, `native_linux -- --ignored --test-threads=1` | 10 passed. |
| Linux N2, `native_supervision_linux -- --ignored --test-threads=1` | 1 passed with real s6 and cgroup v2. |
| Linux P1, `stateful_docker -- --ignored --test-threads=1` | 2 passed with real Docker and PostgreSQL. |
| macOS and Linux `cargo clippy --all-targets --locked -- -D warnings` | Passed. |
| `cargo fmt --check`, `git diff --check`, `bash -n install.sh` | Passed. |

Linux used a fresh Ubuntu 24.04 aarch64 VM without Host directory mounts,
kernel 6.8.0-134, s6 2.12.0.3, Docker 29.1.3 and Compose 2.40.3.
A final clippy expression simplification was followed by all 45 Compose
tests on both platforms, both clippy checks and a rebuilt backend.

W1 fixtures cover queries, redirects, assets, streamed requests and responses,
authenticated WebSockets, prefix limitations, removal ownership and reserved
challenge paths. API tests cover persisted rules and concurrent updates.
Shared-hostname DNS tests keep the answer until its last owner is removed.

N2 checks cover main/readiness identities and capabilities, protected wrappers,
crash recovery, detached descendants, log rotation, reconnect without duplicate
processes, stopped intent after scanner restart and unhealthy startup errors.

P1 Docker fixtures verify retained rows through restart, redeploy and removal,
external-volume adoption, file and directory mounts, read-only mounts, allowed
and denied container connectivity, and a loopback-only native connection using
N1. Integration tests also cover grant revocation after a failed deployment,
retry reconciliation and grant changes while the provider stays stopped.

## Real Host reboot

The N2 worktree prepared running and stopped fixtures, then rebooted the
disposable Linux Host. The verifier checked a changed kernel boot ID,
persisted intent, nonzero UIDs and unchanged data. The integrated checkout
rebuilt and reran the verifier with this result:

```text
reboot-running: persisted intent restored, dedicated uid, retained data, starts=2
reboot-stopped: persisted intent restored, dedicated uid, retained data, starts=1
```

The installed N2 supervisor remained in place through the reboot. The final
integrated supervision test separately exercised the current binary.
Later inspection found four starts in the retained running fixture, so its
strict two-start reboot verifier no longer passes on that reused state. The
isolated supervision test was rerun and passed. A fresh reboot check must
prepare fresh fixtures on another disposable Host.
See [the reboot procedure](../native-applications.md#supervision-validation)
for reproduction on another disposable Host.

## Real backend and console

The final backend runs from the repository root with `cargo run --locked --
serve --no-dns`. Only the disposable VM's configuration and Docker state are
used. The console is available at <http://127.0.0.1:13721/console/> through
local port forwarding. HTTP and HTTPS proxy fixtures use ports 13080 and
13443. DNS setup was deliberately omitted; HTTPS probes used an explicit
hostname resolution and the generated test CA.

Observed through the real backend and Chrome:

- The Overview lists `wave2-web` and `wave2-db` as running. The database has
  no Hostname, web target or published Docker port. Its API health is healthy.
- Stop and Start on `wave2-db` update the console status. Logs show PostgreSQL
  accepting connections. The synthetic database row remains `preserved`.
- Revoking and restoring the database's consumer grant changes the running
  web container's network membership while the database remains stopped.
- A route update returns 200 without restarting the web container. HTTPS `/`
  reaches nginx, `/app/asset.js?q=kept` reaches a second local target as
  `/asset.js?q=kept`, and `/apple` stays with nginx. The HTTP ACME namespace
  returns its reserved 404 instead of redirecting or reaching an Application.
- Restarting the Platform backend retains both Applications and the saved
  route. A native creation request returns 501.

To repeat the console check, open Overview, select `wave2-db`, click Stop and
wait for `stopped`. Click Start and wait for `running`. Open Logs and expect
PostgreSQL startup output. Open `wave2-web` and confirm it remains running.
The fixture login is kept in the local session handoff, outside the repository.

## Limits and blockers

No wave 2 validation blocker remains. Native lifecycle API requests stay
disabled until N3. Public certificate issuance, route editing screens and
PostgreSQL provisioning screens belong to W2/W3/P2.

The existing console still shows web configuration fields and `HTTP unknown`
for an unpublished database. Its lifecycle controls and logs work; use the API
for the new route and network-policy fields. No console screen was changed.

Loopback restricts native database listeners to the Host, not to a single UID.
Applications need database credentials through explicit Variables. Automatic
credential provisioning and rotation belong to P2. Existing rows keep their
shared network policy; new Applications default to private networks. Switching
a legacy single-container Application's policy is refused in this slice.

Storage adoption maps an explicitly named existing volume. It does not copy
data or adopt live workloads automatically. Prefix-incompatible upstreams need
their own base-URL configuration; the proxy does not rewrite response bodies.
