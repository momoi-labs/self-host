import { StrictMode, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertTitle,
  Button,
  Card,
  CardContent,
  FormField,
  PasswordInput,
  Spinner,
  ThemeSelector,
} from "@momoi-labs/kiso-react";

import { Icon } from "./components/Icon.js";
import { Lockup } from "./components/Logo.js";
import { accountStatus, ceremonyMessage, createKey, logIn, setUpPasskey, type AccountStatus } from "./lib/passkey.js";
import { useTheme } from "./lib/theme.js";
import "./console.css";

type Step = "loading" | "elsewhere" | "setup" | "login" | "cli" | "unreachable";

function Brand() {
  return (
    <div className="auth-brand-copy">
      <Lockup />
      <span className="auth-server-url t-metadata muted">{window.location.origin}</span>
    </div>
  );
}

function SetupHelp() {
  return (
    <div className="auth-key-help stack-sm">
      <p>On a new Host, run <code>self-host init --show-key</code> to print the Platform key. It works once, for the first passkey.</p>
      <p>Lost every passkey? On the Host, run <code>self-host account recover</code> for a setup key.</p>
      <p className="t-metadata muted">Use a synced passkey, from your password manager or phone, so a lost device does not lock you out.</p>
    </div>
  );
}

function Failure({ message }: { message: string | null }) {
  return message ? (
    <Alert variant="error">
      <Icon name="alert" size="md" />
      <AlertDescription>{message}</AlertDescription>
    </Alert>
  ) : null;
}

function SetupForm({ onDone }: { onDone: () => void }) {
  const [key, setKey] = useState("");
  const [name, setName] = useState("");
  const [pending, setPending] = useState(false);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLInputElement>(null);

  async function register(event: FormEvent) {
    event.preventDefault();
    if (pending) return;
    setError(null);
    setKeyError(null);
    if (!key.trim()) {
      setKeyError("Paste the Platform key or a setup key to continue.");
      field.current?.focus();
      return;
    }
    setPending(true);
    try {
      await setUpPasskey(key.trim(), name.trim() || "Passkey");
      onDone();
    } catch (cause) {
      setError(ceremonyMessage(cause));
    } finally {
      setPending(false);
    }
  }

  return (
    <form className="stack" onSubmit={register} noValidate aria-busy={pending}>
      <FormField label="Platform key or setup key" id="key" hint="This key only registers the passkey. The console never keeps it." error={keyError ?? undefined}>
        <PasswordInput
          ref={field} id="key" name="setup_key" placeholder="Paste the key"
          required autoFocus readOnly={pending} value={key}
          labels={{ showName: "Show key", hideName: "Hide key", shown: "The key is visible", hidden: "The key is hidden" }}
          onChange={(event) => { setKey(event.target.value); setKeyError(null); }}
        />
      </FormField>
      <FormField label="Passkey name" id="passkey-name" placeholder="iCloud Keychain" hint="Name it after where it lives." value={name} readOnly={pending} onChange={(event) => setName(event.target.value)} />
      <Failure message={error} />
      <Button type="submit" variant="primary" size="lg" className="btn-block" disabled={pending}>
        {pending ? <Spinner size="sm" aria-hidden="true" /> : <Icon name="key" />}
        {pending ? "Waiting for your passkey..." : "Create a passkey"}
      </Button>
      <p className="auth-announcement" role="status">{pending ? "Waiting for your passkey." : ""}</p>
    </form>
  );
}

function LoginForm({ onLoggedIn, onSetup, registered }: { onLoggedIn: (retired: boolean) => void; onSetup: () => void; registered: boolean }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function login() {
    if (pending) return;
    setError(null);
    setPending(true);
    try {
      const result = await logIn();
      onLoggedIn(result.retired_root_key);
    } catch (cause) {
      setError(ceremonyMessage(cause));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="stack" aria-busy={pending}>
      {registered ? (
        <Alert variant="success">
          <Icon name="check" size="md" />
          <AlertDescription>Your passkey is ready. Log in with it now.</AlertDescription>
        </Alert>
      ) : null}
      <Failure message={error} />
      <Button variant="primary" size="lg" className="btn-block" autoFocus disabled={pending} onClick={() => void login()}>
        {pending ? <Spinner size="sm" aria-hidden="true" /> : <Icon name="key" />}
        {pending ? "Waiting for your passkey..." : "Log in with a passkey"}
      </Button>
      <Button variant="ghost" className="btn-block" disabled={pending} onClick={onSetup}>Use a setup key</Button>
      <p className="auth-announcement" role="status">{pending ? "Waiting for your passkey." : ""}</p>
    </div>
  );
}

function CliKey() {
  const [key, setKey] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function create() {
    setError(null);
    setPending(true);
    try {
      const label = `CLI ${new Date().toISOString().slice(0, 10)}`;
      const created = (await createKey({ label, kind: "human", expires_in_days: "never" }, true)) as { key: string };
      setKey(created.key);
    } catch (cause) {
      setError(ceremonyMessage(cause));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="stack">
      <p>The Platform key is retired, so the CLI on the Host needs a Person key of its own.</p>
      {key ? (
        <Alert variant="info">
          <Icon name="info" size="md" />
          <AlertContent>
            <AlertTitle>Copy this key now. It will not be shown again.</AlertTitle>
            <AlertDescription className="mono">{key}</AlertDescription>
            <AlertDescription>On the Host, run <code>self-host account use-key</code> and paste it.</AlertDescription>
          </AlertContent>
        </Alert>
      ) : (
        <Button variant="primary" size="lg" className="btn-block" disabled={pending} onClick={() => void create()}>
          {pending ? <Spinner size="sm" aria-hidden="true" /> : <Icon name="key" />}
          {pending ? "Waiting for your passkey..." : "Create a CLI key"}
        </Button>
      )}
      <Failure message={error} />
      <Button variant={key ? "primary" : "ghost"} className="btn-block" onClick={() => window.location.replace("/console/")}>
        {key ? "Continue to the console" : "Skip for now"}
      </Button>
    </div>
  );
}

const titles: Record<Step, string> = {
  loading: "Opening the console",
  elsewhere: "Open the console at its own address",
  setup: "Create your passkey",
  login: "Log in to the console",
  cli: "Give the CLI its own key",
  unreachable: "The Host did not answer",
};

function Login() {
  const [theme, setTheme] = useTheme();
  const [status, setStatus] = useState<AccountStatus | null>(null);
  const [step, setStep] = useState<Step>("loading");
  const [registered, setRegistered] = useState(false);

  useEffect(() => {
    void (async () => {
      try {
        const who = await fetch("/whoami");
        if (who.ok) return window.location.replace("/console/");
        const current = await accountStatus();
        setStatus(current);
        setStep(!current.canonical ? "elsewhere" : current.usable ? "login" : "setup");
      } catch {
        setStep("unreachable");
      }
    })();
  }, []);

  let body: ReactNode;
  if (step === "loading") body = <Spinner size="md" aria-label="Loading" />;
  else if (step === "unreachable") body = <Failure message="Could not reach the Host. Check your connection and reload the page." />;
  else if (step === "elsewhere") body = (
    <div className="stack">
      <p>Passkeys work only at the console's own address. This address cannot log you in.</p>
      <Button asChild variant="primary" size="lg" className="btn-block">
        <a href={status?.console_url}>Open {status?.console_url}</a>
      </Button>
    </div>
  );
  else if (step === "setup") body = <SetupForm onDone={() => { setRegistered(true); setStep("login"); }} />;
  else if (step === "login") body = (
    <LoginForm
      registered={registered}
      onSetup={() => setStep("setup")}
      onLoggedIn={(retired) => (retired ? setStep("cli") : window.location.replace("/console/"))}
    />
  );
  else body = <CliKey />;

  return (
    <main className="auth-shell">
      <aside className="auth-guide stack" aria-labelledby="guide-title">
        <Brand />
        <div className="stack-sm">
          <p className="auth-eyebrow muted">Operator access</p>
          <h2 id="guide-title" className="auth-display">Your Host.<br />Your console.</h2>
          <p className="muted">Manage Applications, Virtual machines and LAN settings.</p>
        </div>
        <div className="auth-guide-instructions stack-sm">
          <h3 className="t-h3">Passkeys and setup keys</h3>
          <SetupHelp />
        </div>
      </aside>
      <section className="auth-access" aria-labelledby="login-title">
        <div className="auth-panel stack">
          <div className="auth-mobile-brand"><Brand /></div>
          <h1 id="login-title" className="t-h2">{titles[step]}</h1>
          <Card>
            <CardContent>{body}</CardContent>
          </Card>
          <details className="auth-mobile-help">
            <summary>Where do I get a key?</summary>
            <SetupHelp />
          </details>
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
