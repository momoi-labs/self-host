# macOS native Applications use a restricted helper

Keep the macOS Platform under the Operator and run each Native Application
as its own non-administrator `sf-app-<id>` account. The installer grants the
Operator one exact sudo command, a protected copy of `self-host native-control`
that accepts bounded, validated lifecycle requests and derives all privileged
paths. A root LaunchDaemon owns the shared s6 scan tree independently of the API.

This extends ADR-0031 to macOS. Darwin launches replace supplementary groups
with the Application's group, drop real, effective and saved GID/UID, and
verify the result before exec. s6 runs setup and main commands in its service
session and cleans the foreground process group on stop or crash.

macOS has no cgroup boundary or verified equivalent of Linux `no_new_privs`
in this MVP. Detached descendants, resource limits and metrics, native
terminals, Git builds, and managed PostgreSQL connections remain unsupported.
The API rejects unsupported options and the console reads Host capabilities.
Account separation is not a filesystem sandbox.

Status: accepted. Privileged macOS acceptance and reboot validation remain
release gates.
