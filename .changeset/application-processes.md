---
"self-host": minor
---

An Application's Summary shows what it runs right now. A card under the container that receives requests says how many processes run and the CPU and memory they use together, and opens into the process tree as a table with each command, PID, CPU and memory. A Compose Application has a chip per container. The card reads again every 5 seconds while the Summary is open, and replaces the Command line the container used to show. `GET /apps/id/{id}/processes` returns the same tree by container: from the cgroup on Linux, from `ps` under the s6 runner on macOS, and from `docker top` for containers. Variable values in command lines are replaced with `[redacted]`. CPU is what `ps` reports: on Linux the average over the process's lifetime, on macOS a recent average.
