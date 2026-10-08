import { StrictMode, useRef, useState, type FormEvent } from "react";
import { createRoot } from "react-dom/client";
import {
  Alert,
  AlertDescription,
  BrandMark,
  Button,
  Card,
  CardContent,
  FormField,
  Spinner,
  TerminalIcon,
  ThemeSelector,
} from "@momoi-labs/kiso-react";

import { Icon } from "./components/Icon.js";
import { checkApiKey, type LoginFailure } from "./lib/login.js";
import { useTheme } from "./lib/theme.js";
import "./console.css";

function Brand() {
  return (
    <div className="brand">
      <BrandMark><TerminalIcon /></BrandMark>
      <div className="auth-brand-copy">
        <span className="t-h3">self-host</span>
        <span className="auth-server-url t-metadata muted">{window.location.origin}</span>
      </div>
    </div>
  );
}

function KeyHelp() {
  return (
    <div className="auth-key-help stack-sm">
      <p>On the Host, run <code>self-host init --show-key</code> to print the initial API key.</p>
      <p className="muted">If this Host has not been initialized, run <code>self-host init</code> first.</p>
      <p className="t-metadata muted">The API key gives access to the operator console. Keep it private.</p>
    </div>
  );
}

function Login() {
  const [theme, setTheme] = useTheme();
  const [key, setKey] = useState("");
  const [visible, setVisible] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<LoginFailure | null>(null);
  const field = useRef<HTMLInputElement>(null);
  const submitting = useRef(false);

  async function unlock(event: FormEvent) {
    event.preventDefault();
    if (submitting.current) return;
    setError(null);
    if (!key.trim()) {
      setError({ kind: "key", message: "Paste your API key to continue." });
      field.current?.focus();
      return;
    }
    submitting.current = true;
    setPending(true);
    try {
      const result = await checkApiKey(key);
      if (result.kind !== "accepted") {
        setError(result);
        if (result.kind === "key") field.current?.focus();
        return;
      }
      sessionStorage.setItem("api_key", result.key);
      window.location.replace("/console/");
    } catch {
      setError({ kind: "host", message: "Your browser could not open the console in this tab. Try again." });
    } finally {
      submitting.current = false;
      setPending(false);
    }
  }

  return (
    <main className="auth-shell" data-background-style="momoi" data-background-strength="quiet">
      <aside className="auth-guide stack" aria-labelledby="guide-title">
        <Brand />
        <div className="stack-sm">
          <p className="auth-eyebrow muted">Operator access</p>
          <h2 id="guide-title" className="auth-display">Your Host.<br />Your console.</h2>
          <p className="muted">Manage Applications, Virtual machines and LAN settings.</p>
        </div>
        <div className="auth-guide-instructions stack-sm">
          <h3 className="t-h3">Get your API key</h3>
          <KeyHelp />
        </div>
      </aside>
      <section className="auth-access" aria-labelledby="login-title">
        <div className="auth-panel stack">
          <div className="auth-mobile-brand"><Brand /></div>
          <h1 id="login-title" className="auth-announcement">Unlock the console</h1>
          <Card>
            <CardContent>
              <form className="stack" onSubmit={unlock} noValidate aria-busy={pending}>
                <FormField
                  ref={field} label="API key" id="key" name="api_key" className="auth-key"
                  type={visible ? "text" : "password"} placeholder="Paste your API key"
                  autoComplete="current-password" autoCapitalize="none" autoCorrect="off"
                  spellCheck={false} required autoFocus readOnly={pending} value={key}
                  hint="Paste the key for this Host. You can reveal it to check the value."
                  error={error?.kind === "key" ? error.message : undefined}
                  suffix={
                    <Button type="button" size="sm" variant="ghost" aria-controls="key"
                      aria-label={visible ? "Hide API key" : "Show API key"} aria-pressed={visible}
                      onClick={() => setVisible((current) => !current)}>
                      {visible ? "Hide" : "Show"}
                    </Button>
                  }
                  onChange={(event) => { setKey(event.target.value); setError(null); }}
                />
                {error?.kind === "host" ? (
                  <Alert variant="error">
                    <Icon name="alert" size="md" />
                    <AlertDescription>{error.message}</AlertDescription>
                  </Alert>
                ) : null}
                <Button type="submit" variant="primary" size="lg" className="btn-block" disabled={pending}>
                  {pending ? <Spinner size="sm" aria-hidden="true" /> : null}
                  {pending ? "Checking key..." : "Unlock console"}
                </Button>
                <details className="auth-mobile-help">
                  <summary>Where do I find my API key?</summary>
                  <KeyHelp />
                </details>
              </form>
            </CardContent>
          </Card>
          <p className="auth-announcement" role="status">{pending ? "Checking your API key." : ""}</p>
          <p className="auth-announcement" role="alert">{error?.kind === "key" ? error.message : ""}</p>
          <div className="auth-preferences">
            <ThemeSelector theme={theme} onChange={setTheme} />
            <span className="t-metadata muted mono">v{import.meta.env.SELF_HOST_VERSION}</span>
          </div>
        </div>
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<StrictMode><Login /></StrictMode>);
