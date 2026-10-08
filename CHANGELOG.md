# self-host

## 0.7.0

### Minor Changes

- 28b8771: Manage routes from the console. A Routes page lists every hostname and path the proxy answers, and each Application has a Routes tab. Adding, changing or removing a route applies at once through `PUT /apps/id/{id}/routes`, without a rebuild, pull or restart. A path rule without a `target` follows its Application's Web Target.
- 09c52ff: Rebuild the console on Kiso React 0.14. Applications and databases open on a Summary that draws what reaches them and what they use, with panes the Operator can rearrange, variable names with values on request, and the database volume size. The Overview, Settings, login and the new application picker use Kiso's components, and every page keeps one spacing between blocks.
- 6f234b2: Name failing Applications and databases in the console header. `/health` answers `degraded` with the failing workloads, still with HTTP 200, and the header opens the first one.
- bccbab9: Make operator login easier to use with a responsive help panel, revealable API key, pending feedback and separate key, Host and connection errors. Add `self-host init --show-key` to print the existing initial key without repeating bootstrap.

### Patch Changes

- a4a9d20: Say why a managed database failed when Docker could not prepare its PostgreSQL image, instead of leaving the failed stage without a reason.
- 18e2257: Use Kiso's default visual style for console headings and spacing.
- ab7433d: Deleting a native Application on macOS now completes. macOS only deletes an
  account when the responsible process has Full Disk Access, which the Platform
  daemon cannot hold durably, so the Platform retires the account instead: the
  service is removed, nothing can run as the account, its number is never
  reused, and the retained data stays protected. `uninstall.sh` still deletes
  live and retired accounts from a terminal that macOS can ask (#168).
- 18e2257: Keep native command text visible with pixel corners and apply the selected border style to the whole editor, including its line numbers.
- a897b57: Report HTTP readiness for native Applications. The probe looked for a container service name, so a native Application always read "HTTP unknown".

## 0.6.0

### Minor Changes

- c9f9ea8: Dependencies accept any mise tool option, such as `extras=serve,ane` on a pypi
  tool. Press `+` on a dependency chip to add one as `name=value`. A list takes
  its items comma separated, and mise receives one value as a string and several
  as an array. This works for custom images, environments and native
  Applications. npm `allow_builds` keeps its own field.

## 0.5.0

### Minor Changes

- c3e788f: Persist and renew public ACME certificates while preserving local CA mode.
  Connect native Application lifecycle actions to the existing non-root launcher
  and s6 supervision, including stopped intent and retained data after reboot.
  
  Build Applications from reviewed Git commits with contained checkout paths and
  private build credentials. Select repositories through saved GitHub App or
  GitLab OAuth connections, review updates before deployment, and rebuild the
  current commit. Settings manages saved Git access.
- ab56ffe: Run native Applications on a macOS Host. Each Application runs under its own
  non-administrator account, supervised by s6. A protected helper, reached
  through a restricted sudo rule, creates the accounts and controls the
  supervisor, so the Platform keeps running as the Operator. The macOS release
  bundles s6, and the installer sets up the helper and a boot LaunchDaemon.
  
  On macOS the console hides resource limits, metrics, the terminal and managed
  PostgreSQL connections for native Applications, and the API rejects them.
  
  Deleting a native Application on macOS stops it and protects its data, but
  cannot remove its account yet: macOS requires Full Disk Access for that (#168).
- a974ef2: Add Linux s6 supervision for native processes using the existing dedicated
  non-root launcher. Supervision supports readiness checks, crash recovery,
  descendant cleanup, rotating logs and persisted running or stopped intent.
  The installer can opt into one boot supervisor with cgroup delegation.
  Native Application API requests remain disabled until lifecycle integration.
- 8d46bfd: Applications can save hostname/path routes to loopback targets through the API.
  The proxy chooses the longest matching path and applies changes without
  restarting Applications. Prefix handling is explicit; management and
  certificate-challenge paths remain reserved.
  
  Compose Applications can map existing named volumes explicitly and preserve
  file, directory and read-only bind mounts. New Applications use private
  networks, with explicit consumer grants for unpublished database providers.
  Existing Applications retain their shared network policy. Unpublished services
  report container health without requiring an HTTP target.
  
  This adds storage and connectivity support. Managed PostgreSQL provisioning
  and dedicated database screens remain pending.
- a084efa: An Application's record now says how it runs and whether it is published. A
  Compose Application can be created unpublished: no Hostname, no DNS Record, no
  proxy route and no Host port, which is what a worker or a database wants.
  Requests and responses gain optional `runtime`, `publication` and
  `variable_delivery` fields; existing Applications and older clients keep
  working unchanged. A native runtime is recognized but refused with 501 until a
  later release runs it.
  
  The Platform resolves Compose files itself: `${VAR}` and the other Compose
  forms read the Application's Variables and never the daemon's environment, and
  a Variable reaches a service only where the file names it. Applications
  created before this keep the old behavior, every Variable on every service,
  until switched with `variable_delivery`. YAML merge keys and `x-` extension
  fields are accepted; unsupported volume options and mount modes are refused by
  name.
- 4cd8eea: Manage native Applications from the console with private mise environments,
  non-root setup commands, logs, terminals and resource limits.
  
  Create PostgreSQL 16, 17 or 18 with persistent storage, private Application
  connections and guarded logical imports. Add deployment history, health checks,
  optional per-Application Git triggers and recovery from recorded images.
  
  Preserve display names while generating safe technical identifiers. Update the
  console to Kiso React 0.11.0 and Kiso 0.15.0, with numbered configuration steps,
  shared command editors and the selected theme.

### Patch Changes

- d1f59d5: Include s6 and its supervision helpers in Linux releases. The installer and
  Linux packages install a private bundle with its license notices, so native
  Applications no longer require a separate distribution s6 package. Boot
  supervision remains opt-in through `SELF_HOST_NATIVE=1`.
- 4266137: The console reads Platform state through TanStack Query. Screens that show
  the same data share one cached copy, so a screen opened again shows the last
  answer at once while it refreshes. Polling pauses while the console tab is
  hidden and catches up when the tab is shown again. A failed read keeps the
  last answer on screen beside the error, where some lists used to go empty.
- 3ede7b0: Let a restart or a redeploy pull newer images, so an Application on a moving
  tag such as `:latest` can pick up a new release from the console. Restart opens
  a confirmation with a "Pull newer images" checkbox, and Save and redeploy has
  the same checkbox beside it. Both start from a new Platform setting and can be
  changed each time. The event records the old and new image id of each pull,
  and a failed pull before a restart leaves the containers running as they were.
  Settings is now one page of cards that fills the screen instead of tabs.

## 0.4.1

### Patch Changes

- 4845b50: Show the installed version on the login screen and operator console, and expose it through `self-host --version`.
- d7cff57: Name a Virtual machine's guest after the machine, so its shell shows `net-test` instead of `lima-sf-dev-env-...`. Existing machines take the name on their next update.
- c4880a8: Upgrade the console to Kiso React 0.9.0 and use its shared status badges,
  lifecycle controls, and screen layouts. This brings the upstream keyboard,
  focus, touch-target, and select-menu fixes into the console while preserving
  terminal and virtual machine detail layouts. Keep dependency searches intact
  on Escape and event tables within narrow screens.
- 2ee53ac: Fit the terminal to its pane again, so the last lines of a Virtual machine or container shell are no longer cut off and the scrollbar stays inside the terminal.

## 0.4.0

### Minor Changes

- 26283d1: Install self-host through Homebrew on macOS or download Linux packages for
  pacman, apt, and dnf. Releases update the Homebrew Cask and include Arch, Debian,
  and RPM packages. AUR publication is disabled until maintainer access is ready.
  
  Keep Platform State in SQLite and run console actions as queued Tasks with
  status and event history. The console shows paginated events, Virtual machine
  runs, and unsaved changes. Narrow screens stack the sidebar above the content.
