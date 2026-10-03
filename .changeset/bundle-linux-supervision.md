---
"self-host": patch
---

Include s6 and its supervision helpers in Linux releases. The installer and
Linux packages install a private bundle with its license notices, so native
Applications no longer require a separate distribution s6 package. Boot
supervision remains opt-in through `SELF_HOST_NATIVE=1`.
