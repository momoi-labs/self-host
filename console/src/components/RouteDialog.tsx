import { useState, type FormEvent } from "react";
import {
  Button, Card, CardContent, Checkbox, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  FormField, Label, RadioGroup, RadioGroupItem, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@momoi-labs/kiso-react";

import { api, asReport, failureOf } from "../lib/api.js";
import {
  isPublished, nameFor, redirectDraft, redirectRule, replaceRoute, routesOf, sameFingerprint, saveRoutes, targetLabel, type Route,
} from "../lib/routes.js";
import type { App } from "../lib/types.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";

const EXTERNAL = "external";

type Probe = { fingerprint: string; subject?: string | null; not_after?: string | null };

const expiryFormat = new Intl.DateTimeFormat("en-GB", { day: "2-digit", month: "short", year: "numeric" });

/**
 * What the dialog opens on: the Application, or null to choose one, and the
 * route or the redirect (by its index) when changing one.
 */
export type RouteEditor = { app: App | null; route?: Route; redirect?: number; action?: "forward" | "redirect" };

/** What a hostname and path do: reach the Application, or answer with a redirect. */
type Action = "forward" | "redirect";

/** The choice of where a hostname lives: under the Zone, or any other domain pointed at the Host. */
const OTHER = "other";

/**
 * One way to add or change a route, from the Routes page or from an
 * Application's Routes tab. A route is a hostname and a path, and what they
 * do: reach the Application, or redirect. Either way it belongs to the
 * Application whose hostnames answer it, and saving it rebuilds nothing.
 */
export function RouteDialog({ editor, apps, dnsSuffix, onClose, onSaved, onPattern }: {
  editor: RouteEditor | null;
  apps: App[];
  dnsSuffix: string;
  onClose: () => void;
  /** Called once the proxy serves the change; says what changed. */
  onSaved: (message: string) => void;
  /** Opens the pattern editor for a redirect a hostname and path cannot say. */
  onPattern?: (app: App) => void;
}) {
  const [saving, setSaving] = useState(false);
  return (
    <Dialog open={editor !== null} onOpenChange={(open) => { if (!open && !saving) onClose(); }}>
      <DialogContent className="route-dialog">
        {editor ? (
          <RouteForm key={editor.route?.key ?? `${editor.app?.id ?? "any"}:${editor.redirect ?? "new"}`} editor={editor} apps={apps} dnsSuffix={dnsSuffix}
            saving={saving} setSaving={setSaving} onClose={onClose} onSaved={onSaved} onPattern={onPattern} />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function RouteForm({ editor, apps, dnsSuffix, saving, setSaving, onClose, onSaved, onPattern }: {
  editor: RouteEditor;
  apps: App[];
  dnsSuffix: string;
  saving: boolean;
  setSaving: (saving: boolean) => void;
  onClose: () => void;
  onSaved: (message: string) => void;
  onPattern?: (app: App) => void;
}) {
  const editing = editor.route;
  const redirecting = editor.app && editor.redirect !== undefined ? editor.app.redirects?.[editor.redirect] : undefined;
  const draft = redirecting ? redirectDraft(redirecting) : null;
  const published = apps.filter(isPublished);
  const externalRoot = editor.app?.runtime?.kind === "external" && editing?.kind === "hostname" ? editor.app : null;
  const applications = published.filter((one) => one.runtime?.kind !== "external" || (one.id === editor.app?.id && !externalRoot));
  const [appId, setAppId] = useState(externalRoot ? EXTERNAL : editor.app?.id ?? applications[0]?.id ?? EXTERNAL);
  const external = appId === EXTERNAL;
  const app = external ? externalRoot ?? undefined : published.find((one) => one.id === appId);
  const saved = externalRoot?.runtime?.kind === "external" ? externalRoot.runtime : null;
  const [upstream, setUpstream] = useState(saved?.upstream ?? "");
  const [pin, setPin] = useState(saved?.pin ?? "");
  const [probe, setProbe] = useState<Probe | null>(null);
  const [probing, setProbing] = useState(false);
  const [probeFailure, setProbeFailure] = useState<unknown>(null);
  const [matched, setMatched] = useState(false);
  const https = upstream.trim().startsWith("https://");
  const verdict = !probe || !pin.trim() ? null : sameFingerprint(pin, probe.fingerprint) ? "match" : "mismatch";
  const trusted = verdict === "match" || matched;
  const zone = `.${dnsSuffix}`;
  const initial = editing?.hostname ?? draft?.hostname ?? "";
  const inZone = initial === "" || initial.endsWith(zone);
  const [where, setWhere] = useState(inZone ? dnsSuffix : OTHER);
  const [label, setLabel] = useState(inZone ? initial.slice(0, initial.length ? -zone.length : 0) : initial);
  const [path, setPath] = useState(editing?.path ?? draft?.path ?? "/");
  const [action, setAction] = useState<Action>(draft ? "redirect" : editor.action ?? "forward");
  // Removing the prefix is the default, as Coolify does; a changed route keeps its choice.
  const [strip, setStrip] = useState(editing ? editing.stripPrefix : true);
  const [location, setLocation] = useState(draft?.location ?? "");
  // Trying a redirect out is the usual start, and browsers keep a 301 for good.
  const [permanent, setPermanent] = useState(draft?.permanent ?? false);
  const [failure, setFailure] = useState<unknown>(null);

  const name = label.trim().toLowerCase();
  const hostname = where === OTHER ? name : `${name}${zone}`;
  const prefix = path.trim() || "/";
  const target = location.trim();
  const routes = published.flatMap(routesOf);
  const outside = [...new Set(routes.map((route) => route.hostname).filter((one) => !one.endsWith(zone)))];
  const labels = [...new Set(routes.map((route) => route.hostname).filter((one) => one.endsWith(zone)).map((one) => one.slice(0, -zone.length)))];
  const clash = action === "forward"
    ? routes.find((route) => route.key !== editing?.key && route.hostname === hostname && route.path === prefix)
    : undefined;
  const redirectClash = action === "redirect"
    ? (app?.redirects ?? []).some((rule, index) => {
      const other = redirectDraft(rule);
      return index !== editor.redirect && other?.hostname === hostname && other.path === prefix;
    })
    : false;
  const labelError = !name
    ? null
    : !/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/.test(name)
      ? "Use letters, digits and hyphens, with a dot between names."
      : where === OTHER && !name.includes(".")
        ? "Type the whole domain, such as app.example.com."
        : hostname === `console${zone}` || hostname === `admin${zone}`
          ? `${hostname} belongs to the Platform.`
          : clash
            ? `${hostname}${prefix === "/" ? "" : prefix} already reaches ${clash.app.name}.`
            : redirectClash
              ? `${hostname}${prefix} already redirects.`
              : null;
  const pathError = !/^\/([A-Za-z0-9._~-]+(\/[A-Za-z0-9._~-]+)*)?$/.test(prefix)
    ? "Start with / and use letters, digits, dots, hyphens or underscores."
    : null;
  const locationError = !target
    ? null
    : /\s/.test(target)
      ? "A location has no spaces."
      : /^(https?:\/\/|\/)/.test(target)
        ? target === prefix ? "That is the path it answers on." : null
        : "Start with / for a path on this hostname, or with https://.";
  const valid = (app !== undefined || (external && upstream.trim() !== "" && (!probe || trusted))) && name !== "" && !labelError && !pathError
    && (action === "forward" || (target !== "" && !locationError));

  async function fetchFingerprint() {
    setProbing(true);
    setProbe(null);
    setMatched(false);
    setProbeFailure(null);
    try {
      const res = await api("/external/fingerprint", { method: "POST", body: JSON.stringify({ upstream: upstream.trim() }) });
      if (!res.ok) setProbeFailure(await failureOf(res));
      else setProbe(await res.json() as Probe);
    } catch (cause) {
      setProbeFailure(asReport(cause));
    } finally { setProbing(false); }
  }

  async function saveExternal() {
    const chosen = probe ? probe.fingerprint : pin.trim();
    const runtime = { kind: "external", upstream: upstream.trim(), pin: https && chosen ? chosen : null };
    const res = externalRoot
      ? await api(`/apps/id/${encodeURIComponent(externalRoot.id)}`, { method: "PUT", body: JSON.stringify({ hostname, runtime }) })
      : await api("/apps", { method: "POST", body: JSON.stringify({ name: nameFor(name, apps), hostname, runtime }) });
    return res.ok ? null : await failureOf(res);
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (external && valid && !saving) {
      setSaving(true);
      setFailure(null);
      try {
        const refused = await saveExternal();
        if (refused) setFailure(refused);
        else onSaved(externalRoot ? "Route saved" : "Route added");
      } catch (cause) {
        setFailure(asReport(cause));
      } finally { setSaving(false); }
      return;
    }
    if (!app || !valid || saving) return;
    setSaving(true);
    setFailure(null);
    try {
      // What the change leaves: the old route or redirect goes, the new one takes its place.
      const kept = editing ? replaceRoute(app, editing, null) : { aliases: [...(app.aliases ?? [])], route_rules: [...(app.route_rules ?? [])] };
      const redirects = [...(app.redirects ?? [])];
      if (editor.redirect !== undefined) redirects.splice(editor.redirect, 1);
      let body;
      if (action === "forward") {
        const next = replaceRoute({ ...app, ...kept }, null, { hostname, path: prefix, stripPrefix: prefix !== "/" && strip });
        body = { ...next, redirects };
      } else {
        // A redirect answers only on a hostname that reaches the Application, so a new one becomes an alias.
        const answered = hostname === app.hostname || kept.aliases.includes(hostname) || kept.route_rules.some((rule) => rule.hostname === hostname);
        if (!answered) kept.aliases.push(hostname);
        redirects.splice(editor.redirect ?? redirects.length, 0, redirectRule({ hostname, path: prefix, location: target, permanent }));
        body = { ...kept, redirects };
      }
      const refused = await saveRoutes(app, body);
      if (refused) setFailure(refused);
      else onSaved(editing || redirecting ? "Route saved" : "Route added");
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setSaving(false);
    }
  }

  const shownHost = name ? hostname : <span className="muted">{where === OTHER ? "app.example.com" : `name${zone}`}</span>;
  return (
    <>
      <DialogHeader>
        <DialogTitle>{editing || redirecting ? "Edit route" : "Add route"}</DialogTitle>
        <DialogDescription>A hostname and path, and what they do. It applies at once; nothing is rebuilt or restarted.</DialogDescription>
      </DialogHeader>
      <form onSubmit={(event) => void submit(event)}>
        <DialogBody>
          <Select value={appId} onValueChange={(next) => { setAppId(next); setFailure(null); }} disabled={editor.app !== null || saving}>
            <FormField id="route-app" label="Destination" hint={editor.app ? undefined : external ? "A device or service on your LAN that this Host does not run." : "An Application with a Hostname, or an External app on your LAN."}>
              <SelectTrigger><SelectValue /></SelectTrigger>
            </FormField>
            <SelectContent>
              {applications.map((one) => <SelectItem key={one.id} value={one.id}>{one.runtime?.kind === "external" ? `External app · ${targetLabel(one)}` : one.name}</SelectItem>)}
              <SelectItem value={EXTERNAL}>External app</SelectItem>
            </SelectContent>
          </Select>
          {external ? <>
            <FormField id="route-upstream" label="Upstream URL" className="mono" value={upstream} required disabled={saving}
              placeholder="https://192.168.1.1" autoComplete="off" spellCheck={false}
              onChange={(event) => { setUpstream(event.target.value); setProbe(null); setMatched(false); setProbeFailure(null); }}
              hint="http:// or https://, a private LAN IP address and an optional port. No names, no path." />
            {https ? <>
              {probe ? (
                <div className="field">
                  <Label>Certificate fingerprint</Label>
                  <Card className="pin-card">
                    <CardContent className="stack">
                      {verdict === "match"
                        ? <p className="pin-card-verdict pin-card-match"><Icon name="check" />Matches what you entered.</p>
                        : verdict === "mismatch"
                          ? <p className="pin-card-verdict"><Icon name="alert" />Does not match what you entered. Compare with the fingerprint the device shows.</p>
                          : <p className="pin-card-verdict"><Icon name="alert" />Unverified. Compare with the fingerprint the device shows.</p>}
                      <p className="mono pin-card-print">{probe.fingerprint}</p>
                      <p className="mono muted t-label">{[probe.subject, probe.not_after ? `expires ${expiryFormat.format(new Date(probe.not_after))}` : null].filter(Boolean).join("  ·  ")}</p>
                      <div className="row pin-card-actions">
                        {verdict === "match" ? null : <div className="check">
                          <Checkbox id="route-pin-match" checked={matched} disabled={saving} onCheckedChange={(checked) => setMatched(checked === true)} />
                          <Label htmlFor="route-pin-match">It matches the device</Label>
                        </div>}
                        <Button type="button" size="sm" variant="ghost" disabled={saving || probing} onClick={() => void fetchFingerprint()}>{probing ? "Fetching..." : "Refetch"}</Button>
                        <Button type="button" size="sm" variant="ghost" disabled={saving} onClick={() => { setProbe(null); setMatched(false); }}>Clear</Button>
                      </div>
                    </CardContent>
                  </Card>
                </div>
              ) : <>
                <FormField id="route-pin" label="Certificate fingerprint" className="mono" value={pin} disabled={saving}
                  placeholder="SHA-256, 64 hex digits" autoComplete="off" spellCheck={false}
                  onChange={(event) => setPin(event.target.value)}
                  hint="Pins a self-signed certificate. Leave empty when a public CA signed it." />
                <div className="row">
                  <Button type="button" size="sm" disabled={saving || probing || !upstream.trim()} onClick={() => void fetchFingerprint()}>
                    {probing ? "Fetching..." : "Fetch fingerprint"}
                  </Button>
                </div>
              </>}
              {probeFailure ? <Failure failure={probeFailure} /> : null}
            </> : null}
          </> : null}
          <Select value={where} onValueChange={(next) => { setWhere(next); setLabel(""); }} disabled={saving}>
            <FormField id="route-zone" label="Domain" hint={where === OTHER ? "Point its DNS at this Host first." : "The Platform answers its names itself."}>
              <SelectTrigger><SelectValue /></SelectTrigger>
            </FormField>
            <SelectContent>
              <SelectItem value={dnsSuffix}>{dnsSuffix}</SelectItem>
              <SelectItem value={OTHER}>Other domain</SelectItem>
            </SelectContent>
          </Select>
          <FormField
            id="route-hostname"
            label={where === OTHER ? "Hostname" : "Subdomain"}
            className="mono"
            placeholder={where === OTHER ? "app.example.com" : "name"}
            suffix={where === OTHER ? undefined : zone}
            value={label}
            autoFocus
            autoComplete="off"
            list="route-hostnames"
            disabled={saving}
            onChange={(event) => setLabel(event.target.value)}
            error={labelError}
          />
          <datalist id="route-hostnames">
            {(where === OTHER ? outside : labels).map((one) => <option key={one} value={one} />)}
          </datalist>
          {!external ? <FormField
            id="route-path"
            label="Path"
            className="mono"
            placeholder="/"
            value={path}
            disabled={saving}
            onChange={(event) => setPath(event.target.value)}
            error={pathError}
          /> : null}
          {!external ? <RadioGroup label="Action" variant="tiles" value={action} disabled={saving} onValueChange={(next) => setAction(next as Action)}>
            <RadioGroupItem value="forward" label={`Send to ${app?.name ?? "the Application"}`} description="Requests reach the Application, and the address bar keeps the URL." />
            <RadioGroupItem value="redirect" label="Redirect" description="The browser is sent to another path or URL, and the address bar changes." />
          </RadioGroup> : null}
          {external ? null : action === "forward" ? (
            prefix !== "/" ? (
              <div className="check">
                <Checkbox id="route-strip" checked={strip} disabled={saving} onCheckedChange={(checked) => setStrip(checked === true)} />
                <Label htmlFor="route-strip">Remove {prefix} before forwarding</Label>
              </div>
            ) : null
          ) : <>
            <FormField
              id="route-location"
              label="Send to"
              className="mono"
              placeholder="/admin"
              value={location}
              autoComplete="off"
              spellCheck={false}
              disabled={saving}
              onChange={(event) => setLocation(event.target.value)}
              error={locationError}
              hint="A path on this hostname, or a whole URL."
            />
            <div className="check">
              <Checkbox id="route-permanent" checked={permanent} disabled={saving} onCheckedChange={(checked) => setPermanent(checked === true)} />
              <Label htmlFor="route-permanent">Permanent (301). Browsers remember it; off answers 302.</Label>
            </div>
            {onPattern && app ? (
              <div className="between">
                <span className="muted t-label">Matching many URLs at once?</span>
                <Button size="sm" type="button" variant="ghost" disabled={saving} onClick={() => onPattern(app)}>Write a pattern</Button>
              </div>
            ) : null}
          </>}
          <p className="route-preview mono t-label" aria-live="polite">
            https://{shownHost}{prefix === "/" ? "" : prefix}
            <span className="muted"> → </span>
            {action === "forward"
              ? external ? (upstream.trim() || <span className="muted">https://192.168.1.1</span>)
              : <>{app ? targetLabel(app) : <span className="muted">Application</span>}{prefix !== "/" && !strip ? prefix : ""}</>
              : target ? (target.startsWith("/") ? <>https://{shownHost}{target}</> : target) : <span className="muted">/path</span>}
            {action === "redirect" ? <span className="muted"> ({permanent ? "301" : "302"})</span> : null}
          </p>
          {failure ? <Failure failure={failure} /> : null}
        </DialogBody>
        <DialogFooter>
          <Button size="sm" type="button" disabled={saving} onClick={onClose}>Cancel</Button>
          <Button size="sm" type="submit" variant="primary" disabled={!valid || saving}>
            {saving ? "Saving..." : editing || redirecting ? "Save route" : "Add route"}
          </Button>
        </DialogFooter>
      </form>
    </>
  );
}
