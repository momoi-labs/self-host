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

The Linux daemon retains root for account provisioning and launch setup.
The s6 launcher also needs root to join a cgroup and drop the child's
privileges. This is the accepted N2 boundary, resolving the pending-decision
note in the wave 1 handoff. Application code never inherits that privilege.
Main commands, readiness checks, hooks, builds, terminals and their
descendants all use the N1 identity and cgroup checks before exec.

A separate privileged helper could reduce the daemon's authority. We defer
it because it would add an authenticated IPC protocol and a second lifecycle
boundary while N1 already performs privileged setup in the daemon. The
tradeoff is explicit: a daemon compromise has Host-wide authority. Protected
root-owned supervision files and fail-closed child setup are required; an
Application cannot write its launcher or turn a failed check into a root
execution path.

The Host service manager starts one s6 supervisor tree and delegates cgroup
controllers. It does not supervise individual Applications. Restarting the
daemon reconnects to that tree. Restarting the Host restores each
Application's recorded running or stopped intent.

A hook, a build or a terminal is not a privileged path. All four purposes go
through the same order, so the console's terminal on a Native Application is
a shell as that account and nothing more.

N3 connects this boundary to the Operator API. Native create, update,
lifecycle actions, Variables and logs use the installed N1 launcher and
N2 supervisor. The API requires a dedicated Application Account and keeps
its Runtime kind fixed.

**Status:** accepted
