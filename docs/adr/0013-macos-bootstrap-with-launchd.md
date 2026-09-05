# Bootstrap macOS with launchd supervision

The first Host is a Mac used as a household server. The Platform, Platform Infra
and Applications configured to start must return after reboot or startup from
shutdown without an interactive login. Bootstrap will prepare self-host, a
Docker runtime and Compose; launchd will supervise self-host as a LaunchDaemon.

A LaunchAgent depends on a login session and does not meet the requirement.
Putting s6 between launchd and self-host adds installation and lifecycle work
without meeting an additional MVP requirement. Use launchd directly for the
Platform, while keeping future native Application supervision a separate concern.

The Docker runtime and pre-login access to storage and credentials still require
validation on the Mac. This decision selects the Platform supervisor; it does
not claim unattended startup of the complete stack has been demonstrated.

**Status:** accepted for implementation

**Amends:** [ADR-0001](0001-platform-binary-docker-grpc-lan.md) by selecting
macOS as the first Host and making unattended Bootstrap part of the MVP.

**Evidence:** [supervision comparison](../research/self-host-supervision.md).
