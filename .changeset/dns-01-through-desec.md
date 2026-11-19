---
"self-host": minor
---

Public certificates can be validated through DNS-01 with deSEC. Settings has a
DNS zone and a deSEC token field. Hostnames inside the zone get their
certificate with no inbound port 80, and the token never leaves Platform
State. Hostnames outside the zone keep HTTP-01.
