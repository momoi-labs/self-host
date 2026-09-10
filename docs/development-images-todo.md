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
- [ ] Define explicit build checks for installed tools and required native
  modules. Report failures in the build log before marking an image ready.
  The `node-pty` failure showed that a successful install is insufficient.
  Keep `allow_builds` explicit and avoid silently adding tool dependencies.
- [ ] Validate checks under the runtime user and environment. Cover executable
  discovery, native module loading and writable persistent directories.
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
- [ ] Fix QR code rendering in the console logs. The current font appears to
  distort the block characters in T3's pairing QR code. Check the monospace
  font, glyph fallback, line height and character spacing, then verify that a
  phone can scan the rendered code.
- [x] Complete an authenticated Claude exchange through T3. User validation
  confirmed a response with the runtime running as `dev`.
- [ ] Complete an authenticated Codex task through T3.
- [ ] Investigate the missing `ps` executable reported by T3's terminal process
  checks. Determine the required image package and validate it as `dev`.
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
