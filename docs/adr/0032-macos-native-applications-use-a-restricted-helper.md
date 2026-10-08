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

## Amendment: deletion retires the account

macOS deletes a directory record only when the responsible process has Full
Disk Access. The Platform daemon has none, cannot be prompted, and a grant to
an unsigned helper would not survive an upgrade
([#168](https://github.com/momoi-labs/self-host/issues/168)). Deleting a
Native Application therefore retires its Application Account instead: the
service is removed, the per-user launchd domain is booted out, retained data
is secured as root, and the user and group records carry a retired marker.
The records stay, so their UID and GID are never handed out again and the
same Application id cannot adopt the retained data. Only `uninstall.sh`
deletes records, from a terminal that macOS can ask for Full Disk Access.
