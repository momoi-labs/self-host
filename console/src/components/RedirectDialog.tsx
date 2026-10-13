import { useState, type FormEvent } from "react";
import {
  Button, Checkbox, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  FormField, Label,
} from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { saveRedirects } from "../lib/routes.js";
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
  const [pattern, setPattern] = useState(editing?.pattern ?? `^https://${app.hostname}/`);
  const [replacement, setReplacement] = useState(editing?.replacement ?? "");
  // A moved page is the usual reason, and browsers cache a 301 for good.
  const [permanent, setPermanent] = useState(editing?.permanent ?? true);
  const [failure, setFailure] = useState<unknown>(null);

  const trimmedPattern = pattern.trim();
  const location = replacement.trim();
  const locationError = !location
    ? null
    : /\s/.test(location)
      ? "A location has no spaces."
      : /^(https?:\/\/|\/|\$)/.test(location)
        ? null
        : "Start with https://, http:// or /.";
  const valid = trimmedPattern !== "" && location !== "" && !locationError;

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
        <DialogTitle>{editing ? "Edit redirect pattern" : "Add redirect pattern"}</DialogTitle>
        <DialogDescription>
          Answers on every hostname of {app.name} before a request reaches it. It applies at once.
        </DialogDescription>
      </DialogHeader>
      <form onSubmit={(event) => void submit(event)}>
        <DialogBody>
          <FormField
            id="redirect-pattern"
            label="Pattern"
            className="mono"
            placeholder={`^https://${app.hostname}/old/(.*)$`}
            value={pattern}
            autoFocus
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
