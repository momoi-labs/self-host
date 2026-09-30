# Application Accounts are execution identities and cgroups are resource controls

ADR-0014 deferred running Applications without containers. Before that can
land, the Platform needs to say what a native process runs as and what
bounds it. Every Native Application runs under its own Linux system account,
`sf-app-<id>`, that exists for nothing else: `nologin` shell, its own group,
no supplementary groups, a `0750` home. The Platform provisions the account
as root and launches the command as that account after dropping every
privilege in the child, in a fixed order that fails closed. There is no
fallback: when a step of that order fails, the command does not run. The
Platform never runs an Application, a hook, a build or a terminal as root,
and never as its own account.

Each Application also gets a cgroup v2 directory under one the Platform
owns. CPU, memory and task limits apply to the whole tree, `memory.oom.group`
makes an OOM take the tree as one, and `cgroup.kill` ends it as one,
detached grandchildren included. That is the extent of it: a cgroup bounds
resources and tears down; it does not hide the filesystem, the network or
other processes. Ownership and file modes do the filesystem part, which is
why the account and its home are shaped the way they are.

## Considered options

- **Run every native Application as the Operator's account.** Rejected. One
  compromised Application reads every other Application's files and the
  Platform's credentials, and can talk to Docker if the Operator can.
- **Put native Applications in containers.** Rejected. That is the container
  Runtime, which already exists; native exists for the workloads it does not
  fit.
- **One account per Application, limits by cgroup.** Chosen. Isolation
  between Applications comes from ordinary Unix ownership, limits and
  teardown from the kernel's process grouping, and neither pretends to be
  the other.

## Consequences

Provisioning needs root and launching needs root, so the daemon runs with
that privilege on a Linux Host that serves Native Applications. The command
itself never does; `tests/native_linux.rs` proves the identity, the empty
capability sets, the limits and the teardown on a real Host.

A hook, a build or a terminal is not a privileged path. All four purposes go
through the same order, so the console's terminal on a Native Application is
a shell as that account and nothing more.

Nothing in this decision is reachable from the Operator API yet. The record
can describe a native Runtime (ADR-0028) and is refused until the lifecycle
slice wires these pieces to it.

**Status:** accepted
