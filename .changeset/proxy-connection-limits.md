---
"self-host": patch
---

The proxy no longer runs out of open files behind a CDN. It closes a connection after 960 seconds with no request in flight, drops clients that do not finish the TLS handshake within 10 seconds, and detects vanished peers with TCP keepalive and HTTP/2 pings. Responses, streams and WebSockets that run longer are not cut. `serve` raises its open file limit to the hard limit at start, and the proxy pauses after a failed accept instead of flooding the log.
