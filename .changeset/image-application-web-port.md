---
"self-host": patch
---

A container image Application can now listen on a port other than 80. The create and edit forms have a Web port field, and the API accepts `web_port` when creating one from an image. Changing the port of an existing image Application now recreates its container on the new port; before, the change was saved but the container kept publishing the old one, and the Hostname answered with an error.
