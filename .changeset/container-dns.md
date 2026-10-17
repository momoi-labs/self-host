---
"self-host": patch
---

Application containers resolve names under the DNS Suffix, such as another Application's Hostname. They used Docker's resolver, which on a Mac is Colima's VM and does not know the Suffix, so a call to `https://comfyui.momoi.internal` failed with `ENOTFOUND`. When the Platform serves DNS, containers now use it, and it forwards every other name upstream. Existing Applications pick this up the next time their containers are recreated.
