# Build an Application from Git

Choose **Git repository** on the New application form. Select a saved connection
and import a repository, or enter a public repository URL without a connection.
Select **Continue** to check the repository, review the detected recipe and
port, then select **Build and deploy**. The Platform builds the reviewed commit
and records its full id and immutable image id. Git builds use the container
Runtime. Native commands do not use this build path.

## Choose the inputs

- **Git repository** accepts HTTP or HTTPS, without credentials in the URL.
- **Branch or tag** selects the first commit. `HEAD` is the default.
- **Pinned commit** accepts an optional full commit id.
- **Build context** is a directory relative to the checkout root.
- **Dockerfile path** is relative to the checkout root, including when the
  context is a subdirectory.
- **Compose path** selects a Compose file instead of one Dockerfile. Its
  `build.context` is relative to the Compose file; `build.dockerfile` is
  relative to that context.
- **Web port** is the port inside the container. For Compose, the Web service
  can be explicit or use the first service that declares a port.

Choose a Dockerfile or Compose at creation. An existing Git Application keeps
that build type; its checkout paths and declared inputs can still change.

Absolute paths, `..`, checkout symlinks and arbitrary Host bind mounts are
refused. The Platform never runs repository hooks, submodules or scripts on
the Host. It disables Git hooks, global configuration and external transport
helpers while fetching. A checkout has a three-minute limit; Docker source
commands have a ten-minute limit.

## Keep or advance a commit

An ordinary **Save and build** rebuilds the deployed commit. Changing build
arguments, secrets, context or Dockerfile preserves that commit. Changing the
repository or ref selects that source's commit. An explicit pinned commit
always wins.

To move to another version, select **Update** on the Application detail screen.
Choose the branch or tag and select **Check for updates**. Review the current
and next commit, then select **Build and deploy update**. A branch moving after
the check cannot change the reviewed build. If another update changed the
deployed commit, check again before retrying. Updating a pinned source clears
its saved pin and follows the chosen branch or tag.

Select **Rebuild current version** in Summary to rebuild the deployed commit.
**Last update** follows the real task and records its result. The Summary tab
shows the deployed commit and each image id. The build must finish and its
Compose definition must pass validation before the Platform replaces the
active record or containers.
A checkout, build or candidate validation failure leaves the running
Application, route and deployed commit intact. Its Task records the failure.

This is a build gate. After a successful build, container replacement follows
the existing deployment behavior and can interrupt requests. It does not
promise an automatic rollback when the new Application fails to start.

## Dockerfile policy

Every stage that has a `RUN` needs an explicit numeric, nonzero `USER` before
that instruction. The final stage also needs one. An optional group id must
be numeric and nonzero. Names such as `app`, root, variable substitutions and
implicit inherited users are refused.

```dockerfile
FROM alpine:3.21
USER 1001:1001
WORKDIR /tmp
RUN printf 'built by a non-root account\n' > /tmp/build-result
CMD ["sleep", "infinity"]
```

`FROM` names a literal image, optionally with `AS` and a stage name. The
Platform pulls registry bases, refuses inherited `ONBUILD`, and pins each
base to its inspected registry digest before building. Multiline `FROM`,
custom frontend/parser directives, `ONBUILD`, insecure run entitlements and
heredocs are refused. Standard `COPY`, `ADD`, `WORKDIR`, `ENV`, `ARG`, `LABEL`,
`EXPOSE`, `ENTRYPOINT`, `CMD`, `HEALTHCHECK`, `VOLUME`, `STOPSIGNAL` and `SHELL`
remain available.

Build commands run inside Docker BuildKit. The Platform grants no insecure
entitlements and does not run them as a Host shell. Each `RUN` mounts a
trusted static launcher read-only from a separate build context and routes
shell or JSON commands through it. The launcher refuses root identities,
clears process and ambient capabilities, sets `no-new-privileges`, then
executes the command. Build descendants cannot regain root through setuid
programs or file capabilities. A `RUN` requires a Linux Host matching the
Docker architecture, `/usr/bin/cc` and static libc. If those are unavailable,
the build fails without a fallback; builds without `RUN` remain available.

Explicit `SHELL` and inherited base-image shells retain their argv. `RUN`
supports literal `--mount` flags, with absolute targets that cannot cover the
launcher. Other run flags and mount-variable expansion are refused.
`BUILDKIT_SYNTAX` cannot be supplied as an argument. The checked Dockerfile
uses Docker's default frontend. The Application context keeps its original
ignore rules. The launcher is absent from the final image.

Git Application containers
drop all capabilities and use `no-new-privileges` so setuid programs cannot
regain root. This policy relies on Docker's isolation; it does not establish
that arbitrary repository code is safe against container escapes.

## Compose inputs

Git Compose files support the ordinary Compose subset plus `build` and
`env_file`. Pasted Compose files still refuse those checkout-only inputs.

`build` accepts a context string or a mapping with `context`, `dockerfile`
and `args`. Arguments can be a mapping or explicit `NAME=value` list.
The source form's declared arguments override matching Compose arguments.
Built services can inherit their validated image user. Registry-only services
need an explicit numeric nonzero `user`, and the Platform pins their images
too. A Compose user override cannot select root.

`env_file` accepts one path or a list. Files contain `NAME=value` lines,
optional matching quotes and comments on separate lines. Later files
override earlier files; explicit service `environment` overrides them all.
File values are literal, including dollar signs. They never read the
daemon's environment. Long `env_file` syntax and shell expansion are refused.

The resulting Compose definition has no build or file references. It keeps
the existing storage mapping, private networking, Application Variables and
Publication behavior.

## Private inputs

Select **Connect Git**, choose GitHub or GitLab, and authorize access in the
provider popup. The new connection is selected without clearing the Application draft.
The **Git connections** panel in **Settings** lists saved access and allows reconnection or
disconnection. Reconnecting keeps the credential id used by existing
Applications. App and OAuth reconnection requires the same authorized account
or installation and authentication method. Manual token reconnection keeps
the existing provider check and allows the token's username to change.
Disconnecting removes local access, so the next private fetch needs a replacement
connection. It does not uninstall a GitHub App or delete a provider registration.
Public URLs do not require a connection.

The Platform uses GitHub App installation access for GitHub.com and OAuth for
GitLab.com. It verifies the account and installation before saving access.
Repository lists come from the provider API. The Host renews short-lived access
before listing repositories or fetching a checkout. Provider credentials can
only fetch repositories on that provider's HTTPS host. It refuses redirects and
never returns tokens in connection metadata, Application responses or task payloads.

### Set up GitHub once

1. Open **Settings > Git connections**, then **Provider setup**.
2. Enter the console URL you use in the browser, ending in `/console/`.
   Use HTTPS, or HTTP on a loopback address for local development.
3. Select GitHub. Leave the organization empty for a personal registration,
   or enter an organization where you can register an App.
4. Select **Create GitHub App**. Choose a unique App name in GitHub and finish
   registration. Keep the popup open until self-host closes it automatically.
5. Return to self-host and select **Connect Git**. Install the App on the
   intended account, select its repositories, then authorize the account in
   the same popup.

If the App is already installed, select **App already installed? Use existing
access** in the connection dialog. The Host verifies the authorized user's
installation before saving the connection. If several installations are
available, it refuses to choose one arbitrarily. Reconnecting uses the
connection's existing installation.

The manifest requests Contents read and Metadata read. Webhooks are disabled.
An installation callback starts a separate user authorization step so the
Platform can verify that the selected installation belongs to that user and
the configured App. Registration follows the [GitHub manifest flow](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest).
You are done when the saved connection appears and its repository list loads.

### Set up GitLab once

1. Open **Settings > Git connections**, then **Provider setup**, and select GitLab.
2. Enter the current console URL, ending in `/console/`. Keep this browser
   origin when connecting accounts.
3. Create a confidential OAuth application on GitLab.com. Register the exact
   callback URL shown by self-host, ending in `/source/authorization/callback`.
4. Enable `read_user`, `read_api` and `read_repository`. Copy the application
   ID and client secret into the setup dialog and save.
5. Select **Connect Git**, choose GitLab, and authorize the requested read access.

GitLab's [application registration guide](https://docs.gitlab.com/integration/oauth_provider/)
describes its registration screen and scopes. The Host uses authorization code
with PKCE and stores refresh tokens privately. You are done when the saved
connection appears and its repository list loads.

### Use a token manually

Open **Advanced** in the connection popup to save or reconnect a personal
access token. Existing token connections keep this method and their credential
ids. GitHub tokens need repository metadata and contents read access. GitLab
tokens need `read_api` and `read_repository`.

Provider setup does not replace an integration used by active App or OAuth
connections. Disconnect those connections before registering a replacement.
Their Applications remain intact and need new access for the next private fetch.

### Keep build inputs private

The authenticated credential API supports Git credentials, registry
credentials and build secrets:

```json
POST /source/credentials
{"kind":"build-secret","value":"synthetic-build-input"}
```

The response contains only `id` and `kind`. `GET /source/credentials` lists
that metadata. Registry credentials also require `username` and `server`,
the registry host. They use an isolated Docker configuration for this build;
the Platform does not change the daemon's global credentials.

Ordinary build arguments are visible Application configuration. Put secret
values in a `build-secret` credential and enter `mount-name=credential-id` in
**Build secrets**. Docker receives a BuildKit secret file, separate from
`--build-arg`. The Dockerfile reads that mount with
`RUN --mount=type=secret,id=mount-name,uid=1001,gid=1001 ...` after
`USER 1001:1001`. Match the mount owner to the declared Application user.

Credential directories have mode `0700`; credential files have mode `0600`.
The temporary checkout is private and removed after the attempt. Credentials
remain outside its build context. The Platform withholds repository and build
output from API errors and audit events because a build can print arbitrary
secret values. A failure reports its stage without exposing raw command output.
