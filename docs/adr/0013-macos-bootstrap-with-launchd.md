# Bootstrap macOS with launchd supervision

The first Host is a Mac used as a household server. The Platform, Platform Infra
and Applications configured to start must return after reboot or startup from
shutdown without an interactive login. Bootstrap will prepare self-host, a
Docker runtime and Compose; launchd will supervise self-host as a LaunchDaemon.

A LaunchAgent depends on a login session and does not meet the requirement.
Putting s6 between launchd and self-host adds installation and lifecycle work
without meeting an additional MVP requirement. Use launchd directly for the
Platform, while keeping future native Application supervision a separate concern.

The Docker runtime and pre-login access to storage and credentials were
observed working on the real Mac on 2026-09-08: launchd started the Platform
and the Colima VM before any login, and the stack answered on the LAN
([docs/macos-host.md](../macos-host.md)). The decision stands. It delivers
what it claims only under Host conditions it does not itself provide: a
wired network, because the Wi-Fi password sits in a login keychain nobody
has unlocked at boot; FileVault off; and a Host that can power itself on,
which a laptop cannot, because `autorestart` is a desktop capability.

**Status:** accepted for implementation

**Amends:** [ADR-0001](0001-platform-binary-docker-grpc-lan.md) by selecting
macOS as the first Host and making unattended Bootstrap part of the MVP.

**Evidence:** [supervision comparison](../research/self-host-supervision.md).
