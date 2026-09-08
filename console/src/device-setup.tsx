import { useEffect, useRef, useState } from "react";
import { Button, Card, CardContent, PageHeader, PageHeaderDescription, PageHeaderTitle } from "@momoi-labs/kiso-react";
import "./console.css";

type SetupInfo = { admin_hostname: string; dns_suffix: string; fingerprint: string };
type Platform = "" | "macos" | "linux";

function quote(value: string): string {
  return `'${value.replaceAll("'", "'\"'\"'")}'`;
}

function Command({ label, text }: { label: string; text: string }) {
  const code = useRef<HTMLElement>(null);
  const [message, setMessage] = useState("");
  useEffect(() => setMessage(""), [text]);

  function selectAndCopy() {
    if (!code.current) return;
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(code.current);
    selection?.removeAllRanges();
    selection?.addRange(range);
    try {
      if (document.execCommand("copy")) {
        setMessage("Copied");
        return;
      }
    } catch {
      // Keep the command selected when the browser cannot copy over HTTP.
    }
    setMessage("Text selected. Use your keyboard to copy it.");
  }

  async function copy() {
    if (window.isSecureContext && navigator.clipboard) {
      try {
        await navigator.clipboard.writeText(text);
        setMessage("Copied");
        return;
      } catch {
        // Plain HTTP needs the selection-based fallback below.
      }
    }
    selectAndCopy();
  }

  return (
    <div className="stack-sm">
      <div className="between">
        <span className="t-label">{label}</span>
        <Button size="sm" onClick={() => void copy()} aria-label={`Copy ${label}`}>Copy</Button>
      </div>
      <pre className="setup-command"><code ref={code}>{text}</code></pre>
      <span className="muted t-metadata" role="status">{message}</span>
    </div>
  );
}

export function DeviceSetup() {
  const [info, setInfo] = useState<SetupInfo | null>(null);
  const [error, setError] = useState(false);
  const [platform, setPlatform] = useState<Platform>("");
  const ip = window.location.hostname.replace(/^\[|\]$/g, "");

  useEffect(() => {
    document.title = "Connect this device | self-host";
    void fetch("/setup/info", { cache: "no-store", credentials: "omit" })
      .then(async (response) => {
        if (!response.ok) throw new Error("Setup unavailable");
        setInfo(await response.json() as SetupInfo);
      })
      .catch(() => setError(true));
  }, []);

  const dnsCommand = info && platform === "macos"
    ? `sudo mkdir -p /etc/resolver\nprintf 'nameserver %s\\n' ${quote(ip)} | sudo tee ${quote(`/etc/resolver/${info.dns_suffix}`)}\nsudo dscacheutil -flushcache`
    : info && platform === "linux"
      ? `dns_interface=$(ip -o route get ${quote(ip)} | awk '\n  { for (i=1; i<NF; i++) if ($i == "dev") { print $(i+1); exit } }\n')\n\nif [ -n "$dns_interface" ]; then\n  sudo resolvectl dns "$dns_interface" ${quote(ip)} &&\n  sudo resolvectl domain "$dns_interface" ${quote(`~${info.dns_suffix}`)}\nelse\n  echo "Could not find a network interface to reach the Host." >&2\nfi`
      : "";

  return (
    <main className="device-setup stack">
      <PageHeader>
        <p className="t-caps muted">self-host</p>
        <PageHeaderTitle>Connect this device</PageHeaderTitle>
        <PageHeaderDescription>
          Run these commands on the computer where you want to open Applications.
        </PageHeaderDescription>
      </PageHeader>

      {error ? <p role="alert">Setup is unavailable. Check that the Host has been initialized, then reload this page.</p> : !info ? (
        <p role="status">Loading setup instructions…</p>
      ) : (
        <>
          <p className="muted t-label">Host: <code>{ip}</code> · DNS suffix: <code>{info.dns_suffix}</code></p>
          <Card><CardContent>
            <div className="stack">
              <h2 className="t-h3">1. Configure DNS</h2>
              <label className="stack-sm">
                <span>Operating system on this device</span>
                <select className="input" value={platform} onChange={(event) => setPlatform(event.target.value as Platform)}>
                  <option value="">Choose your operating system</option>
                  <option value="macos">macOS</option>
                  <option value="linux">Linux with systemd-resolved</option>
                </select>
              </label>
              {dnsCommand ? <Command label="DNS configuration" text={dnsCommand} /> : null}
              {platform === "linux" ? <p className="muted t-label">These DNS settings may reset after a reconnect or reboot. Save them in your network settings for permanent use.</p> : null}
            </div>
          </CardContent></Card>

          <Card><CardContent>
            <div className="stack">
              <h2 className="t-h3">2. Trust the certificate</h2>
              <p>Confirm this SHA-256 fingerprint with the Operator through the Host's terminal or SSH before running the trust command. HTTP alone cannot verify it.</p>
              <pre className="setup-command"><code>{info.fingerprint}</code></pre>
              <details className="disclosure">
                <summary>Find the fingerprint on the Host</summary>
                <Command label="Fingerprint check on the Host" text={'openssl x509 -in "$HOME/.config/self-host/certs/ca.pem" -noout -fingerprint -sha256'} />
              </details>
              {platform ? (
                <>
                  <details className="disclosure">
                    <summary>Install the CLI on this device if needed</summary>
                    <p className="muted t-label">This installs only the CLI. It does not prepare this computer as a Host.</p>
                    <Command label="CLI installation" text="curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/install.sh | SELF_HOST_BINARY_ONLY=1 bash" />
                  </details>
                  {platform === "macos" ? <p>Run this in Terminal in the Mac's graphical session and approve the system prompt. An SSH session cannot complete this step.</p> : null}
                  <Command label="Certificate trust" text={`self-host trust-ca --from ${quote(info.admin_hostname)} --fingerprint ${quote(info.fingerprint)}`} />
                  <p className="muted t-label">Fully restart your browsers after installing the CA.</p>
                </>
              ) : <p className="muted t-label">Choose your operating system above to see the commands.</p>}
              <p><a href="/setup/ca.pem" download="self-host-ca.pem">Download the public CA</a> if you prefer to install it manually.</p>
            </div>
          </CardContent></Card>

          <Card><CardContent>
            <div className="stack-sm">
              <h2 className="t-h3">3. Open the console</h2>
              <p>After DNS and certificate trust are configured, open:</p>
              <a href={`https://${info.admin_hostname}`}>{`https://${info.admin_hostname}`}</a>
            </div>
          </CardContent></Card>
        </>
      )}
    </main>
  );
}
