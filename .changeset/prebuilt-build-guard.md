---
"self-host": patch
---

Git builds with a Dockerfile `RUN` no longer need a C compiler on the Host. The release now ships the static build launcher with its bundled s6 tools, and the Platform uses it instead of compiling one with `/usr/bin/cc`. A Platform built from source still compiles it.
