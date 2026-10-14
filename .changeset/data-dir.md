---
"self-host": patch
---

`-D <dir>` (or `--data-dir`, or `SELF_HOST_DATA_DIR`) keeps everything the Platform stores under one directory, the way `postgres -D` does: configuration, state, certificates, Compose projects and their data, Virtual machines, and native Application data on Linux. Without it the default paths apply, and nothing is moved. `install.sh` passes `SELF_HOST_DATA_DIR` on to `init` and the macOS LaunchDaemon. `serve --port` (or `-p`) picks the API port, so two Platforms can run on one machine for testing.
