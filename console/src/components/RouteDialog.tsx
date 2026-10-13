import { useState, type FormEvent } from "react";
import {
  Button, Checkbox, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  FormField, Label, RadioGroup, RadioGroupItem, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import {
  isPublished, redirectDraft, redirectRule, replaceRoute, routesOf, saveRoutes, targetLabel, type Route,
} from "../lib/routes.js";
import type { App } from "../lib/types.js";
import { Failure } from "./Failure.js";

/**
 * What the dialog opens on: the Application, or null to choose one, and the
 * route or the redirect (by its index) when changing one.
 */
export type RouteEditor = { app: App | null; route?: Route; redirect?: number };

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
      <DialogContent>
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
  const [appId, setAppId] = useState(editor.app?.id ?? published[0]?.id ?? "");
  const app = published.find((one) => one.id === appId);
  const zone = `.${dnsSuffix}`;
  const initial = editing?.hostname ?? draft?.hostname ?? "";
  const inZone = initial === "" || initial.endsWith(zone);
  const [where, setWhere] = useState(inZone ? dnsSuffix : OTHER);
  const [label, setLabel] = useState(inZone ? initial.slice(0, initial.length ? -zone.length : 0) : initial);
  const [path, setPath] = useState(editing?.path ?? draft?.path ?? "/");
  const [action, setAction] = useState<Action>(draft ? "redirect" : "forward");
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
        ? "Type the whole domain, such as admin.example.com."
        : hostname === `admin${zone}`
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
  const valid = app !== undefined && name !== "" && !labelError && !pathError
    && (action === "forward" || (target !== "" && !locationError));

  async function submit(event: FormEvent) {
    event.preventDefault();
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

  const shownHost = name ? hostname : <span className="muted">{where === OTHER ? "admin.example.com" : `name${zone}`}</span>;
  return (
    <>
      <DialogHeader>
        <DialogTitle>{editing || redirecting ? "Edit route" : "Add route"}</DialogTitle>
        <DialogDescription>A hostname and path, and what they do. It applies at once; nothing is rebuilt or restarted.</DialogDescription>
      </DialogHeader>
      <form onSubmit={(event) => void submit(event)}>
        <DialogBody>
          <Select value={appId} onValueChange={setAppId} disabled={editor.app !== null || saving}>
            <FormField id="route-app" label="Application" hint={editor.app ? undefined : "Only an Application with a Hostname can be reached."}>
              <SelectTrigger><SelectValue placeholder="No Application has a Hostname" /></SelectTrigger>
            </FormField>
            <SelectContent>
              {published.map((one) => <SelectItem key={one.id} value={one.id}>{one.name}</SelectItem>)}
            </SelectContent>
          </Select>
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
            placeholder={where === OTHER ? "admin.example.com" : "name"}
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
          <FormField
            id="route-path"
            label="Path"
            className="mono"
            placeholder="/"
            value={path}
            disabled={saving}
            onChange={(event) => setPath(event.target.value)}
            error={pathError}
          />
          <RadioGroup label="Action" variant="tiles" value={action} disabled={saving} onValueChange={(next) => setAction(next as Action)}>
            <RadioGroupItem value="forward" label={`Send to ${app?.name ?? "the Application"}`} description="Requests reach the Application, and the address bar keeps the URL." />
            <RadioGroupItem value="redirect" label="Redirect" description="The browser is sent to another path or URL, and the address bar changes." />
          </RadioGroup>
          {action === "forward" ? (
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
              ? <>{app ? targetLabel(app) : <span className="muted">Application</span>}{prefix !== "/" && !strip ? prefix : ""}</>
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
