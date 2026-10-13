import { useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger, FormField, Search, Table, TableBody, TableCell, TableHead, TableFrame, TableHeader, TableRow } from "@momoi-labs/kiso-react";
import { api, failureOf, readJson } from "../lib/api.js";
import { fetchEvents } from "../lib/useEvents.js";
import { waitForTask } from "../lib/tasks.js";
import type { App, Report } from "../lib/types.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";

const noNames: string[] = [];

/** Existing values stay on the Host. Editing starts with an empty replacement. */
export function NativeVariables({ app, reload, disabled, onEditingChange }: {
  app: App; reload?: () => Promise<App[]>; disabled: boolean; onEditingChange: (editing: boolean) => void;
}) {
  // Identity owns this panel; changing configuration does not reveal values.
  const list = useQuery({
    queryKey: ["apps", app.id, "variable-names"],
    queryFn: ({ signal }) => readJson<string[]>(`/apps/id/${encodeURIComponent(app.id)}/variable-names`, signal),
  });
  const names = list.data ?? noNames;
  const loading = list.isPending;
  const [editor, setEditor] = useState<"add" | "replace" | "remove" | null>(null);
  const [key, setKey] = useState("");
  const loadFailure = list.isError ? { error: "Could not load variable names.", caused_by: [] } : null;

  function edit(mode: typeof editor, name = "") {
    setEditor(mode); setKey(name);
    onEditingChange(mode !== null);
  }
  const [query, setQuery] = useState("");
  const visible = names.filter((name) => name.toLowerCase().includes(query.trim().toLowerCase()));
  return <>
    <TableFrame>
      <div className="table-toolbar panel-toolbar">
        <h2 className="section-title">Variables</h2>
        <Search aria-label="Search variables" containerClassName="section-search" placeholder="Search variables…"
          value={query} disabled={!names.length} onChange={(event) => setQuery(event.target.value)} />
        <Button size="sm" type="button" variant="ghost" className="panel-action" disabled={disabled || loading} onClick={() => edit("add")}>
          <Icon name="plus" />
          Add variable
        </Button>
      </div>
      <div className="table-scroll">
        <Table aria-label="Application variables">
          <TableHeader><TableRow><TableHead scope="col">Name</TableHead><TableHead scope="col">Value</TableHead><TableHead scope="col" className="col-tight" aria-label="Actions" /></TableRow></TableHeader>
          <TableBody>
            {loading || visible.length === 0 ? (
              <TableRow><TableCell colSpan={3} className="muted">{loading ? "Loading variables…" : names.length ? "No variables match your search." : "No variables."}</TableCell></TableRow>
            ) : visible.map((name) => <TableRow key={name}>
              <TableCell><span className="mono">{name}</span></TableCell>
              <TableCell><span className="mono muted">***************</span></TableCell>
              <TableCell className="col-tight"><DropdownMenu><DropdownMenuTrigger asChild><Button size="sm" type="button" variant="ghost" className="btn-icon" disabled={disabled} aria-label={`Actions for ${name}`}><Icon name="more" /></Button></DropdownMenuTrigger><DropdownMenuContent align="end"><DropdownMenuItem onSelect={() => edit("replace", name)}>Replace</DropdownMenuItem><DropdownMenuItem variant="destructive" onSelect={() => edit("remove", name)}>Remove</DropdownMenuItem></DropdownMenuContent></DropdownMenu></TableCell>
            </TableRow>)}
          </TableBody>
        </Table>
      </div>
    </TableFrame>
    {!editor && loadFailure ? <Failure failure={loadFailure} /> : null}
    <VariableDialog app={app} editor={editor} name={key} disabled={disabled} onClose={() => edit(null)}
      onSaved={async () => { await list.refetch(); await reload?.(); }} />
  </>;
}

/**
 * Adds, replaces or removes one Variable. A value is never read back: replacing
 * starts from an empty field.
 */
export function VariableDialog({ app, editor, name, disabled, onClose, onSaved, onBusyChange }: {
  app: App; editor: "add" | "replace" | "remove" | null; name: string; disabled?: boolean;
  onClose: () => void; onSaved: () => Promise<unknown>; onBusyChange?: (busy: boolean) => void;
}) {
  const [key, setKey] = useState(name);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Report | null>(null);
  const [opened, setOpened] = useState(editor);
  if (opened !== editor) { setOpened(editor); setKey(name); setValue(""); setFailure(null); }

  function working(next: boolean) { setBusy(next); onBusyChange?.(next); }
  async function submit(event: FormEvent) {
    event.preventDefault();
    event.stopPropagation();
    if (busy || disabled || !editor) return;
    working(true); setFailure(null);
    try {
      const removing = editor === "remove";
      const url = `/apps/id/${encodeURIComponent(app.id)}/env${removing ? `/${encodeURIComponent(key)}` : ""}`;
      const response = await api(url, { method: removing ? "DELETE" : "POST", ...(!removing ? { body: JSON.stringify({ key: key.trim(), value }) } : {}) });
      if (!response.ok) { setFailure(await failureOf(response)); return; }
      const accepted = await response.json() as { task_id: string };
      const outcome = await waitForTask(accepted.task_id, { events: fetchEvents });
      if (outcome.status === "failed") setFailure(outcome.error ?? { error: "Could not update the variable.", caused_by: [] });
      else onClose();
      await onSaved();
    } catch { setFailure({ error: "Could not update the variable.", caused_by: [] }); }
    finally { working(false); }
  }

  return (
    <Dialog open={editor !== null} onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
      <DialogContent>
        <DialogHeader><DialogTitle>{editor === "remove" ? "Remove variable" : editor === "replace" ? "Replace variable" : "Add variable"}</DialogTitle></DialogHeader>
        <form onSubmit={(event) => void submit(event)}>
          <DialogBody>
            {editor === "remove" ? <p>Remove <code>{key}</code>?</p> : <>
              <FormField id="native-variable-key" label="Name" value={key} autoComplete="off" required pattern="[A-Za-z_][A-Za-z0-9_]*" disabled={busy || editor === "replace"} autoFocus={editor === "add"} onChange={(event) => setKey(event.target.value)} />
              <FormField id="native-variable-value" label={editor === "replace" ? "New value" : "Value"} type="password" autoComplete="new-password" value={value} disabled={busy} autoFocus={editor === "replace"} onChange={(event) => setValue(event.target.value)} />
            </>}
            {failure ? <Failure failure={failure} /> : null}
          </DialogBody>
          <DialogFooter><Button size="sm" type="button" disabled={busy} onClick={onClose}>Cancel</Button><Button size="sm" type="submit" variant={editor === "remove" ? "destructive" : "primary"} disabled={busy || disabled || !key.trim()}>{busy ? "Saving..." : editor === "remove" ? "Remove" : "Save variable"}</Button></DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
