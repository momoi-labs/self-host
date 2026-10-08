// Throwaway: compare three login hierarchies on login.html?variant=A|B|C.
// All submissions are simulated. No credentials leave this component.
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import {
  Alert, AlertDescription, BrandMark, Button, Card, CardContent, FormField,
  Spinner, TerminalIcon, ThemeSelector,
} from "@momoi-labs/kiso-react";
import { applyTheme } from "./lib/theme.js";
import "./login-prototype.css";

const variants = ["A", "B", "C"] as const;
type Variant = typeof variants[number];
type Outcome = "accepted" | "rejected" | "offline" | "unavailable";
type State = "idle" | "empty" | "checking" | Outcome;
const names: Record<Variant, string> = {
  A: "Focused form", B: "Help beside the form", C: "Open workspace",
};
const notes: Record<Variant, string> = {
  A: "One clear action; detailed help opens when needed.",
  B: "Preferred direction. Full-height help panel on desktop; focused form on mobile.",
  C: "No card. More context above the form, with instructions directly below it.",
};

function Brand({ serverUrl }: { serverUrl?: string }) {
  return <div className="brand">
    <BrandMark><TerminalIcon /></BrandMark>
    <div className="lp-brand-copy">
      <span className="t-h3">self-host</span>
      {serverUrl ? <span className="lp-server-url t-metadata muted">{serverUrl}</span> : null}
    </div>
  </div>;
}

function KeyHelp() {
  return <div className="stack-sm lp-key-help">
    <p>On the Host, run <code>self-host init --show-key</code> to print the initial API key.</p>
    <p className="muted">If this Host has not been initialized, run <code>self-host init</code> first.</p>
    <p className="t-metadata muted">The API key gives access to the operator console. Keep it private.</p>
  </div>;
}

function HelpDisclosure() {
  return <details className="lp-help">
    <summary>Where do I find my API key?</summary>
    <KeyHelp />
  </details>;
}

function AccessForm({ outcome, help, inline = false }: { outcome: Outcome; help?: ReactNode; inline?: boolean }) {
  const [key, setKey] = useState("");
  const [visible, setVisible] = useState(false);
  const [state, setState] = useState<State>("idle");
  const input = useRef<HTMLInputElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (timer.current !== null) clearTimeout(timer.current); }, []);

  function submit(event: FormEvent) {
    event.preventDefault();
    if (timer.current !== null) return;
    if (!key.trim()) {
      setState("empty");
      input.current?.focus();
      return;
    }
    setState("checking");
    timer.current = setTimeout(() => {
      timer.current = null;
      setState(outcome);
      if (outcome === "rejected") input.current?.focus();
    }, 1400);
  }

  const fieldError = state === "empty" ? "Paste your API key to continue."
    : state === "rejected" ? "This key was not accepted. Check that it belongs to this Host and try again."
    : undefined;

  return <form className={`lp-form stack${inline ? " lp-form-inline" : ""}`} onSubmit={submit} noValidate aria-busy={state === "checking"}>
    <FormField
      ref={input} id="prototype-key" label="API key"
      type={visible ? "text" : "password"} value={key}
      placeholder="Paste your API key" autoComplete="off" autoCapitalize="none"
      autoCorrect="off" spellCheck={false} required readOnly={state === "checking"}
      hint="Paste the key for this Host. You can reveal it to check the value."
      error={fieldError}
      suffix={<Button type="button" size="sm" variant="ghost"
        aria-label={visible ? "Hide API key" : "Show API key"}
        aria-controls="prototype-key" aria-pressed={visible}
        onClick={() => setVisible(!visible)}>{visible ? "Hide" : "Show"}</Button>}
      onChange={(event) => { setKey(event.target.value); setState("idle"); }}
    />
    {state === "offline" || state === "unavailable" ? <Alert variant="error">
      <AlertDescription>{state === "offline"
        ? "Could not reach the Host. Check your LAN connection and try again."
        : "The Host could not check your key. Wait a moment and try again."}</AlertDescription>
    </Alert> : null}
    {state === "accepted" ? <Alert variant="success">
      <AlertDescription>Key accepted. In the real console, your dashboard would open now.</AlertDescription>
    </Alert> : null}
    <Button type="submit" variant="primary" size="lg" className="btn-block"
      disabled={state === "checking"}>
      {state === "checking" ? <Spinner size="sm" aria-hidden="true" /> : null}
      {state === "checking" ? "Checking key..." : "Unlock console"}
    </Button>
    <span role="status" className="lp-sr-only">
      {state === "checking" ? "Checking your API key." : ""}
    </span>
    <span role="alert" className="lp-sr-only">{fieldError || ""}</span>
    {help}
    <details className="lp-state t-metadata muted">
      <summary>Prototype state: {state}</summary>
      <code>state={state}; value={key.trim() ? "present" : "empty"}; visibility={visible ? "visible" : "masked"}; outcome={outcome}</code>
    </details>
  </form>;
}

export function VariantA({ form }: { form: ReactNode }) {
  return <main className="lp-centered">
    <section className="lp-focused stack" aria-labelledby="access-title">
      <Brand />
      <div className="stack-xs">
        <h1 id="access-title" className="t-h2">Unlock the console</h1>
        <p className="muted">Manage this Host with your API key.</p>
      </div>
      <Card><CardContent>{form}</CardContent></Card>
      <p className="t-metadata muted">Operator access to <code>admin.home.lan</code></p>
    </section>
  </main>;
}

export function VariantB({ form }: { form: ReactNode }) {
  const serverUrl = window.location.origin;
  return <main className="lp-split" data-background-style="momoi" data-background-strength="quiet">
    <aside className="lp-guide stack" aria-labelledby="guide-title">
      <Brand serverUrl={serverUrl} />
      <div className="stack-sm">
        <p className="lp-eyebrow muted">Operator access</p>
        <h2 id="guide-title" className="lp-display">Your Host.<br />Your console.</h2>
        <p className="muted">Manage Applications, Virtual machines and LAN settings.</p>
      </div>
      <div className="lp-guide-instructions stack-sm">
        <h3 className="t-h3">Get your API key</h3>
        <KeyHelp />
      </div>
    </aside>
    <section className="lp-split-form" aria-labelledby="access-title">
      <div className="lp-access-content stack">
        <div className="lp-mobile-brand"><Brand serverUrl={serverUrl} /></div>
        <h1 id="access-title" className="lp-sr-only">Unlock the console</h1>
        <Card><CardContent>{form}</CardContent></Card>
      </div>
    </section>
  </main>;
}

export function VariantC({ form }: { form: ReactNode }) {
  return <main className="lp-open">
    <header className="lp-open-header"><Brand /><span className="muted t-metadata">Operator console</span></header>
    <section className="lp-open-content" aria-labelledby="access-title">
      <div className="lp-open-heading stack-sm">
        <p className="lp-eyebrow muted">Host: admin.home.lan</p>
        <h1 id="access-title" className="lp-display">Connect to your Host.</h1>
        <p className="muted">Unlock the console to manage your LAN.</p>
      </div>
      <div className="lp-open-form stack">{form}
        <section className="lp-open-help stack-sm" aria-labelledby="key-help-title">
          <h2 id="key-help-title" className="t-h3">Need your API key?</h2>
          <KeyHelp />
        </section>
      </div>
    </section>
  </main>;
}

function PrototypeSwitcher({ variant, choose, outcome, setOutcome, theme, setTheme }: {
  variant: Variant; choose: (next: Variant) => void; outcome: Outcome;
  setOutcome: (next: Outcome) => void; theme: string; setTheme: (next: string) => void;
}) {
  function cycle(direction: number) {
    choose(variants[(variants.indexOf(variant) + direction + variants.length) % variants.length]);
  }
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.target instanceof Element && event.target.closest("input, textarea, select, [contenteditable], [role=radio], [role=combobox]")) return;
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        cycle(event.key === "ArrowLeft" ? -1 : 1);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  return <aside className="lp-switcher" aria-label="Prototype controls">
    <div className="lp-switch-row">
      <Button aria-label="Previous variant" onClick={() => cycle(-1)}>&larr;</Button>
      <div className="lp-variant-label"><strong>{variant}: {names[variant]}</strong><span className="t-metadata muted">{notes[variant]}</span></div>
      <Button aria-label="Next variant" onClick={() => cycle(1)}>&rarr;</Button>
    </div>
    <div className="lp-scenarios">
      <label className="t-label" htmlFor="prototype-outcome">On submit</label>
      <select id="prototype-outcome" className="select" value={outcome}
        onChange={(event) => setOutcome(event.target.value as Outcome)}>
        <option value="accepted">Key accepted</option>
        <option value="rejected">Key rejected</option>
        <option value="offline">Host offline</option>
        <option value="unavailable">Host service failure</option>
      </select>
      <ThemeSelector theme={theme} onChange={setTheme} />
    </div>
    <p className="t-metadata muted">Variant {variant} / simulated {outcome}. Switching resets the form. Use any demo text.</p>
  </aside>;
}

export function LoginPrototype() {
  const query = new URLSearchParams(window.location.search).get("variant");
  const [variant, setVariant] = useState<Variant>(variants.includes(query as Variant) ? query as Variant : "A");
  const [outcome, setOutcome] = useState<Outcome>("accepted");
  const [theme, setTheme] = useState(document.documentElement.dataset.theme || "system");
  function choose(next: Variant) {
    const url = new URL(window.location.href);
    url.searchParams.set("variant", next);
    window.history.replaceState(null, "", url);
    setVariant(next);
  }
  const form = <AccessForm key={`${variant}:${outcome}`} outcome={outcome} inline={variant === "C"}
    help={variant === "A" ? <HelpDisclosure /> : variant === "B"
      ? <div className="lp-mobile-help"><HelpDisclosure /></div> : undefined} />;
  return <div className="lp-prototype" data-variant={variant}>
    <div className="lp-banner t-metadata">Login prototype. Sample Host, simulated requests. Use demo text.</div>
    {variant === "A" ? <VariantA form={form} /> : variant === "B" ? <VariantB form={form} /> : <VariantC form={form} />}
    <PrototypeSwitcher variant={variant} choose={choose} outcome={outcome} setOutcome={setOutcome}
      theme={theme} setTheme={(next) => { applyTheme(next); setTheme(next); }} />
  </div>;
}
