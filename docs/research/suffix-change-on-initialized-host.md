# Changing the DNS Suffix of an initialized Host

`self-host init` with another suffix still fails with `DnsSuffixMismatch`
([src/bootstrap.rs:143](../../src/bootstrap.rs)). Option B below is built as
`self-host change-suffix`. This note lists what depends on the suffix, the
three options, and what was built.

## What depends on the suffix

| State | Where | What breaks on a change |
| --- | --- | --- |
| Stored suffix | `dns_suffix` state, written at [src/bootstrap.rs:189](../../src/bootstrap.rs) | Every reader below uses the old value. |
| DNS config | `dns.json`, built at [src/bootstrap.rs:96](../../src/bootstrap.rs), read at [src/uninstall.rs:305](../../src/uninstall.rs) | The Zone answers the old suffix only. |
| LAN certificate | CN and SANs `*.<suffix>` and `<suffix>` at [src/tls.rs:238](../../src/tls.rs) | Clients get a name mismatch. Startup validation fails at [src/tls.rs:139](../../src/tls.rs). |
| Platform Records | `console`, `admin` and `*` ([src/dns_records.rs:338](../../src/dns_records.rs)) | Names are relative, so they follow the suffix after a rebuild ([src/dns_records.rs:366](../../src/dns_records.rs)). |
| Machine Records | `machine_name` strips the suffix ([src/dns_records.rs:389](../../src/dns_records.rs)) | A machine hostname under the old suffix no longer maps to a Record. |
| Machine hostnames | stored in full, [src/environments.rs:149](../../src/environments.rs) | Saved SSH commands and names point at the old suffix. |
| Application hostnames and aliases | default from the suffix ([src/apps.rs:197](../../src/apps.rs)), stored in full | Every Application answers only on the old name. |
| Route rules | stored in full, checked at [src/routes.rs:176](../../src/routes.rs) | Rules for the old suffix match nothing. |
| Console hostname | [src/config.rs:115](../../src/config.rs), [src/proxy.rs:459](../../src/proxy.rs), alias at [src/routes.rs:167](../../src/routes.rs) | The proxy serves the old console name. |
| CLI config | `api_base_url` ([src/config.rs:5](../../src/config.rs)), used at [src/cli.rs:48](../../src/cli.rs) | Every CLI call goes to the old name. |
| Device trust | CA fetched from the console name (`trust-ca --from`, [src/main.rs:799](../../src/main.rs)) | Devices keep the CA but resolve the old name, and need the Host's DNS again. |
| Public certificates | outside the suffix, [docs/public-certificates.md](../public-certificates.md) | Unaffected, except an Application that moved into or out of the suffix. |

The CA itself does not depend on the suffix. Only the leaf does, so devices
that trust the CA need no new trust step.

## Options

| Option | What it does | Cost |
| --- | --- | --- |
| A. Stop and re-init | Document that a new suffix means a new Host state. | Zero code. The Operator loses Applications and data, or restores from backup. |
| B. Offline migration command | `self-host change-suffix <new>` with the Platform stopped: rewrite `dns_suffix`, `dns.json`, regenerate the leaf, rewrite stored hostnames, aliases and route rules that end in the old suffix, then rebuild Records. | Medium. One pass over six stores plus a rollback copy. Needs tests for each store above. |
| C. Dual suffix period | Serve both suffixes, with the old one as an alias, until the Operator removes it. | High. Every name check (apps, routes, Records, certificates) gains a second suffix, and ADR-0037 already shows the cost of one legacy alias. |

## What was built

The Operator chose B with no legacy alias for the old suffix: the old suffix
stops answering at once. The `admin.<suffix>` alias from ADR-0037 stays.

`self-host change-suffix <new>` ([src/suffix.rs](../../src/suffix.rs)):

1. Refuses while the Platform runs. The state lock tells it.
2. Builds a plan and prints every change. `--dry-run` stops here, writes
   nothing, and works while the Platform runs.
3. Copies `state/`, `dns.json`, `config.json` and `certs/` to
   `<data dir>.before-suffix-<old>-<time>`, mode `0700`, and prints the path.
   A failed copy stops the command before any write.
4. Rewrites, in order: Applications (Hostname, aliases, route rules), machines
   (hostname, SSH, tunnel and web addresses), the console Records, the stored
   suffix, `dns.json`, the LAN leaf (same CA), and the CLI `api_base_url`.
   On a failure it names the copy to restore from.

Per row of the table above:

| State | Result |
| --- | --- |
| Stored suffix, DNS config | Rewritten. |
| LAN certificate | Reissued from the same CA. |
| Platform Records | Relative, so they follow at the next start. `console` and `admin` are ensured. |
| Machine Records and hostnames | Hostnames rewritten, so `machine_name` maps them again. |
| Application hostnames, aliases, route rules | Rewritten when they end in the old suffix. Applications need no restart: the proxy reads names from the store. |
| Console hostname | Derived from the suffix. |
| CLI config | `api_base_url` host rewritten. |
| Device trust | The CA is unchanged. Devices need the Host's DNS for the new suffix. |
| Public certificates | Unchanged. Public suffix mode (ADR-0038) applies if the new suffix sits inside the saved zone. |

Not rewritten, reported by name as warnings: variables whose value mentions
the old suffix, and redirects that mention it. The command also refuses a new
suffix that would give one Hostname to two Applications.

Out of scope for code: pg-yard moves to `yard.<new suffix>` by hand on the
Hosts that run it.
