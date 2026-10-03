# self-host

## 0.5.0

### Minor Changes

- c3e788f: Persist and renew public ACME certificates while preserving local CA mode.
  Connect native Application lifecycle actions to the existing non-root launcher
  and s6 supervision, including stopped intent and retained data after reboot.
  
  Build Applications from reviewed Git commits with contained checkout paths and
  private build credentials. Select repositories through saved GitHub App or
  GitLab OAuth connections, review updates before deployment, and rebuild the
  current commit. Settings manages saved Git access.
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
