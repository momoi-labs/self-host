---
"self-host": minor
---

When the saved deSEC zone contains the DNS Suffix, the Platform orders one
Let's Encrypt wildcard for the Suffix and serves it for every name one label
under it, the console included. Names keep the local CA certificate until the
wildcard is issued and after a failed renewal.
