# The console answers on console and keeps admin as an alias

New Hosts name the console `console.<suffix>`. `admin` read as a role, and the
console is the product's name for the screen. `admin.<suffix>` stays as a
legacy alias: same DNS answer, same certificate names, same routing, and
still unavailable to Applications.

The LAN wildcard certificate covers both names, so no Host needs a new
certificate. An existing Host keeps its `admin` Record, and the wildcard
answers `console.<suffix>`. A new Host gets both Records from `init`. The CLI,
the installer and the trust-ca instructions prefer `console.<suffix>`.

The Platform's own API key label and the audit actor stay `admin`. They are
identities, not hostnames.

Changing the suffix of an initialized Host is out of scope here. `self-host
change-suffix` does it with the Platform stopped. See
[the research note](../research/suffix-change-on-initialized-host.md).
