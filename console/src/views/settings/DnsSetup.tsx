import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  KV,
  KVKey,
  KVValue,
} from "@momoi-labs/kiso-react";

import { requireKey } from "../../lib/api.js";
import { bootstrapStatusQuery } from "../../lib/queries.js";
import type { BootstrapStatus } from "../../lib/types.js";

/**
 * The DNS setup cards of Settings: the Host a device points at, and how to
 * point it. The public page a device sees before it can log in is
 * `DeviceSetup`, served over plain HTTP; this is the Operator's copy inside
 * the console.
 */
/* Only one of these is ever relevant to the reader, so they stack as
   disclosures rather than as cards. */
const steps = (hostIp: string): Record<string, string[]> => ({
  macOS: [
    "Open System Settings → Network.",
    "Open the active connection and select DNS.",
    `Add ${hostIp} as a DNS server, then apply the change.`,
  ],
  Windows: [
    "Open Network Connections and the active adapter's properties.",
    "Open Internet Protocol Version 4 (TCP/IPv4).",
    `Use ${hostIp} as the preferred DNS server and save.`,
  ],
});

function Steps({ platform, hostIp }: { platform: string; hostIp: string }) {
  return (
    <details className="disclosure">
      <summary>{platform}</summary>
      <ol>
        {steps(hostIp)[platform].map((step) => (
          <li key={step}>{step}</li>
        ))}
      </ol>
    </details>
  );
}

/** `undefined` while loading, `null` when the Platform did not answer. */
function useBootstrapStatus(): BootstrapStatus | null | undefined {
  useEffect(() => {
    requireKey();
  }, []);
  const { data, isError } = useQuery(bootstrapStatusQuery);
  return data ?? (isError ? null : undefined);
}

/** The Host a device on the LAN points its DNS at. */
export function YourHost() {
  const settings = useBootstrapStatus();
  const suffix = settings?.dns_suffix;
  const hostIp = settings?.host_ip;
  const hostAddresses = settings?.host_addresses ?? [];

  return (
    <Card aria-labelledby="host-heading">
      <CardHeader>
        <h2 className="t-h3" id="host-heading">
          Your host
        </h2>
        <CardDescription>What a device on the LAN uses as its DNS server.</CardDescription>
      </CardHeader>
      <CardContent>
        <KV>
          <KVKey>Host IP</KVKey>
          <KVValue>{hostIp || (settings === undefined ? "Loading…" : "Unavailable")}</KVValue>
          {hostAddresses.length > 1 ? (
            <>
              <KVKey>All host addresses</KVKey>
              <KVValue>{hostAddresses.join(", ")}</KVValue>
            </>
          ) : null}
          <KVKey>DNS suffix</KVKey>
          <KVValue>{suffix || (settings === undefined ? "Loading…" : "Unavailable")}</KVValue>
        </KV>
      </CardContent>
    </Card>
  );
}

/** How to point a device at the Host, one platform at a time. */
export function DnsSetup() {
  const settings = useBootstrapStatus();
  const suffix = settings?.dns_suffix;
  const hostIp = settings?.host_ip;

  return (
    <Card id="settings-dns-setup" data-size="wide" aria-labelledby="instructions-heading">
      <CardHeader>
        <h2 className="t-h3" id="instructions-heading">
          Connect a device
        </h2>
        <CardDescription>Reach applications by hostname from a device on your network.</CardDescription>
      </CardHeader>
      <CardContent>
        {hostIp && suffix ? (
          <>
            <section className="stack-sm" aria-label="Instructions">
              <p className="muted t-label">
                Point your device's DNS resolver to the Host IP so that <code>*.{suffix}</code> resolves
                locally.
              </p>
              <div>
                <Steps platform="macOS" hostIp={hostIp} />
                <details className="disclosure">
                  <summary>Linux</summary>
                  <div className="dns-instructions">
                    <section className="stack-sm" aria-labelledby="linux-host-heading">
                      <h3 className="t-h3" id="linux-host-heading">On the Host</h3>
                      <p className="muted t-label">
                        Configure persistent DNS with systemd-resolved:
                      </p>
                      <pre><code>self-host setup-dns</code></pre>
                      <p className="t-metadata muted">This configuration survives reconnects and reboots.</p>
                    </section>
                    <section className="stack-sm" aria-labelledby="linux-device-heading">
                      <h3 className="t-h3" id="linux-device-heading">On another Linux device</h3>
                      <p className="muted t-label">
                        With systemd-resolved, paste this whole block into your terminal.
                        It detects the network interface used to reach the Host.
                      </p>
                      <pre>
                        <code>
                          {`dns_interface=$(ip -o route get ${hostIp} | awk '
  { for (i=1; i<NF; i++) if ($i == "dev") { print $(i+1); exit } }
')

if [ -n "$dns_interface" ]; then
  sudo resolvectl dns "$dns_interface" ${hostIp} &&
  sudo resolvectl domain "$dns_interface" ~${suffix}
else
  echo "Could not find a network interface to reach ${hostIp}. Check your connection." >&2
fi`}
                        </code>
                      </pre>
                      <p className="t-metadata muted">
                        These settings may be cleared when you reconnect or reboot.
                      </p>
                    </section>
                  </div>
                </details>
                <Steps platform="Windows" hostIp={hostIp} />
                <details className="disclosure">
                  <summary>iOS and Android</summary>
                  <p className="muted t-label">
                    Edit the active Wi-Fi network, choose manual or static DNS, and use <code>{hostIp}</code> as the
                    first DNS server.
                  </p>
                </details>
              </div>
            </section>

            <section className="stack-sm" aria-labelledby="example-heading">
              <h3 className="t-label" id="example-heading">
                Try it
              </h3>
              <p className="muted t-label">After setup, open an application by hostname:</p>
              <pre>
                <code>http://blog.{suffix}</code>
              </pre>
            </section>
          </>
        ) : (
          <p className="muted t-label" role="status">
            {settings === undefined ? "Loading DNS settings…" : "DNS settings unavailable. Reload the page to try again."}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
