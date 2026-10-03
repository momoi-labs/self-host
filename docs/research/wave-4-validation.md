# Wave 4 validation

Wave 4 implements N4, P2 and D3. Validation used a disposable Linux Host,
synthetic Applications and the rebuilt console served by `cargo run -- serve`.
No production inventory, credentials or configuration are part of this report.
This is implementation evidence, not migration or release approval.

## Delivered behavior

| Slice | Result |
| --- | --- |
| N4 | Native creation and configuration, masked Variables, logs, terminals and cgroup metrics. Each Application has a dedicated non-root account and private mise dependencies, versions, caches and setup commands. |
| P2 | Managed PostgreSQL 16/17/18, durable storage, readiness, private consumer connections, explicit credential reveal, revocation and guarded logical import. |
| D3 | Manual Git deployment by default, optional scoped trigger per Application, durable deduplication, health gates and explicit recovery from recorded immutable images. |

Native recipes reuse the custom-image dependency and shell-command editors.
Preparation runs under the Application's account and resource limits. Main
processes and terminals use that same environment. A successful recipe is
cached across lifecycle and Variable changes; changing it applies setup again.
Stopped Applications stay stopped. Failed preparation blocks Start until the
Operator reapplies a valid configuration.

## Automated checks

| Check | Result |
| --- | --- |
| `cargo test -j3` on macOS | 553 tests passed. Linux-only checks are listed separately below. |
| `cargo clippy --all-targets -- -D warnings` | Passed. |
| `cargo fmt --check` and `git diff --check` | Passed. |
| Console `npm run build` and `npm test` | Typecheck, build and 58 tests passed with Kiso React 0.11.0 and Kiso 0.15.0. |
| Privileged native API, launch, supervision and source-build guard suites | 19 tests passed on Linux, explicitly enabling ignored tests. |
| Git API and Git build guard fixtures | 6 tests passed on Linux, including scoped triggers, image recovery and healthy/unhealthy image refresh. |
| `scripts/test-managed-postgres.py` | Passed with real PostgreSQL 18, versioned volume layout, persistence after restart/recreation, private access, logical import, data guards and credential revocation. |
| `scripts/test-native-postgres.py` | Passed again after mise integration: non-root SQL client, loopback transport, restricted role, stopped connection edits and revoked-login refusal. |
| `scripts/test-native-mise.py` | Passed with real Node 22 and 24 installations, separate accounts/homes/cgroups, mutual configuration-read refusal, stopped updates, cached setup, failure/repair and redacted logs. |

The native Linux regressions also cover attempts to redirect setup through
home symlinks and cleanup of preparation processes left by an interrupted
daemon. They use a fake mise executable to make those failure cases repeatable;
the separate mise script exercises real downloads and runtimes.

The real PostgreSQL import fixture exposed an archive-header parsing error.
The parser now reads the actual `pg_restore --list` header format while still
rejecting missing or incompatible version metadata. The full fixture passed
after that correction.

Concurrent console actions exposed a private-network reconciliation race when
an unrelated consumer container was recreated. Membership changes now use the
inspected container ID and accept only confirmed removal or an already-applied
change. Six regressions cover the race and preserve real network failures.

Names now retain capitals, spaces, Unicode and punctuation. Generated Hostnames
use DNS-safe labels and avoid occupied names. Tests cover each Application
source, persistent names, CLI access and ID-based Variable/removal operations.
A queued Variable edit stays on its original Application after a rename and
name reuse. A recovery regression also proved that a concurrent rename could
be overwritten; recovery now shares the metadata update lock.

## Console checks against the real backend

- Created a native Application with `node@24`, setup commands and a directory
  created during setup. Logs showed preparation followed by the main process.
- Added `npm:semver@7` through the same form and ran the installed command
  during non-root setup, verifying package installation alongside the runtime.
- Confirmed that the terminal used the same installed Node version, private
  path, working directory and nonzero UID as the main process.
- Reused an existing custom-image recipe for a native code workspace. Tool
  installation, terminal commands and the server ran under the Application
  Account and its cgroup. HTTP, the embedded HTTPS route and browser pairing
  passed. Provider authentication remains an Operator step.
- Edited dependencies and used Discard. The saved version, setup and argv
  remained intact. Saving a setup edit opened Logs and applied the new recipe.
- Rechecked the native form with seven creation steps, Kiso controls and one
  Start command field. Invalid quotes showed an inline error. Discard restored
  the command; saving a resource limit preserved its exact argv. Creating a
  published Application saved its Variable and returned HTTP 200 through the
  embedded proxy. Both publication choices showed the expected fields.
- Moved native Variables into Configuration, with Kiso dialogs for add, replace
  and remove. Verified masked values, empty replacement fields and no nested
  forms against the real API.
- Created PostgreSQL 18 from Databases, observed recorded provisioning stages,
  connected a native consumer and exercised connection reveal/hide and removal.
- Matched the Kiso list composition for databases, connections and deployments:
  separate header action, search/status filters, primary and secondary row text,
  compact action menus and detail dialogs. Internal database identifiers remain
  in connection details. Database navigation uses its own icon above DNS.
- Reviewed the new list, create and detail screens with `skills/add-page`.
  Checked empty filters, menus, dialog padding, cancellation and the sticky
  native save bar. At 800 px, lifecycle actions wrap below the heading instead
  of squeezing the title and metrics; desktop headers still fit at 1280 px.
- Verified that managed databases leave the Applications list/count and Events
  identifies them as databases, including existing history.
- Created and removed a database with a capitalized name through the console.
  Separate Linux fixtures verified generated Hostname collisions, Unicode,
  punctuation and surrounding spaces, rename identity, Variables and removal.
  Renaming the native code workspace preserved its account and Hostname.
- Verified the Git trigger's manual default, enable/disable controls and a
  real trigger request. Recovered a previous deployment through its confirmation
  dialog and checked the recorded image identity.
- Checked native, database and deployment screens in light/dark themes at
  desktop and narrow widths. Tables and recipe controls stayed within the
  viewport at 1280 px and 800 px.
- Rebuilt and served the console with Kiso React 0.11.0 and Kiso 0.15.0.
  Database lists and creation, native configuration and lifecycle controls
  remained usable at 1280 px and a 390 px phone viewport.
- Applied the appearance settings to the console, login and DNS setup pages.
  Verified the native Start command editor's highlighting, line numbers and
  Discard behavior. Git creation and configuration use the shared numbered
  steps, with publication fields in their own step and stacked fields on phones.

## Remaining release work

Wave 5 still owns W3 public routing/certificate/console management and R1
backup/restore. Wave 6 owns V1: a selected release, fresh reboot and restoration
rehearsal, and a separate migration decision. This validation does not replace
those gates or perform a live cutover.

Native setup is not transactional. A failed command can leave files behind;
retry commands must tolerate that state. Cgroups bound resources, while account
permissions control file access. Native Applications still share Host networking.
Terminals expose the Application's environment to the authenticated Operator.

Managed database major upgrades and automatic data rollback are outside P2.
Deployment recovery selects recorded images; it does not rewind database data,
Application Variables, routes or connection grants.

See [native Applications](../native-applications.md),
[managed PostgreSQL](../managed-postgresql.md), and
[deployment triggers and recovery](../deployments.md) for the operating contracts
and repeatable fixture commands.
