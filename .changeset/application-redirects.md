---
"self-host": minor
---

An Application can answer a request with a redirect instead of serving it. Each rule is a regular expression over the request URL and a location, with `${1}` for the pattern's groups and a choice of 301 or 302. Rules are tried in order on every hostname the Application owns, before the request reaches it, so they answer even while it is stopped. The Routes tab lists them and adds, edits or removes one in a dialog; `PUT /apps/id/{id}/routes` takes them as `redirects`.
