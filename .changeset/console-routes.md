---
"self-host": minor
---

Manage routes from the console. A Routes page lists every hostname and path the proxy answers, and each Application has a Routes tab. Adding, changing or removing a route applies at once through `PUT /apps/id/{id}/routes`, without a rebuild, pull or restart. A path rule without a `target` follows its Application's Web Target.
