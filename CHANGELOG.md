# self-host

## 0.13.1

### Patch Changes

- af84dbc: Application containers resolve names under the DNS Suffix, such as another Application's Hostname. They used Docker's resolver, which on a Mac is Colima's VM and does not know the Suffix, so a call to `https://comfyui.momoi.internal` failed with `ENOTFOUND`. When the Platform serves DNS, containers now use it, and it forwards every other name upstream. Existing Applications pick this up the next time their containers are recreated.

## 0.13.0

### Minor Changes

- d22f90d: A database's Imports tab lists every import into it, newest first, with its Application, file, size, result and imported object count, and shows the steps and pg_restore log of the one you select. The import runs in the daemon, so the list shows how it ended even if the console was closed. `GET /databases/{id}/imports` returns the same list. A database keeps its last 50 imports. An import no longer needs an empty target database: what clashes with existing data fails the restore and shows pg_restore's error. The import also stops checking the archive's PostgreSQL version and stops timing the restore, waits on its database's queue instead of holding the consumer Application back, shows pg_restore's output unfiltered except for the Platform's passwords, and fails with pg_restore's last error line. New import takes any pg_dump format (custom, tar, directory or plain SQL, read from the file and preselected, or chosen when it cannot be told), either uploaded with no size limit or read from a path on the Host. New import opens in a dialog with numbered steps that turn into checks as they are filled, and an upload runs in the background with its progress in the list and a Retry when it fails.

### Patch Changes

- 7c59ebb: A database import no longer waits for the consumer Application to be stopped, so an Application that failed its first deploy can take its data. The restore now shows pg_restore's log, error included, under Last operation, and the Import tab gives the `pg_dump` command that makes a compatible archive. Stopping a Compose Application whose definition needs a Variable that is not set, such as a Git Application whose first deploy failed for it, stops it instead of failing with "The database runtime operation failed". Every database stage in Last operation now lists the commands it ran with their output and exit status, passwords cut out, instead of "Check PostgreSQL and Docker logs", and an archive from a newer PostgreSQL names both versions.

## 0.12.0

### Minor Changes

- 946d5b3: Git builds are a plain `docker build` by default, so a Dockerfile can install packages as root before it switches user, and Git Applications with a `RUN` now build on a Mac. The image still has to run as a numeric nonzero `USER`. Turn on **Strict build** in an Application's Git source for a repository you do not trust: every `RUN` then goes through the build guard as before, which refuses root and needs a Linux Host.

## 0.11.0

### Minor Changes

- d52b7f5: The console works on a phone. Below 700px the navigation moves to a bar at the bottom, with Applications, virtual machines and Settings behind More, and from 700px to 1023px it opens from a menu button. The top bar shows the Platform version and the health dot at every width. Detail screens show their tabs as sections you open one at a time, the status badges wrap, and Start, Stop, Restart and Remove sit behind one "More actions" button. The Summary diagram fits the screen instead of running off it. The console moves to kiso-react 0.16.1 and kiso 0.20.4.

### Patch Changes

- bea3acd: The console has a Host page. It opens a login shell on the Host, as the account the Platform runs under, and on a Mac it opens the screen through Screen Sharing in a window of its own, for the dialogs that only take a click. Both are off until the Operator turns them on in Settings > Host, and the page leaves the menu while both are off. On a Linux Host that runs the Platform as root, the shell is a root shell.
- 1f424a0: A long dependency chip, such as a mise tool with a `url=` option, no longer shows a scrollbar that squeezes it under classic scrollbars. The console moves to kiso 0.19.1.
- 7cc17be: Upgrading a macOS Host with `install.sh` now reminds you that native Applications reading an external drive need Full Disk Access granted to the helper again, since an upgrade drops it. The steps are in `docs/macos-native-applications.md`.
- cdadfae: Creating several Applications at once no longer fails some of them with "network with name sf-app-… already exists". Every deploy makes sure each Application's network exists, so two deploys could create the same one; the one that loses now finds it there and goes on.
- ff5e5c0: The console manages private access. An unpublished Compose Application's Configuration has **Applications with access**, to grant or remove other container Applications, and the Summary lists who reaches whom with each service's address to copy. Editing an unpublished Compose Application no longer asks for a Hostname and a web service it cannot have.
- 0afbba3: The console's container terminal opens on images without Bash, such as Alpine, with their `sh`. It used to end at once with code 127. Bash is still the shell wherever the image has it.

## 0.10.0

### Minor Changes

- 04e5666: An Application's Summary shows what it runs right now. A card under the container that receives requests says how many processes run and the CPU and memory they use together, and opens into the process tree as a table with each command, PID, CPU and memory. A Compose Application has a chip per container. The card reads again every 5 seconds while the Summary is open, and replaces the Command line the container used to show. `GET /apps/id/{id}/processes` returns the same tree by container: from the cgroup on Linux, from `ps` under the s6 runner on macOS, and from `docker top` for containers. Variable values in command lines are replaced with `[redacted]`. CPU is what `ps` reports: on Linux the average over the process's lifetime, on macOS a recent average.

### Patch Changes

- 3669988: Events name who acted: work done with the Platform key shows as **admin**, and work the Platform starts on its own, like a scheduled run, shows as **system**. The Platform key is listed in Settings, API keys, as admin and cannot be revoked there. New keys cannot take the labels admin or system.
- a44685b: `-D <dir>` (or `--data-dir`, or `SELF_HOST_DATA_DIR`) keeps everything the Platform stores under one directory, the way `postgres -D` does: configuration, state, certificates, Compose projects and their data, Virtual machines, and native Application data on Linux. Without it the default paths apply, and nothing is moved. `install.sh` passes `SELF_HOST_DATA_DIR` on to `init` and the macOS LaunchDaemon. `serve --port` (or `-p`) picks the API port, so two Platforms can run on one machine for testing.
- 6f7c876: Upgrading a macOS Host with `install.sh` no longer stops with `Bootstrap failed: 5` and needs a second run. The installer now waits for launchd to let go of a daemon it unloaded before loading it again, retries the load a few times, and then checks that each daemon stays up and that the Platform answers on its API before it reports success.
- fde566b: Git Applications build on macOS Hosts. A build's isolated Docker configuration now uses the Operator's Docker endpoint and plugin directories, so it reaches Colima and finds buildx.
- 72f50a2: Removing a native Application with its data works on macOS. Its data goes, and its home stays as an empty folder: macOS refuses to delete the home of a registered account, even as root, and deleting the account needs an approval on the Mac's screen.
- bda44c4: Removing an Application can now delete everything it stored. The console's Remove dialog has **Also delete everything it stored**, confirmed by typing the Application's name, and the API takes `?delete_data=true`: its named volumes, its project directory and data, its deployment history and the images only it used go with it. Without it, removal keeps the data as before.
  
  `self-host prune` removes what removed Applications left behind, and Git builds no deployment keeps: volumes, project directories, images and deployment history. It lists each item with the space it frees and asks first. It is safe while everything runs: it only touches names that carry an Application id, and anything that still has a container stays. `GET /prune` shows the list and `POST /prune` runs it as a task.
- 64e5267: Settings has a Scheduled tasks pane: work the Platform runs on its own. The first is **Remove unused data**, which runs `prune`. It is off until turned on, then runs daily or weekly on Sunday at the hour picked in the browser's time. Each run is a task with its outcome in Events, a run missed while the Host was off runs once on start, and runs never overlap. **Run now** starts one at once.
- 93189ba: `self-host uninstall` removes everything the Platform created on the Host, on Linux and macOS: Applications with their data and images, Virtual machines, native Applications and their accounts, the CA in the system trust store, the host DNS routing and the data directory. It lists what goes and asks first, and only touches this Platform's names when Docker is shared. On macOS, `uninstall.sh` runs it and then removes what the installer set up. The installer now marks the `/etc/resolver` file it writes, so the uninstall can tell it from Tailscale's.

## 0.9.2

### Patch Changes

- a21ab99: Typing or pasting `NAME=value` into the Name field of the Add variable dialog now fills both fields, so a line copied from a `.env` file lands in the right place. An `export` prefix and one pair of quotes around the value are dropped.
- 8879353: Routes are now a hostname, a path and what they do. The Add route dialog picks the domain first, the Zone or any other domain pointed at the Host, then a subdomain or the whole hostname; before, it appended the Zone to anything typed. It then chooses between sending requests to the Application and redirecting them to a path or URL, so sending `admin.example.com` to `/admin` no longer needs a regular expression. A redirect on a new hostname makes it an alias too, so the Host answers it.
  
  Redirects are listed with the routes, on the Routes page and an Application's Routes tab, and Hostname, Alias, Path and Redirect each have an icon, the alias a corner arrow. A redirect written as a pattern still opens in the pattern editor. On the Summary, a Domains card lists the hostnames, paths and redirects in sections with their icons and an add action each, drawn as a tree. The card folds, and chips show one kind at a time when there is more than one. The connected databases get the same card, each line with an env chip naming the variable that carries its connection URL, and the diagram fits its card, scrolling only when its content cannot wrap any further. The Application shows the container that receives requests, with its image and port, and the arrow from the domains ends on it; any other containers are listed under it in the same card. That container lists its image, port and start command, or for a Git Application its repository, ref, the file it was built from and the deployed commit with a badge saying whether the ref has moved since. The database page draws the same way: its Applications in a card with their env chips, and the database and volume with their facts.
- 2d3400f: Updating a Git Application now checks the saved branch or tag as soon as the dialog opens and lists the commits it would deploy, newest first, down to the deployed one. When nothing moved it says so and offers to rebuild the deployed commit. Changing the branch or tag is one action away, in the console's form style.
- f89b800: An Application that failed because a required Variable has no value can now have its Variables set one at a time. Before, each save failed while any other required Variable was still empty. On a failed Application a Variable change is only saved, and Retry redeploys it; Retry also redeploys now when nothing else changed, where before it did nothing.

## 0.9.1

### Patch Changes

- 5e8857e: The Configuration tab of an image, Compose, Git or custom image Application now edits its Variables: add one, replace its value or remove it. Before, only a native Application had this editor, and the others needed `self-host apps env set` on the Host.
- 591e106: A container image Application can now listen on a port other than 80. The create and edit forms have a Web port field, and the API accepts `web_port` when creating one from an image. Changing the port of an existing image Application now recreates its container on the new port; before, the change was saved but the container kept publishing the old one, and the Hostname answered with an error.
- 9ed1622: Git builds with a Dockerfile `RUN` no longer need a C compiler on the Host. The release now ships the static build launcher with its bundled s6 tools, and the Platform uses it instead of compiling one with `/usr/bin/cc`. A Platform built from source still compiles it.

## 0.9.0

### Minor Changes

- 141be8a: An Application can answer a request with a redirect instead of serving it. Each rule is a regular expression over the request URL and a location, with `${1}` for the pattern's groups and a choice of 301 or 302. Rules are tried in order on every hostname the Application owns, before the request reaches it, so they answer even while it is stopped. The Routes tab lists them and adds, edits or removes one in a dialog; `PUT /apps/id/{id}/routes` takes them as `redirects`.
- 5cfc3fe: An image Application can enable a deploy trigger from its Deployments tab, like a Git Application. CI calls `POST /deploy/{id}` with the trigger token after pushing to the image's registry repository, naming the new tag in `image` or leaving it out to pull the configured reference again. The trigger binds to the repository, so a different tag deploys and a different repository is refused.
- 78498e8: A hostname without a certificate of its own now answers HTTPS with the Platform's own certificate instead of refusing the handshake. A browser reaching the Host directly still rejects the mismatch, but an edge proxy that trusts the origin on its own terms, such as Cloudflare in "Full" mode, completes the handshake and serves the Application. A Host whose domains all sit behind such a proxy needs no ACME account.
- 1de8cb3: Public certificates follow the Applications. Save an ACME account once, in the new Public certificates card of Settings or with `PUT /certificates/acme`, and every Hostname outside the DNS Suffix is issued a certificate: Hostnames, aliases and route rule names alike. The daemon reads the account and the Hostnames again every minute, so a new name, a removed one or a changed CA no longer needs a restart or a hand-edited `public.json`. The card lists each certificate's state, expiry and last error.

## 0.8.0

### Minor Changes

- 77f8cd5: The proxy can send an Application's Web Target address as `Host` instead of the Hostname a client asked for. Turn it on for every Application under Settings > General, or set `rewriteHost` on `PUT /settings`. Each Application can override it on its Routes tab, or with `rewrite_host` on `PUT /apps/id/{id}/routes`. A change applies at once, without a rebuild or restart, and `X-Forwarded-Host` still names the Hostname. Use it for an app that refuses any `Host` but a loopback one, such as `laya-apple serve`.
- 0c67949: The Operator API serves Virtual machines under `/virtual-machines` instead of `/environments`, and the console links to them as `#virtual-machine-<id>` and `#new-virtual-machine`. The old paths are gone, with no alias. Existing machines keep their records, MAC addresses and Lima instances.

### Patch Changes

- 0c67949: The address bar follows the console. Opening an Application, a Virtual machine, Events or a custom image writes its fragment, so the URL can be shared or bookmarked, and a screen without one clears the previous fragment. A link to a removed Application no longer traps Back.

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
