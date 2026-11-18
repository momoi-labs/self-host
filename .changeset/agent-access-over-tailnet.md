---
"self-host": minor
---

AI agents can configure the Platform through an MCP server at `/mcp`, reached
only on the Host's Tailscale name over HTTPS. Agent keys have scopes, expire
within a year, can be bound to one tailnet device, and cannot open the console
or a terminal. Events marks changes made by an agent or through MCP. The
Operator API on port 3721 now listens on loopback only, so other machines use
the admin hostname or the tailnet name.
