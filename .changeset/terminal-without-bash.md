---
"self-host": patch
---

The console's container terminal opens on images without Bash, such as Alpine, with their `sh`. It used to end at once with code 127. Bash is still the shell wherever the image has it.
