---
"self-host": minor
---

Applications can save hostname/path routes to loopback targets through the API.
The proxy chooses the longest matching path and applies changes without
restarting Applications. Prefix handling is explicit; management and
certificate-challenge paths remain reserved.

Compose Applications can map existing named volumes explicitly and preserve
file, directory and read-only bind mounts. New Applications use private
networks, with explicit consumer grants for unpublished database providers.
Existing Applications retain their shared network policy. Unpublished services
report container health without requiring an HTTP target.

This adds storage and connectivity support. Managed PostgreSQL provisioning
and dedicated database screens remain pending.
