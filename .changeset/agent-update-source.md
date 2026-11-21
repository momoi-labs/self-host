---
"self-host": minor
---

An agent can change a Git Application's source through the MCP tool
`update_source` and rebuild it, for example to remove a build arg. The tool
needs a key with the `apps:write` and `host` scopes. An agent key can now hold
`host`, which is off by default; the API keys form warns what it allows. Agent
keys without `host` behave as before.
