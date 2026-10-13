import { useState, type FormEvent } from "react";
import {
  Button, Checkbox, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  FormField, Label, RadioGroup, RadioGroupItem, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { rootRedirect, rootRedirectRule, saveRedirects } from "../lib/routes.js";
import type { App, RedirectRule } from "../lib/types.js";
import { Failure } from "./Failure.js";

/** What the dialog opens on: the Application, and the rule's index when changing one. */
export type RedirectEditor = { app: App; index?: number };

/**
 * One way to add or change a redirect. A redirect belongs to the Application
 * whose hostnames it answers on, and saving it rebuilds nothing.
 */
export function RedirectDialog({ editor, onClose, onSaved }: {
  editor: RedirectEditor | null;
  onClose: () => void;
  /** Called once the proxy serves the change; says what changed. */
  onSaved: (message: string) => void;
}) {
  const [saving, setSaving] = useState(false);
  return (
    <Dialog open={editor !== null} onOpenChange={(open) => { if (!open && !saving) onClose(); }}>
      <DialogContent>
        {editor ? (
          <RedirectForm key={`${editor.app.id}:${editor.index ?? "new"}`} editor={editor}
            saving={saving} setSaving={setSaving} onClose={onClose} onSaved={onSaved} />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function RedirectForm({ editor, saving, setSaving, onClose, onSaved }: {
  editor: RedirectEditor;
  saving: boolean;
  setSaving: (saving: boolean) => void;
  onClose: () => void;
  onSaved: (message: string) => void;
}) {
  const { app, index } = editor;
  const editing = index === undefined ? undefined : app.redirects?.[index];
  const root = editing ? rootRedirect(editing) : null;
  const hostnames = [app.hostname, ...(app.aliases ?? [])].filter((one): one is string => !!one);
  // Sending a hostname to a path is the usual redirect; any other is a pattern over the URL.
  const [mode, setMode] = useState<"root" | "pattern">(editing && !root ? "pattern" : "root");
  const [hostname, setHostname] = useState(root?.hostname ?? hostnames[0] ?? "");
  const [path, setPath] = useState(root?.path ?? "/");
  const [pattern, setPattern] = useState(editing?.pattern ?? `^https://${app.hostname}/`);
  const [replacement, setReplacement] = useState(editing?.replacement ?? "");
  // A moved page is the usual reason, and browsers cache a 301 for good.
  const [permanent, setPermanent] = useState(editing?.permanent ?? true);
  const [failure, setFailure] = useState<unknown>(null);

  const trimmedPath = path.trim();
  const pathError = /^\/[A-Za-z0-9._~/%-]*$/.test(trimmedPath) ? null : "Start with / and use letters, digits, dots, hyphens, slashes or underscores.";
  const rootRule = rootRedirectRule(hostname, trimmedPath, permanent);
  const trimmedPattern = mode === "root" ? rootRule.pattern : pattern.trim();
  const location = mode === "root" ? rootRule.replacement : replacement.trim();
  const locationError = !location
    ? null
    : /\s/.test(location)
      ? "A location has no spaces."
      : /^(https?:\/\/|\/|\$)/.test(location)
        ? null
        : "Start with https://, http:// or /.";
  const valid = mode === "root"
    ? hostname !== "" && trimmedPath !== "/" && !pathError
    : trimmedPattern !== "" && location !== "" && !locationError;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!valid || saving) return;
    setSaving(true);
    setFailure(null);
    const rule: RedirectRule = { pattern: trimmedPattern, replacement: location, permanent };
    const redirects = [...(app.redirects ?? [])];
    if (index === undefined) redirects.push(rule);
    else redirects[index] = rule;
    try {
      const refused = await saveRedirects(app, redirects);
      if (refused) setFailure(refused);
      else onSaved(editing ? "Redirect saved" : "Redirect added");
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <DialogHeader>
        <DialogTitle>{editing ? "Edit redirect" : "Add redirect"}</DialogTitle>
        <DialogDescription>
          Answers on every hostname of {app.name} before a request reaches it. It applies at once.
        </DialogDescription>
      </DialogHeader>
      <form onSubmit={(event) => void submit(event)}>
        <DialogBody>
          <RadioGroup label="Redirect" variant="segmented" value={mode} disabled={saving} onValueChange={(next) => setMode(next as "root" | "pattern")}>
            <RadioGroupItem value="root" label="Hostname to a path" />
            <RadioGroupItem value="pattern" label="Pattern" />
          </RadioGroup>
          {mode === "root" ? <>
            <Select value={hostname} onValueChange={setHostname} disabled={saving}>
              <FormField id="redirect-hostname" label="Hostname" hint="A visit to its root is sent to the path.">
                <SelectTrigger><SelectValue placeholder="No hostname" /></SelectTrigger>
              </FormField>
              <SelectContent>
                {hostnames.map((one) => <SelectItem key={one} value={one}>{one}</SelectItem>)}
              </SelectContent>
            </Select>
            <FormField
              id="redirect-path"
              label="Path"
              className="mono"
              placeholder="/admin"
              value={path}
              autoComplete="off"
              spellCheck={false}
              disabled={saving}
              onChange={(event) => setPath(event.target.value)}
              error={trimmedPath && trimmedPath !== "/" ? pathError : null}
            />
            <p className="route-preview mono t-label" aria-live="polite">
              https://{hostname}/<span className="muted"> → </span>https://{hostname}{trimmedPath === "/" ? <span className="muted">/path</span> : trimmedPath}
            </p>
          </> : <>
            <FormField
              id="redirect-pattern"
              label="Pattern"
              className="mono"
              placeholder={`^https://${app.hostname}/old/(.*)$`}
              value={pattern}
              autoComplete="off"
              spellCheck={false}
              disabled={saving}
              onChange={(event) => setPattern(event.target.value)}
              hint="A regular expression over the whole URL, https://hostname/path?query. Escape dots as \. and anchor with ^ and $."
            />
            <FormField
              id="redirect-replacement"
              label="Send to"
              className="mono"
              placeholder={`https://${app.hostname}/new/\${1}`}
              value={replacement}
              autoComplete="off"
              spellCheck={false}
              disabled={saving}
              onChange={(event) => setReplacement(event.target.value)}
              error={locationError}
              hint="${1} inserts what the pattern's first group matched."
            />
          </>}
          <div className="check">
            <Checkbox id="redirect-permanent" checked={permanent} disabled={saving} onCheckedChange={(checked) => setPermanent(checked === true)} />
            <Label htmlFor="redirect-permanent">Permanent (301). Browsers remember it; off answers 302.</Label>
          </div>
          {failure ? <Failure failure={failure} /> : null}
        </DialogBody>
        <DialogFooter>
          <Button size="sm" type="button" disabled={saving} onClick={onClose}>Cancel</Button>
          <Button size="sm" type="submit" variant="primary" disabled={!valid || saving}>
            {saving ? "Saving..." : editing ? "Save redirect" : "Add redirect"}
          </Button>
        </DialogFooter>
      </form>
    </>
  );
}
