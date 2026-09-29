# self-host

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
