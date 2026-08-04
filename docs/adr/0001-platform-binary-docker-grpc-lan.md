# Platform = binary + Docker on LAN; control via API

Primary environment is a LAN Host, without depending on external SaaS on the happy path. The Platform is a single binary (daemon) that starts Infra and Applications as Docker containers; the Operator controls it via a CLI talking to the daemon API. UI, Kubernetes, and git-as-source are out of MVP — MVP Deploy is image pull or local build.

The Operator API protocol was initially gRPC; that is **superseded by ADR-0007** (HTTP JSON).

**Status:** accepted
