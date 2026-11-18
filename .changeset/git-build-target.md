---
"self-host": minor
---

A Git Application can name a Dockerfile stage as its build target. The build
stops at that stage and the numeric `USER` rule applies to it, so one
multi-stage Dockerfile can build several Applications. The console has a
Build target field. Compose sources do not take it.
