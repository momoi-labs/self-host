# Development images TODO

Track the remaining prototype work here. The first wave has been tested in the
console, including an authenticated Claude response through T3.

## First change

- [x] Set the development container's internal hostname to the Application
  name. For an Application named `t3`, running `hostname` must return `t3`,
  including after a redeploy or rename. This is separate from the Application
  Hostname used for LAN DNS, such as `t3.prototype.lan`, and from Docker's
  container name.

## Runtime and persistence

- [ ] Fix Bash job-control warnings during T3 environment capture. The installed
  T3 `bin.mjs` calls `execFile(shell, ["-ilc", ...])` without a PTY. Review
  interactive shell requirements in T3 while preserving environment loading.
  The Application start command itself is `t3 serve`, not interactive Bash.

Bring the runtime setup from [PR #92](https://github.com/momoi-labs/self-host/pull/92)
into the generated images. Keep the prototype's configurable mise dependencies.

- [x] Run T3 and agent processes as the `dev` user. Use root only to prepare
  volume permissions before starting the configured command. The current
  Claude failure reported that `--dangerously-skip-permissions` could not run
  as root. The migrated server now runs as UID 1000.
- [x] Declare `/data/home` as the user's home in `/etc/passwd`. Keep agent
  credentials, shell configuration and history inside the persistent volume.
  Keep T3 state in `/data/t3home` and repositories in `/data/repos`.
- [ ] Restore the image's PATH and mise configuration in login shells as well
  as directly launched processes. Verify installed tools from the T3 terminal
  and from an agent session. Login-shell checks passed for Node, T3, Rust,
  Codex and Claude; the interactive workflow still needs user validation.
- [x] Make the image's mise configuration trusted and its tools usable by
  `dev`. Fix ownership of writable directories on startup, including when an
  existing volume is attached.
- [x] Migrate existing root-owned state when changing to `dev`. Preserve the
  current `/data` contents and existing agent credentials under `/root`, with
  a backup before migration. Credentials must stay out of image layers.
- [ ] Handle saved T3 projects and sessions that still reference `/root` after
  migration. User validation found `EACCES` on `/root/.t3code/vcs.json`.
  Relocate project data and update its saved path explicitly, preserving files.
  New projects should use `/data/repos`. Do not grant `dev` access to `/root`
  as a workaround.
- [ ] Confirm which first-boot settings from PR #92 belong in every development
  image. SSH key generation, Git signing and passwordless sudo need an explicit
  decision. The configured start command must remain an Application setting.

## Application editing and image updates

- [x] Preserve the Development image form when editing an Application. Keep
  the image selector, start command, web port and persistence control available
  after creation instead of converting the Application to a plain Compose editor.
- [x] Store the selected development image ID and deployed tag with the
  Application's runtime settings. Reopen the form without guessing these
  values from arbitrary Compose text.
- [x] Replace an image through Save and redeploy. Preserve the Application ID,
  command, port, volume and credentials. No manual tag editing or terminal
  commands should be needed.
- [x] Show when a saved image has a newer successful build. Keep the deployed
  tag until the Operator applies the update. A failed build must leave the
  running Application usable.
- [ ] Decide how advanced Compose editing works alongside the form. Preserve
  custom Compose settings and make any conversion explicit.
- [x] Verify deletion stays disabled while an Application uses any tag of a
  development image, including an older build or a stopped Application.

## Templates

- [ ] Add template selection to prefill development image and Application
  settings while keeping them editable.
- [ ] Provide a T3 Code template with mise dependencies, explicit build
  permissions and checks, start command, web port and persistence settings.

## Build checks and local testing

- [x] Speed up dependency search. Cache the complete, paginated catalog for the
  server's lifetime and filter it in the browser. The live check loaded 1,091
  tools in four seconds, then answered a cached search in about one millisecond.
  Typing different terms makes no additional requests. Restarting the server
  refreshes the catalog on its next request.
- [x] Keep dependency entry usable when search fails. Previously `just` showed
  "Search unavailable." Both registry names and explicit backend keys can now
  be entered while the catalog loads or is unavailable. A failed catalog load
  can be retried by reopening the page; a successful load remains cached.
- [ ] Reproduce the reported "Please fill out this field" tooltip during
  dependency selection. Browser checks added `node`, `just`, `claude-code` and
  `npm:t3` with Enter and Tab, with both a working and failing catalog, without
  submitting or triggering validation. Only an explicit Save and build with an
  empty image name triggered the expected required-field validation.
- [x] Add explicit build checks for installed tools and required native
  modules. The optional Build checks field saves one command per line in
  `tasks.check.run`. A nonzero exit or timeout fails the Docker build before
  the image can become ready. No package checks or dependencies are inferred.
- [x] Run checks as `dev`, with the runtime PATH, no network and a 60-second
  limit per command. Check installed mise tools, `ps` and writable data
  directories first. Use temporary `/data` during the build so check output
  does not become personal state in the image.
- [x] Use the normal Platform DNS, HTTPS and console for local testing. The
  temporary preview runner has been retired. Applications use
  `https://<application>.prototype.lan`, and the console uses
  `https://admin.prototype.lan/console/`.
- [ ] Verify browser certificate trust and terminal paste after local setup.
  HTTP on a container IP does not provide the Clipboard API. HTTPS must have a
  trusted certificate; bypassing a certificate warning is not the setup.
- [ ] Distinguish a running container from a server ready to accept HTTP.
- [ ] Decide whether pairing-token creation should be available in the console.
  Document the current command until then. Image updates must preserve T3 state
  so they do not require pairing again unnecessarily.
- [x] Preserve whitespace in logs and use adjoining monospace rows for QR
  block characters. A browser screenshot decodes successfully with the fix;
  the same fixture does not decode with the previous CSS. SSE comments such
  as `: keepalive` no longer appear as application output.
- [ ] Confirm a phone can scan the pairing QR code in the console logs.
- [x] Complete an authenticated Claude exchange through T3. User validation
  confirmed a response with the runtime running as `dev`.
- [ ] Complete an authenticated Codex task through T3.
- [x] Include Debian's `procps` package for T3's terminal process checks.
  Every image build runs `ps -eo pid,ppid,args` as `dev`.
- [ ] Recreate the Application and verify projects, files, agent logins and T3
  state survive. Confirm the terminal can still find the installed tools.
- [ ] Finish the device checks from PR #92. Access the environment from a phone
  and confirm an agent task survives closing and reconnecting the client.

## Already verified in the prototype

- [x] Prevent scrolling over a focused web-port field from changing its value.
  Reproduced `3000` becoming `2997` before saving; port fields now lose focus
  on wheel events so the page can scroll without changing the port.
- [x] Save and edit image dependencies, build with mise, and inspect build logs.
- [x] Name image repositories with a stable ID and tag them with the MD5 of the
  generated `mise.toml`.
- [x] Configure a start command, web port and optional `/data` volume during
  Application creation. A file survived a real container recreation.
- [x] Build T3 with `allow_builds = ["node-pty"]`, Python and build tools. The
  rebuilt server returned HTTP 200 and displayed the pairing page.

These checks do not yet cover a complete agent workflow after an image update.

## First-wave validation

The local `t3` Application has explicit development metadata and runs with
hostname `t3` as UID 1000. Its existing data volume remains attached. The
migration copied its Claude and Codex settings into `/data/home` after a backup.
The console reopens its saved image, command, port and persistence settings.

The combined Rust suite passed. Browser checks covered keeping the deployed tag,
selecting a newer build and editing settings after a failed build. Docker checks
covered HTTP 200, login-shell tools and persistence across recreation using a
disposable volume. The local T3 server also returned HTTP 200 after migration.

User validation also confirmed the `dev` user, Application hostname, and an
authenticated Claude response through the normal HTTPS endpoint. Codex tasks,
SSH provisioning, Git signing, sudo, advanced Compose conversion and generic
build checks remain open decisions or follow-up work.

## Second-wave validation

The image now includes `procps` and checks its runtime before Docker tags the
build. A real Node and T3 image passed checks as `dev` without network access,
including opening a PTY. Removing `pty.node` in a disposable build made that
same check fail, and Docker did not publish its tag.

The console preserves QR whitespace and hides SSE comments. A QR decoder read
the browser screenshot after the CSS fix and failed on the previous rendering.
The two stream-parser tests cover split events, blank lines and notice levels.
The Rust suite passed 213 tests; the console build, clippy and all-targets check
also passed.

To validate in the UI:

1. Open a development image and add installed commands to Build checks, such
   as `node --version` and `t3 --help`. Save and build, then confirm the log
   contains `Build checks passed.` The checks appear in the mise.toml preview
   and remain available when reopening the image.
2. In a disposable recipe, use `false` as a check. The image must show Failed.
   Version or help checks alone do not prove native modules work; use a command
   that exercises the module when that behavior matters.
3. Select the rebuilt image in the Application and Save and redeploy. Confirm
   `ps -eo pid,ppid,args` works in its terminal.
4. Generate a fresh pairing QR code and scan it with a phone. Confirm spaces
   remain aligned and `: keepalive` does not appear as a log line.
