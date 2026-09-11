# Deferred work

Open work is tracked in issues. The ones in flight:

- [#89](https://github.com/momoi-labs/self-host/issues/89): run the first
  development environment on the Host.
- [#90](https://github.com/momoi-labs/self-host/issues/90): enrol credentials
  in a development environment without a browser.

The [development images TODO](docs/development-images-todo.md) keeps the test
history of the image prototype; items left unchecked there are superseded by
those two issues.

The macOS MVP will use launchd (a LaunchDaemon) to supervise self-host.
Host-level s6 support is outside the MVP.

- [ ] Add s6 support to run and supervise Applications without containers after
  the MVP. Define the first native Application and its Bootstrap requirements
  then. Revisit [issue #40](https://github.com/momoi-labs/self-host/issues/40)
  and [PR #41](https://github.com/momoi-labs/self-host/pull/41).

See the [product vision](docs/vision.md), [MVP plan](docs/mvp-plan.md) and
[runtime decision](docs/adr/0014-compose-applications-and-future-native-supervision.md).
