---
"self-host": minor
---

Public certificates follow the Applications. Save an ACME account once, in the new Public certificates card of Settings or with `PUT /certificates/acme`, and every Hostname outside the DNS Suffix is issued a certificate: Hostnames, aliases and route rule names alike. The daemon reads the account and the Hostnames again every minute, so a new name, a removed one or a changed CA no longer needs a restart or a hand-edited `public.json`. The card lists each certificate's state, expiry and last error.
