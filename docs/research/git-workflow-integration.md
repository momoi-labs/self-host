# Git workflow integration validation

Workflow B now uses the real Application form and backend. Repository access
is optional, saved in Settings and selected before repository import. Public
URLs work without access. Creation reviews a commit before building it;
updates compare the current and next commit. Rebuild keeps the current commit.

## Automated checks

Checked on 2026-09-30 against the integrated workspace:

| Check | Result |
| --- | --- |
| Console build and TypeScript | Passed |
| Console tests | 39 passed |
| macOS `cargo test --locked --all-targets` | 476 passed |
| Clippy, all targets, warnings denied | Passed |
| `cargo fmt --check` and `git diff --check` | Passed |
| Linux Git API fixtures | 3 passed |

The Linux API tests created fresh Git repositories and Applications. They
checked reviewed commits across moved refs, stale previews, rebuild pinning,
failed build retention, tag selection, stopped intent, Compose containment
and execution with uid 10001, all capabilities dropped and no-new-privileges.

Six provider fixture tests checked GitHub/GitLab authentication, pagination,
private permissions, token-free responses, reconnect, disconnect and expired
access. They also checked provider redirects, host binding after metadata
removal and stale responses after reconnect. The fixtures inject a local API
only inside tests. The public API has fixed provider endpoints.

Logs are under `/tmp/self-host-wave3-evidence/`, including
`git-workflow-api-linux.log`, `git-workflow-macos-tests.log`,
`git-workflow-clippy.log` and `git-workflow-console-tests.log`.

## Real console

The console runs through `cargo run --locked -- serve --no-dns` in the
disposable `wave3-linux` Lima VM, with sanitized state. Its URL is
`http://localhost:23722/console/`, forwarded locally to port 23721 in the VM.
The test Git repository is `http://127.0.0.1:29091/app.git` inside that VM.
Production was not accessed or changed.

Safari checks against this backend:

- Git connection selection appears before repository selection or manual URL.
- Opening and cancelling Connect Git preserves the typed repository URL.
- Continue performs a real checkout and shows SHA, ref, build file and port.
- Changing the branch invalidates the candidate and requires another check.
- Build and deploy created `git-workflow-real` from commit `9b5cf275d644`.
- Update compared that commit with `94d37ce33082` and deployed the reviewed v2.
- Last update showed the actual task outcome and both revision ids.
- A root-user candidate `0257e54935e0` failed. The full Application response,
  exact container id and image id stayed unchanged. HTTPS still served v2.
- Rebuild kept v2 while the branch pointed at the rejected root-user commit.
- Settings opened the real Git connections list. Its empty state rendered in
  light and dark themes.
- Lifecycle controls stayed on one line at 1280 px. The shared header wrapped
  above them. Below 1024 px, the shared navigation stacked above the page.

The preservation and rebuild checks are recorded in
`git-workflow-ui-preservation.log` and `git-workflow-ui-rebuild.log`.
The fixture branch `workflow-main` was restored to the successful v2 commit.

## Manual review

1. Open the console and select `git-workflow-real`.
2. Open Summary. Expect commit `94d37ce33082` and its image id.
3. Select Update, then Check for updates. Expect the same current and next
   commit and the message that this version is already deployed.
4. Cancel, then select Rebuild current version in Summary. Expect Last update
   to report completion and Summary to keep the same commit.
5. Open Settings, then Git connections. Expect a list and Connect Git. In New
   application, choose Git repository and a connection to import its
   repositories without entering a URL.

## Remaining validation

At the 2026-09-30 checkpoint, private account listing and import were checked
with provider fixtures, and connections used saved access tokens. GitHub App
and GitLab OAuth were implemented on 2026-10-01. Their current evidence and
remaining consent checks are in [Git provider authentication](git-provider-auth.md).

Public ACME staging remains blocked because `cloud-server` hosts production.
No public challenge fixture or production route was changed.

Raw build output is withheld to preserve sanitized errors. Last update shows
the task and its safe failure report. A successful build still uses the
existing container replacement path, which can interrupt requests and does
not promise automatic runtime rollback.

The unslop validator `bin/validate-prose` is absent in this checkout. Changed
prose was reviewed manually.
