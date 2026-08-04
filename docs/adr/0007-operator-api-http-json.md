# Operator API = HTTP JSON (not gRPC)

gRPC suited CLI ↔ daemon and log streaming, but makes a future web console harder (browser needs gRPC-Web, a gateway, or a BFF). We chose **HTTP JSON** as the Operator API from MVP day one: the same seam serves the CLI now and a web console later, aligned with Disco. MVP logs use HTTP streaming (e.g. SSE or chunked responses). gRPC/`tonic` are out of MVP.

**Status:** accepted  
**Supersedes:** gRPC protocol in ADR-0001, ADR-0005, and ADR-0006
