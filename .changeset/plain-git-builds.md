---
"self-host": minor
---

Git builds are a plain `docker build` by default, so a Dockerfile can install packages as root before it switches user, and Git Applications with a `RUN` now build on a Mac. The image still has to run as a numeric nonzero `USER`. Turn on **Strict build** in an Application's Git source for a repository you do not trust: every `RUN` then goes through the build guard as before, which refuses root and needs a Linux Host.
