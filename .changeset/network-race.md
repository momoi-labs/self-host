---
"self-host": patch
---

Creating several Applications at once no longer fails some of them with "network with name sf-app-… already exists". Every deploy makes sure each Application's network exists, so two deploys could create the same one; the one that loses now finds it there and goes on.
