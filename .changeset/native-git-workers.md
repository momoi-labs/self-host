---
"self-host": minor
---

Native Applications can build from a Git repository and run under the native
runtime. The form gains a Worker or static site mode that builds a module
Worker (a `main` config) or a static site (an `assets` config) with a pinned
Wrangler and runs it locally with workerd. Nothing is deployed to Cloudflare.
Host native build limits (source size, file count and build payload) are
editable under Settings > General > Applications and apply to the next build.
