---
"self-host": patch
---

An Application that failed because a required Variable has no value can now have its Variables set one at a time. Before, each save failed while any other required Variable was still empty. On a failed Application a Variable change is only saved, and Retry redeploys it; Retry also redeploys now when nothing else changed, where before it did nothing.
