---
"self-host": minor
---

The CLI manages routes, Virtual machines and databases, like the console. `self-host routes` adds and removes aliases, path rules and redirects, and sets whether the proxy rewrites the Host header. `self-host vms` creates, starts, stops, restarts, retries and removes Virtual machines. `self-host db` creates, recreates and removes PostgreSQL databases, connects and disconnects Applications, and prints a connection's URL. Each command waits for its task and prints the failure when it fails.
