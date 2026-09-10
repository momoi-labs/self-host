import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  Card,
  CardContent,
  CardHeader,
  KV,
  KVKey,
  KVValue,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Toasts,
} from "@momoi-labs/kiso-react";

import { Shell } from "./components/Shell.js";
import { getJson, requireKey } from "./lib/api.js";
import { usePlatform } from "./lib/usePlatform.js";
import { DeviceSetup } from "./device-setup.js";
import "./console.css";

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

function Setup() {
  const [settings, setSettings] = useState<{
    dns_suffix?: string | null;
    host_ip?: string | null;
    host_addresses?: string[] | null;
  } | null>();
  const suffix = settings?.dns_suffix;
  const hostIp = settings?.host_ip;
  const hostAddresses = settings?.host_addresses ?? [];

  useEffect(() => {
    if (!requireKey()) return;
    void (async () => {
      setSettings(
        await getJson<{
          dns_suffix?: string | null;
          host_ip?: string | null;
          host_addresses?: string[] | null;
        }>("/bootstrap/status"),
      );
    })();
  }, []);

  return (
    <section className="page">
      <PageHeader>
          <PageHeaderTitle>DNS setup</PageHeaderTitle>
          <PageHeaderDescription>
            Configure your network to reach applications by hostname.
          </PageHeaderDescription>
      </PageHeader>

      <Card aria-labelledby="host-heading">
        <CardHeader>
          <h2 className="t-caps" id="host-heading">
            Your host
          </h2>
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

      {hostIp && suffix ? (
        <>
          <section className="stack-sm" aria-labelledby="instructions-heading">
            <h2 className="t-h3" id="instructions-heading">
              Instructions
            </h2>
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

          <Card aria-labelledby="example-heading">
            <CardHeader>
              <h2 className="t-h3" id="example-heading">
                Try it
              </h2>
            </CardHeader>
            <CardContent>
              <p className="muted t-label">After setup, open an application by hostname:</p>
              <pre>
                <code>http://blog.{suffix}</code>
              </pre>
            </CardContent>
          </Card>
        </>
      ) : (
        <p className="muted t-label" role="status">
          {settings === undefined ? "Loading DNS settings…" : "DNS settings unavailable. Reload the page to try again."}
        </p>
      )}
    </section>
  );
}

function SettingsShell({ crumb, children }: { crumb: string; children: React.ReactNode }) {
  const { apps, dnsSuffix, healthy } = usePlatform();
  return (
    <Shell
      crumb={crumb}
      dnsSuffix={dnsSuffix}
      apps={apps}
      healthy={healthy}
      overview={{ href: "/console/", active: false }}
      deploy={{ href: "/console/#new", active: false }}
      application={(app) => ({ href: `/console/#app-${app.id}`, active: false })}
    >
      {children}
    </Shell>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {window.location.protocol === "http:" && window.location.pathname === "/setup" ? (
      <DeviceSetup />
    ) : (
      <Toasts>
        <SettingsShell crumb="DNS setup">
          <Setup />
        </SettingsShell>
      </Toasts>
    )}
  </StrictMode>,
);
