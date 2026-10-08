import { useState, type FormEvent } from "react";
import {
  Button, Checkbox, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  FormField, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { isPublished, replaceRoute, routesOf, saveRoutes, targetLabel, type Route } from "../lib/routes.js";
import type { App } from "../lib/types.js";
import { Failure } from "./Failure.js";

/** What the dialog opens on: the Application, or null to choose one, and the route when changing one. */
export type RouteEditor = { app: App | null; route?: Route };

/**
 * One way to add or change a route, from the Routes page or from an
 * Application's Routes tab. Either place writes the same record: a route
 * belongs to the Application it reaches, and saving it rebuilds nothing.
 */
export function RouteDialog({ editor, apps, dnsSuffix, onClose, onSaved }: {
  editor: RouteEditor | null;
  apps: App[];
  dnsSuffix: string;
  onClose: () => void;
  /** Called once the proxy serves the change; says what changed. */
  onSaved: (message: string) => void;
}) {
  const [saving, setSaving] = useState(false);
  return (
    <Dialog open={editor !== null} onOpenChange={(open) => { if (!open && !saving) onClose(); }}>
      <DialogContent>
        {editor ? (
          <RouteForm key={editor.route?.key ?? editor.app?.id ?? "any"} editor={editor} apps={apps} dnsSuffix={dnsSuffix}
            saving={saving} setSaving={setSaving} onClose={onClose} onSaved={onSaved} />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function RouteForm({ editor, apps, dnsSuffix, saving, setSaving, onClose, onSaved }: {
  editor: RouteEditor;
  apps: App[];
  dnsSuffix: string;
  saving: boolean;
  setSaving: (saving: boolean) => void;
  onClose: () => void;
  onSaved: (message: string) => void;
}) {
  const editing = editor.route;
  const published = apps.filter(isPublished);
  const [appId, setAppId] = useState(editor.app?.id ?? published[0]?.id ?? "");
  const app = published.find((one) => one.id === appId);
  const zone = `.${dnsSuffix}`;
  // A hostname outside the Zone, set through the API, is edited whole.
  const foreign = editing !== undefined && !editing.hostname.endsWith(zone);
  const [label, setLabel] = useState(editing ? (foreign ? editing.hostname : editing.hostname.slice(0, -zone.length)) : "");
  const [path, setPath] = useState(editing?.path ?? "/");
  // Removing the prefix is the default, as Coolify does; a changed route keeps its choice.
  const [strip, setStrip] = useState(editing ? editing.stripPrefix : true);
  const [failure, setFailure] = useState<unknown>(null);

  const name = label.trim().toLowerCase();
  const hostname = foreign ? name : `${name}${zone}`;
  const prefix = path.trim() || "/";
  const routes = published.flatMap(routesOf);
  const clash = routes.find((route) => route.key !== editing?.key && route.hostname === hostname && route.path === prefix);
  const labels = [...new Set(routes.map((route) => route.hostname).filter((one) => one.endsWith(zone)).map((one) => one.slice(0, -zone.length)))];
  const labelError = !name
    ? null
    : !/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/.test(name)
      ? "Use letters, digits and hyphens, with a dot between names."
      : hostname === `admin${zone}`
        ? `${hostname} belongs to the Platform.`
        : clash
          ? `${hostname}${prefix === "/" ? "" : prefix} already reaches ${clash.app.name}.`
          : null;
  const pathError = !/^\/([A-Za-z0-9._~-]+(\/[A-Za-z0-9._~-]+)*)?$/.test(prefix)
    ? "Start with / and use letters, digits, dots, hyphens or underscores."
    : null;
  const valid = app !== undefined && name !== "" && !labelError && !pathError;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!app || !valid || saving) return;
    setSaving(true);
    setFailure(null);
    try {
      const refused = await saveRoutes(app, replaceRoute(app, editing ?? null, { hostname, path: prefix, stripPrefix: prefix !== "/" && strip }));
      if (refused) setFailure(refused);
      else onSaved(editing ? "Route saved" : "Route added");
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <DialogHeader>
        <DialogTitle>{editing ? "Edit route" : "Add route"}</DialogTitle>
        <DialogDescription>It applies at once. Nothing is rebuilt or restarted.</DialogDescription>
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
          <FormField
            id="route-hostname"
            label="Hostname"
            className="mono"
            placeholder="name"
            suffix={foreign ? undefined : zone}
            value={label}
            autoFocus
            autoComplete="off"
            list="route-hostnames"
            disabled={saving}
            onChange={(event) => setLabel(event.target.value)}
            error={labelError}
          />
          <datalist id="route-hostnames">
            {labels.map((one) => <option key={one} value={one} />)}
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
          {prefix !== "/" ? (
            <div className="check">
              <Checkbox id="route-strip" checked={strip} disabled={saving} onCheckedChange={(checked) => setStrip(checked === true)} />
              <Label htmlFor="route-strip">Remove {prefix} before forwarding</Label>
            </div>
          ) : null}
          <p className="route-preview mono t-label" aria-live="polite">
            https://{name ? hostname : <span className="muted">name{zone}</span>}{prefix === "/" ? "" : prefix}
            <span className="muted"> → </span>
            {app ? targetLabel(app) : <span className="muted">Application</span>}{prefix !== "/" && !strip ? prefix : ""}
          </p>
          {failure ? <Failure failure={failure} /> : null}
        </DialogBody>
        <DialogFooter>
          <Button size="sm" type="button" disabled={saving} onClick={onClose}>Cancel</Button>
          <Button size="sm" type="submit" variant="primary" disabled={!valid || saving}>
            {saving ? "Saving..." : editing ? "Save route" : "Add route"}
          </Button>
        </DialogFooter>
      </form>
    </>
  );
}
