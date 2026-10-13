---
"self-host": patch
---

The Add route dialog now takes a whole hostname, so a route can answer on any domain pointed at the Host, such as `admin.example.com`. Before, it always appended the DNS suffix, and `admin.example.com` became `admin.example.com.momoi.cloud`. Suggestions list every hostname the Applications already answer on.
