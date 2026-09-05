# macOS Compose MVP implementation plan

Implementation is tracked in [issue #42](https://github.com/momoi-labs/self-host/issues/42).

Approved scope: Bootstrap a macOS Host and operate Hermes through Compose so
household Consumers can chat from a LAN browser. The Platform and configured
Applications must return after restart without an interactive login.

The [vision](vision.md) explains the product boundaries. This plan is not evidence
that the Docker runtime or Hermes workflow has been tested on the Mac.

## Delivery sequence

1. **Validate unattended Docker startup.** Select a runtime that can start before
   login on the Apple Silicon Mac. Test runtime access, storage paths and required
   credentials after reboot and startup from shutdown. Document prerequisites
   and any blocking Host condition before building the rest of the workflow.
2. **Bootstrap the Platform.** Extend the installer to prepare the chosen Docker
   runtime, Compose, self-host and Platform Infra. Install self-host as a launchd
   LaunchDaemon and handle dependencies becoming available during startup.
3. **Add Compose Applications through the console.** Accept an Operator-provided
   Compose definition and the configuration needed by its services. Use the
   official Hermes example to cover command, environment, ports, persistent
   storage and browser access. Define and document the supported subset; do not
   promise full Compose compatibility or invent a universal Application manifest.
4. **Operate the Application.** Support install, start, stop, restart, state and
   logs through the console and its API. Preserve Application identity and
   isolation from Platform Infra. Define how Compose services map to Application
   state and routing as part of this implementation.
5. **Validate the complete workflow.** Configure Hermes, including its model
   provider and browser access. Check real conversations, data persistence and
   recovery on the Mac from a Consumer's machine on the LAN.

The installer research identifies requirements; it does not authorize running
the Hermes installer as the Platform's installation strategy. Application-specific
configuration steps must be explicit in the delivered workflow.

## Acceptance criteria

- [ ] From documented Host prerequisites, the installer leaves the Platform
  available on the LAN without an interactive session remaining open.
- [ ] The Operator supplies the Hermes Compose definition through the console
  and provides its required configuration through the documented workflow.
- [ ] Both household Consumers can chat with Hermes from their browsers,
  including access from the Linux workstation.
- [ ] Install, start, stop and restart work; state and logs expose startup failures.
- [ ] Configuration and persisted conversation data survive Application restart
  and container recreation without deleting the persistent storage.
- [ ] After a Host reboot and a startup from shutdown, the Platform, Platform
  Infra and Applications configured to start become available without login.
- [ ] The supported Compose behavior and Host prerequisites are documented.

## Validation

Use focused tests for Compose lifecycle behavior, configuration and persistence
boundaries, including failure reporting. Complete the final acceptance checks on
the real Mac and record the macOS version, runtime version and relevant boot
conditions. CI on Linux alone cannot establish unattended macOS startup.

Process supervision does not by itself prove Application readiness. If a Host
condition prevents unattended startup, report it explicitly and resolve it
before marking the issue complete; requiring a login is not an accepted fallback.

## Deferred

s6 for Applications without containers, other Host operating systems, messaging
channels, additional Application recipes, a catalog, backup/restore and automatic
updates. 1Password is not required. Full Compose compatibility and a shared
manifest for all runtimes are outside this slice.
