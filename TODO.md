# Deferred work

The macOS MVP will use launchd (a LaunchDaemon) to supervise self-host.
Host-level s6 support is outside the MVP.

- [ ] Add s6 support to run and supervise Applications without containers after
  the MVP. Define the first native Application and its Bootstrap requirements
  then. Revisit [issue #40](https://github.com/momoi-labs/self-host/issues/40)
  and [PR #41](https://github.com/momoi-labs/self-host/pull/41).

See the [product vision](docs/vision.md), [MVP plan](docs/mvp-plan.md) and
[runtime decision](docs/adr/0014-compose-applications-and-future-native-supervision.md).
