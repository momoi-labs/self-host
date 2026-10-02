# Wave 3 validation

W2 certificates, N3 native lifecycle and D2 Git builds passed local integrated
checks on 2026-09-30. The Git form passed against the real Linux daemon.
Public ACME staging remains blocked by the Operator's decision to keep the
available production Host out of the test. Local ACME success is not public
CA acceptance.

## Scope and integration base

The implementation starts at updated `origin/main`, commit
`8d46bfda9c2cb88f90d3fbcdf6a551dae209467f`, which contains merged PR #162.
The integrator agreed on certificate persistence and renewal, native dispatch,
Git records and task payloads before feature edits. Three agents worked in
separate worktrees on that same base:

| Task | Preserved worktree |
| --- | --- |
| W2 | `/tmp/self-host-wave3-w2` |
| N3 | `/tmp/self-host-wave3-n3` |
| D2 | `/tmp/self-host-wave3-d2` |

The integrator applied their shared-file patches serially. At this checkpoint,
changes remained uncommitted in the integration worktree. No production,
traffic, public DNS, release or GitHub publication occurred. W3, N4, D3 and P2
remain outside its scope. Fixtures use synthetic names and values.

The operating contracts are in [public certificates](../public-certificates.md),
[native Applications](../native-applications.md) and
[Git source builds](../git-source-builds.md). The form uses the existing
creation and detail layouts reviewed with `skills/add-page/SKILL.md`.
Documentation received an unslop review. This checkout has no
`bin/validate-prose`; punctuation was checked directly.

## Automated checks

The disposable Linux VM uses Ubuntu 24.04, aarch64, cgroup v2, s6, Docker 29.1.3
and Compose 2.40.3. It has independent Platform State and no workspace mounts.
A file manifest confirms that all 52 changed code, test and console bundle
files match the integration worktree. The console was rebuilt before Cargo.

| Check | Result |
| --- | --- |
| macOS Cargo suite | 466 passed, including 422 library tests |
| Linux `cargo test --all-targets --locked` | 463 passed, including 419 library tests |
| macOS and Linux Clippy, all targets, `-D warnings` | Passed |
| Console type check and production build | Passed |
| Console tests | 39 passed |
| Explicit Linux N1, N2 and N3 fixtures | 14 passed |
| Explicit Linux Git API, BuildKit and static launcher fixtures | 4 passed |
| Existing P1 Docker storage and private-network fixtures | 2 passed |
| Fresh native API reboot fixtures | Passed, running starts=2 and stopped starts=1 |
| `git diff --check` | Passed |

The ordinary Linux Cargo run ignored 20 privileged integration tests. All
20 ran explicitly in the three fixture groups above. macOS cannot run those
Linux-only fixtures; its Cargo result does not stand in for them.

The integrated commands included:

```sh
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked --test native_api_linux --test native_linux --test native_supervision_linux -- --ignored --test-threads=1
SELF_HOST_GIT_API_FIXTURE=1 SELF_HOST_GIT_BUILD_FIXTURE=1 cargo test --locked --test git_api_linux --test git_build_guard_linux --test source_build_guard_linux -- --ignored --test-threads=1
SELF_HOST_DISPOSABLE_DOCKER=1 cargo test --locked --test stateful_docker -- --ignored --test-threads=1
```

Run the ignored tests only as root on a disposable Linux Host. The privileged
daemon setup provisions dedicated accounts; Application commands still run
without root.

## Certificate evidence

Eight certificate tests use a local HTTPS ACME service that validates request
signatures, nonces, account identifiers, HTTP-01 responses and signed CSRs.
They issue separate certificates for independent names and an alias through
the real challenge listeners. They verify actual SNI handshakes, live
renewal, account and key persistence, private file modes, per-name failures,
hourly backoff and expired-certificate refusal. Existing proxy tests preserve
route and challenge precedence. The certificate metadata API requires
authentication and excludes keys and account credentials.

The real daemon ran without public configuration. Its local-CA HTTPS proxy
served both the Git and native Web Targets, with the client verifying the
fixture CA. The native Web Target allocated no Docker Host port. These checks
also passed after the VM reboot.

## Native evidence

The three new API tests use an HTTP listener, file-backed Platform State and
the real N1 launcher and N2 s6 supervision. They cover lifecycle and task
outcomes, immutable execution identity, Variable updates, status and logs,
stopped intent, recovery, a private loopback Web Target, an unpublished worker
and refusal of a listener bound to all addresses. Account removal retains
data in an inaccessible root-owned home. The ten N1 and one N2 tests passed
again on the integrated code.

The reboot check used fresh Applications and a new manifest with
`examples/native_api_reboot.rs`. Preparation required one start per fixture.
The check then required a changed boot id, exactly two starts for the running
Application and one for the stopped Application. Both passed with non-root
uids, zero capabilities and `NoNewPrivs=1`. The retained-data probe was written
after the first launch, so regenerating an empty home could not pass it.
The installed native systemd unit restored the running Application before
the Operator API restarted. No stale counters were reused or assertions
weakened. See [native validation](wave-3-native-validation.md).

## Git evidence

The real Docker API fixtures verify pinned revisions, explicit refresh,
immutable image ids and task audit outcomes. A failed candidate preserves
the full active record, container and image. Rebuilding a stopped Application
retains stopped intent, and the next start uses its new recorded image.
Git Compose materializes contained build and environment files and preserves
P1 storage, private networking and literal dollar signs.

Credential checks cover private file modes, metadata-only responses,
credential redaction and checkout path containment. BuildKit checks cover
shell and JSON commands, inherited and explicit `SHELL`, private secret mounts,
withheld secret output, original ignore rules and checkout cleanup. A trusted
static launcher enforces non-root execution, zero capabilities and
`no-new-privileges`. Its separate read-only build-context mount remains absent
from the final image. Setuid, file-capability and ambient-capability checks
passed. Git Application containers also drop all capabilities and set
`no-new-privileges`.

The two existing P1 fixtures passed with fresh resources after integration.
They verify retained mapped storage, private consumers, native loopback access
and refusal of a missing external volume without creating empty storage.

## Real console acceptance

The console was served by the integrated `cargo run --locked -- serve --no-dns`
daemon on the disposable Linux VM. Native Chrome actions exercised the
actual form and task worker, without a mocked backend:

- Creating `git-console-created-wave3` from a synthetic Git repository and
  the `console-v2-fixture` tag reached `running` and `HTTP responding`.
  Summary displayed the full deployed commit and immutable image id.
- Saving `git-console-wave3` after its branch advanced, without selecting
  refresh, kept its deployed v1 commit and HTTP body.
- Selecting **Build the latest commit from this ref** deployed v2. Summary
  updated automatically when the task completed.
- A subsequent root-user candidate produced an inline build error. The
  failed task retained the exact Application response, container id, image
  id and v2 HTTPS body. The container kept its non-root user, dropped
  capabilities and `no-new-privileges` setting.

After reboot, Lima's automatic port forwarding held a stale tunnel. An
explicit local SSH tunnel restored access without another Application start.
The current console URL is **http://localhost:23722/console/**. The test tab
is authenticated and shows the created Git Application running, with its
completed build and v2 commit. No production access is involved.

To repeat a manual check while this disposable fixture is running:

1. Open the console and select **git-console-created-wave3**. In **Summary**,
   expect `completed`, a full commit and an image beginning with `sha256:`.
2. Open **Configuration** and select **Save and build** with refresh cleared.
   Expect the task to complete and the deployed commit to stay the same.
3. Set **Branch or tag** to `blocked-root-fixture` and select **Save and build**.
   Expect an inline build error while the Application remains running with
   the same Summary commit and image. Select **Discard** to restore the form.

For another creation check, select **Deploy application**, choose **Git
repository**, use `http://127.0.0.1:29091/app.git`, tag `console-v2-fixture`
and Web port `8080`, then select **Build and deploy**. Use a fresh Application
name. The loopback repository is accessible from this test VM only.

## Evidence and remaining blockers

Local raw logs and the snapshot manifest are preserved under
`/tmp/self-host-wave3-evidence/`. Main files are `macos-tests.log`,
`linux-tests.log`, `macos-clippy-final.log`, `linux-clippy.log`,
`console-tests.log`, `self-host-wave3-native-final-integrated-linux.log`,
`native-reboot.log`, `linux-git-builds.log`, `linux-stateful-docker.log`,
`console-failure-preservation.log` and `linux-snapshot-match.log`.
Private remote preflight evidence is excluded from the repository.

Public ACME staging was not attempted. The Operator confirmed that the
available remote Host runs production and kept staging blocked. A controlled,
isolated Host with public HTTP-01 access is still required. The local protocol
and TLS tests cannot prove public issuance.

A supplemental adversarial agent review did not run because its automated
review flagged cybersecurity risk. The implementation used a separate
read-only launcher mount, and the explicit BuildKit privilege and mount
checks passed. No claim of a completed adversarial review is made.

No implementation or real-backend blocker remains for N3 or D2. The wave
stops here, with public staging recorded as unvalidated.

## Final PR checks on 2026-10-02

The final snapshot includes the later approved GitHub App and GitLab OAuth
workflow, Settings connections, repository import and reviewed updates.
The Operator completed real GitHub consent and repository import. Its attempted
build was refused by the documented non-root Dockerfile policy. A successful
build of that repository and real GitLab consent/renewal remain unvalidated.

| Check | Result |
| --- | --- |
| macOS `cargo test --locked --all-targets` | 494 passed |
| Linux `cargo test --locked --all-targets -- --test-threads=1` | 491 passed, 21 ignored |
| Fresh privileged native API, N1 and N2 fixtures | 14 passed |
| Fresh Git API, BuildKit and static launcher fixtures | 5 passed |
| Fresh P1 storage and network regression fixtures | 2 passed |
| Clippy with all targets and warnings denied, macOS and Linux | Passed |
| Console clean install, build and TypeScript | Passed |
| Console tests | 50 passed |
| Release configuration tests | 7 passed |
| `cargo fmt --check` and staged diff whitespace check | Passed |
| Standards and Spec code review | No material findings |

All 21 ignored Linux tests were exercised separately with fresh privileged
fixtures. The first parallel ordinary run hit an existing DNS fixture collision
between its UDP and TCP ephemeral port binds. The complete sequential rerun
passed. No source change was needed; both run logs are retained.

The final Linux source and console bundle matched the integration worktree.
The existing nine Application records, one saved Git connection and provider
integration metadata remained unchanged. The validation did not restart the
console daemon or reboot the Host. Fresh reboot evidence remains the earlier
wave check, not a claim of another reboot at this checkpoint.

The corrected creation form passed real-backend checks at 1280px and 900px.
Its scroll area and action bar stay inside the card. Further interface redesign
is deferred. Public ACME staging remains blocked and production is untouched.

Final logs use the `pr-final-` prefix in the same local evidence directory.
`pr-final-linux-summary.json` records the Linux counts and preservation checks;
`pr-final-review.md` records the two review axes. Layout evidence is in
`new-app-layout-live-checks.json`. A minor changeset records the user-visible
wave features; the version release remains a separate pull request.

The first PR CI run passed its tests but Clippy 1.99 reported 76 redundant
`must_use` annotations generated by `async_trait`. The same failure was reproduced
on Linux with Rust 1.99. Only the five affected trait declarations allow
`clippy::double_must_use`, each with an explicit reason. The exception follows
the [upstream macro false-positive report](https://github.com/rust-lang/rust-clippy/issues/17529).
No dependency, API or runtime behavior changed. All-target Clippy with warnings
denied then passed on Rust 1.99 and the existing macOS toolchain. Red and green
logs are `pr-164-rust-1.99-clippy-red.log` and
`pr-164-rust-1.99-clippy-green.log` in the local evidence directory.
