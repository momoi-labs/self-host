# Git builds are plain Docker builds unless strict

A Git build runs the repository's Dockerfile as a plain `docker build`. Its
steps run as whatever user the Dockerfile sets, root included. The build
guard, a static launcher that every `RUN` went through to refuse root, drop
capabilities and set `no-new-privileges`, is now opt-in per Application as
**strict build** (`strict_build` in the Git source).

The guard cost more than it protected here:

- **It refused most real Dockerfiles.** Nearly every one installs packages as
  root before it switches user. The Rails Dockerfiles on the cloud server and
  mochi-cut on the home-server could not build.
- **It refused every `RUN` on a Mac.** The guard runs inside the Linux build
  container, but it is built for the Host, so the build refused any Host whose
  own OS is not Linux, Colima included.
- **It guarded a narrow window.** Root inside a BuildKit container is not root
  on the Host; leaving it needs a kernel or runtime bug. The running container
  of a Git Application already drops every capability and sets
  `no-new-privileges`. Build steps still have the network and run whatever
  they want as their user.
- **The Operator picks the repository.** The guard is defense in depth
  against code the Operator does not trust, and most Git Applications come
  from the Operator's own repositories.

What stays in both modes is the identity of what runs: the final stage needs
an explicit numeric nonzero `USER`, the built image's user is checked, and a
Git Compose service needs a numeric nonzero `user`. The rest of the
Dockerfile policy also stays: bases pinned by digest, no `ONBUILD`, no parser
directives or custom frontends, no `BUILDKIT_SYNTAX`, no insecure entitlements
and no heredocs.

Strict build keeps the guard's policy as it was, including its Linux-only
limit. A Dockerfile that built under it builds unchanged without it, so
existing Applications move to the plain build.

## Considered options

- **Keep the guard and ship it to macOS.** Fixes the Mac, but every Dockerfile
  that installs packages as root still fails.
- **A rootless builder or userns-remap.** Maps root in the build to an
  unprivileged Host user without rewriting the Dockerfile, which is the
  standard answer to untrusted builds. It is Host Docker configuration the
  Platform does not own today; it can come later as what strict build means.

Status: accepted.
