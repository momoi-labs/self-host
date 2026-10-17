---
"self-host": patch
---

A Compose deploy that fails on a missing bind source now says which source is missing, the Host path it looked for, and to create the file there. It used to say only "prepare bind source" and "No such file or directory". When the source comes from a Variable, the message leaves out the path, so the Variable's value stays out of the failure report.
