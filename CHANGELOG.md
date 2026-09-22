# self-host

## 0.4.1

### Patch Changes

- 4845b50: Show the installed version on the login screen and operator console, and expose it through `self-host --version`.

## 0.4.0

### Minor Changes

- 26283d1: Install self-host through Homebrew on macOS or download Linux packages for
  pacman, apt, and dnf. Releases update the Homebrew Cask and include Arch, Debian,
  and RPM packages. AUR publication is disabled until maintainer access is ready.
  
  Keep Platform State in SQLite and run console actions as queued Tasks with
  status and event history. The console shows paginated events, Virtual machine
  runs, and unsaved changes. Narrow screens stack the sidebar above the content.
