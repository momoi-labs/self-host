import { StrictMode, useRef, useState, type FormEvent } from "react";
import { createRoot } from "react-dom/client";
import {
  Alert,
  AlertDescription,
  BrandMark,
  Button,
  CardContent,
  FormField,
  TerminalIcon,
  ThemeSelector,
} from "@momoi-labs/kiso-react";

import { Icon } from "./components/Icon.js";
import { useTheme } from "./lib/theme.js";
import "./console.css";

function Login() {
  const [theme, setTheme] = useTheme();
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLInputElement>(null);

  async function unlock(event: FormEvent) {
    event.preventDefault();
    setError(null);
    try {
      const response = await fetch("/health", {
        headers: { Authorization: "Bearer " + key.trim() },
      });
      if (!response.ok) throw new Error("That API key was not accepted. Check it and try again.");
      sessionStorage.setItem("api_key", key.trim());
      window.location.replace("/console/");
    } catch (cause) {
      setError(
        (cause as Error).message ||
          "Could not reach the host. Check the connection and try again.",
      );
      field.current?.focus();
    }
  }

  return (
    <main className="auth-shell">
      <section className="auth-panel stack" aria-labelledby="login-title">
        <div className="between">
          <div className="brand">
            <BrandMark>
              <TerminalIcon />
            </BrandMark>
            <div>
              <h1 className="t-h2" id="login-title">
                self-host
              </h1>
              <p className="muted t-label">Unlock the operator console</p>
            </div>
          </div>
        </div>
        <form className="card" onSubmit={unlock}>
          <CardContent>
            <FormField
              ref={field}
              label="API key"
              id="key"
              className="mono"
              type="password"
              placeholder="sk-…"
              autoComplete="off"
              required
              autoFocus
              value={key}
              onChange={(event) => setKey(event.target.value)}
            />
            {error ? (
              <Alert variant="error">
                <Icon name="alert" size="md" />
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            ) : null}
            <Button type="submit" variant="primary" className="btn-block">
              Unlock
            </Button>
            <p className="t-metadata muted">
              Run <code>self-host serve</code> to get your API key.
            </p>
          </CardContent>
        </form>
        <ThemeSelector theme={theme} onChange={setTheme} />
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Login />
  </StrictMode>,
);
