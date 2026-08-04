# Implementation in Rust

The Platform (CLI + daemon) is written in **Rust**, matching the Operator’s learning goal and a daemon + HTTP API design talking to the Docker Engine API. Go and TypeScript were practical alternatives (Docker/CLI ecosystem / proximity to Disco·Dokploy) and were set aside on purpose.

HTTP stack: **axum** (or equivalent) server; HTTP client in the CLI. The earlier gRPC/`tonic` choice is **superseded by ADR-0007**.

**Status:** accepted
