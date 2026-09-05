# Optional 1Password integration

Research recorded on September 5, 2026. This integration was considered and
removed from the MVP requirements. No vault was accessed, no item was moved and
no installation or boot test was performed. Account plan and item locations were
not verified.

This concerns supplying secrets to Applications. It does not define how people
authenticate to those Applications. See the approved [vision](../vision.md).

## Service account authentication

A 1Password service account lets `op` retrieve authorized secrets using a token,
without interactive desktop-app authentication. The documented plans include
Individual, Families and Teams as well as Business, with different limits.
[CLI authentication](https://www.1password.dev/service-accounts/use-with-1password-cli),
[plan limits](https://www.1password.dev/service-accounts/rate-limits).

Default Personal, Private, Employee and Shared vaults have access restrictions.
A dedicated eligible vault could hold the credentials needed by the homelab,
with read access granted to its service account.
[Service account setup](https://www.1password.dev/service-accounts/get-started).

The service account still needs its initial token before it can fetch other
secrets. Pre-login access to that token and startup without internet were not
validated. Service account authentication alone does not establish unattended
startup of the whole stack.
[Authentication concepts](https://www.1password.dev/sdks/concepts).

## Possible integration, outside the approved scope

Application configuration could store secret references. At startup, the Platform
would resolve them and deliver the required values to the Application. This is a
design option inferred from the documented tools, not an implemented integration.

- `op run` starts a process with secrets in its environment.
- `op inject` fills a configuration file with actual secret values; that file
  needs protection and removal when no longer needed.

Sources: [op run](https://www.1password.dev/cli/reference/commands/run) and
[op inject](https://www.1password.dev/cli/reference/commands/inject).
Delivery to the project's Docker runtime was not tested.

Desktop app integration uses interactive authentication. A service account is
the candidate if this integration later needs unattended startup.
[Desktop app integration](https://www.1password.dev/cli/app-integration).

## Token, permissions and limits

The token is shown at creation and must be protected. Keeping a recovery copy in
1Password would not remove the Host's need to obtain a token for its first
authentication; this follows from the documented authentication flow.
[Service account security](https://www.1password.dev/service-accounts/security).

Access is scoped by vault. Changing vault access or permissions requires creating
another service account, and creating one requires the appropriate account
permission.
[Creation and restrictions](https://www.1password.dev/service-accounts/get-started).

At research time, Individual and Families plans allowed 1,000 daily requests
shared across service accounts. One command can issue several requests. If this
integration is adopted, fetching at Application startup rather than on every
health check would reduce request volume.
[Rate limits](https://www.1password.dev/service-accounts/rate-limits).

## Connectivity and Connect

The service account path uses 1Password's remote service. This research does not
establish a local cache as an offline-startup guarantee. 1Password Connect offers
a cache in the Operator's infrastructure after initial retrieval, at the cost of
another component to operate. Evaluate it only if an actual requirement justifies
that cost.
[Secrets automation options](https://www.1password.dev/secrets-automation).

Keychain token storage before login and its interaction with FileVault remain
unverified. None of these questions adds a task to the current MVP.
