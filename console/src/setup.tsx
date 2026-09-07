import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  Button,
  Card,
  CardContent,
  CardHeader,
  KV,
  KVKey,
  KVValue,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  ThemeSelector,
} from "@momoi-labs/kiso-react";

import { getJson, requireKey } from "./lib/api.js";
import { useTheme } from "./lib/theme.js";
import "./console.css";

/* Only one of these is ever relevant to the reader, so they stack as
   disclosures rather than as cards. */
const steps: Record<string, string[]> = {
  macOS: [
    "Open System Settings → Network.",
    "Open the active connection and select DNS.",
    "Add the Host IP as a DNS server, then apply the change.",
  ],
  Windows: [
    "Open Network Connections and the active adapter's properties.",
    "Open Internet Protocol Version 4 (TCP/IPv4).",
    "Use the Host IP as the preferred DNS server and save.",
  ],
};

function Steps({ platform }: { platform: keyof typeof steps }) {
  return (
    <details className="disclosure">
      <summary>{platform}</summary>
      <ol>
        {steps[platform].map((step) => (
          <li key={step}>{step}</li>
        ))}
      </ol>
    </details>
  );
}

function Setup() {
  const [theme, setTheme] = useTheme();
  const [suffix, setSuffix] = useState("home.lan");

  useEffect(() => {
    if (!requireKey()) return;
    void (async () => {
      const status = await getJson<{ dns_suffix?: string }>("/bootstrap/status");
      if (status?.dns_suffix) setSuffix(status.dns_suffix);
    })();
  }, []);

  return (
    <main className="page">
      <div className="between">
        <PageHeader>
          <PageHeaderTitle>DNS setup</PageHeaderTitle>
          <PageHeaderDescription>
            Configure your network to reach applications by hostname.
          </PageHeaderDescription>
        </PageHeader>
        <div className="row">
          <ThemeSelector theme={theme} onChange={setTheme} />
          <Button size="sm" asChild>
            <a href="/console/">Back to console</a>
          </Button>
        </div>
      </div>

      <Card aria-labelledby="host-heading">
        <CardHeader>
          <h2 className="t-caps" id="host-heading">
            Your host
          </h2>
        </CardHeader>
        <CardContent>
          <KV>
            <KVKey>Host IP</KVKey>
            <KVValue>{window.location.hostname}</KVValue>
            <KVKey>DNS suffix</KVKey>
            <KVValue>{suffix}</KVValue>
          </KV>
        </CardContent>
      </Card>

      <section className="stack-sm" aria-labelledby="instructions-heading">
        <h2 className="t-h3" id="instructions-heading">
          Instructions
        </h2>
        <p className="muted t-label">
          Point your device's DNS resolver to the Host IP so that <code>*.{suffix}</code> resolves
          locally.
        </p>
        <div>
          <Steps platform="macOS" />
          <details className="disclosure">
            <summary>Linux</summary>
            <p className="muted t-label">With systemd-resolved:</p>
            <pre>
              <code>
                {"sudo resolvectl dns INTERFACE HOST_IP\nsudo resolvectl domain INTERFACE ~SUFFIX"}
              </code>
            </pre>
            <p className="t-metadata muted">
              Replace INTERFACE, HOST_IP and SUFFIX with the values for your host.
            </p>
          </details>
          <Steps platform="Windows" />
          <details className="disclosure">
            <summary>iOS and Android</summary>
            <p className="muted t-label">
              Edit the active Wi-Fi network, choose manual or static DNS, and use the Host IP as the
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
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Setup />
  </StrictMode>,
);
