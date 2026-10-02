# Git provider authentication

Provider buttons use GitHub App installation access and GitLab OAuth. Manual
tokens remain an advanced option. The one-time setup belongs to this self-host
installation, so its owner controls the provider registrations and their secrets.

## Provider contracts checked

Reviewed official documentation on 2026-10-01.

| Contract | Source | Implementation choice |
| --- | --- | --- |
| App registration from a manifest returns a temporary code for conversion | [GitHub manifest flow](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest) | The browser submits a manifest. The Host exchanges the returned code and keeps App secrets private. |
| Installation setup and automatic user authorization are separate registration options | [GitHub App registration](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/registering-a-github-app) | Use the setup callback to identify the installation, then start user authorization explicitly. |
| A user access token can list the installations accessible to that user | [GitHub installation API](https://docs.github.com/en/rest/apps/installations#list-app-installations-accessible-to-the-user-access-token) | Verify the selected installation and configured App before saving access. |
| User authorization supports PKCE | [GitHub user access tokens](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app) | Start user authorization with a new state and server-side PKCE verifier after installation setup. |
| Installation access tokens can authenticate Git HTTPS with Contents permission | [GitHub installation authentication](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation) | Request Contents read and Metadata read. List installation repositories and issue short-lived checkout access. |
| OAuth applications register redirect URLs and scopes | [GitLab application registration](https://docs.gitlab.com/integration/oauth_provider/) | Register the exact bridge URL and request `read_user`, `read_api`, `read_repository`. |
| Authorization code supports PKCE and token renewal returns replacement tokens | [GitLab OAuth API](https://docs.gitlab.com/api/oauth2/) | Keep the PKCE verifier server-side and replace access/refresh tokens together. |

The two-step GitHub installation flow is our design choice. GitHub's setup callback carries
an installation id; account authorization supplies the user proof needed to
verify that id. Neither callback alone creates a connection.

An already installed App can start user authorization directly. The optional
`use_existing_installation` start field enables this path. After user proof,
the Host selects the sole installation for the configured App, or the saved
installation when reconnecting. It rejects missing or ambiguous access and
does not accept an installation id from that callback as selection proof.

## Browser and API boundary

The console opens a popup from the user's click. Provider redirects reach the
public bridge, which sends callback data only to the origin recorded with the
pending authorization. The parent checks origin, popup identity and state.
Only its authenticated completion request can exchange a code or save access.
GitHub may return another authorization URL and state for the same popup.

The bridge has no account-changing behavior and no arbitrary redirect target.
Its HTML escapes script data and uses a random CSP nonce, `no-store` and
`no-referrer`. Invalid, missing or malformed callback states receive a fixed
error page that does not reflect query values. Installation ids are JSON numbers.

Protected setup, start, complete and cancel requests use the existing API-key
middleware. Audit records the operation and a bounded failure message. It
excludes states, codes, registration secrets and provider response bodies.

## Access and renewal boundary

Connection metadata exposes the name, provider, credential id, status and
authentication method. App keys, client secrets, OAuth tokens and installation
tokens stay in private storage with existing source credential permissions.
GitHub/GitLab endpoints are fixed. A provider token cannot authenticate Git on
another host, including after connection metadata is removed.

Listing repositories and fetching a checkout both resolve renewable access.
App and OAuth reconnection keeps Application references and verifies the same
account or installation and authentication method. Legacy token reconnection
preserves its provider check and username behavior. The Host refuses integration
changes while provider connections still use it. Disconnect removes local access
and preserves Application records; it does not delete an external provider
registration or GitHub installation.

## Validation status

Automated checks passed on 2026-10-01:

- macOS: 489 Rust tests, including six authorization API tests. Clippy passed
  for all targets with warnings denied.
- Console: production build, TypeScript check and 50 tests. Eleven popup tests
  cover origin/source/state checks, continuation, denial, closure and timeout.
- Linux: 13 connection tests and the six authorization API tests passed with
  fresh local provider fixtures. Renewal tests cover concurrent requests,
  rotated refresh tokens, revoked access and stale reconnection responses.

Passing logs are in `/tmp/self-host-wave3-evidence/` on the development machine:
`git-provider-auth-macos-tests.log`, `git-provider-auth-linux-tests.log` and
`git-provider-auth-clippy.log`. The fixture runs the updated code through
`cargo run -- serve --no-dns` in the `wave3-linux` VM. Its console is available
at `http://localhost:23722/console/` through the local tunnel.

Manual checks in Safari against that console confirmed:

- Settings lists Git connections and opens GitHub App or GitLab OAuth setup.
  The GitLab callback uses the current console origin; its secret field is masked.
- GitHub registration opens the official provider login in a popup. Closing it
  shows a retry message and does not save a connection.
- New Application keeps the repository URL and connection name when nested
  setup is cancelled. Manual tokens stay under Advanced.
- The list and popup render in light and dark themes. The header fits at 1280px.
  Below 1024px the sidebar stacks above the page and both primary actions remain
  reachable. Window size and theme were restored after the checks.

The served console bundle matches the local build. All eight Application
records match their pre-check snapshot. The existing Git Application still runs
as uid/gid 10001, with all capabilities dropped and no-new-privileges enabled.
`git-provider-auth-live-checks.json` records the API and bundle comparison.

The first real GitHub registration attempt rejected a loopback webhook URL,
even with delivery inactive. The manifest now omits `hook_attributes` and keeps
the browser callback URLs. GitHub's [manifest parameters](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest)
make that object optional. This flow does not receive webhook events.

The regression failed before the fix. All 14 connection tests then passed on
macOS and Linux, and Clippy passed. The live registration API returned no
webhook configuration and preserved its local callback. Chrome opened the real
GitHub registration page with the corrected manifest and displayed the App
name field and creation button without the webhook errors. The check stopped
before creating the App. Logs are `github-manifest-fix-macos-tests.log`,
`github-manifest-fix-linux-tests.log` and `github-manifest-fix-clippy.log` in
the same evidence directory.

On 2026-10-02, the Operator completed real GitHub App registration. The Host
saved the integration with its loopback callback. No connection was saved.
The callback's instruction to close the window was misleading; it now asks
the Operator to keep it open until self-host closes it automatically.

Chrome also confirmed a retry failure for an already installed App. GitHub's
"Configure" link opens its installation settings without the pending state,
so the installation callback cannot complete that retry. The console now
offers direct authorization through **App already installed? Use existing
access**. GitHub reconnection uses this path automatically. The Git connections
table now appears inside the Settings page, with its section link preserved.

Four regression fixtures failed before the existing-installation change and
passed after it. All 18 connection and authorization tests passed on macOS
and Linux, alongside the six authorization API tests. Clippy passed with
warnings denied. The console build and its 50 tests passed.

The updated Linux fixture serves the current JavaScript and CSS. Live API
checks verified direct OAuth startup, PKCE, the loopback callback, the new
bridge message and session cancellation without a provider exchange. Chrome
confirmed the Settings card and visible header after following its section
link. **Use existing access** opened the real GitHub authorization screen with
the expected loopback return address. The agent left consent to the Operator.
All eight Application records and the private provider configuration
were preserved. `git-settings-live-checks.json` records these checks in the
same local evidence directory.

The Operator then completed GitHub authorization through the existing-installation
flow. Settings showed the saved connection. New application listed and imported
a repository without a URL. The attempted build was rejected because its
Dockerfile had a `RUN` without an explicit numeric non-root `USER` in that stage.
GitHub consent, repository listing and source inspection passed. A successful
build of that repository has not been validated.

Real GitLab registration, consent and token
renewal have not been validated against a controlled account. Local provider
fixtures prove protocol and failure handling; they do not prove a real
provider consent screen or organization policy. No provider registration or
production environment was changed by the agent during this work.

For a real check, follow the [setup steps](../git-source-builds.md#private-inputs)
with a controlled account and repository. Confirm that connection success
selects the saved access, imports its repository without a URL, and preserves
the Application draft when consent is denied or the popup closes.
