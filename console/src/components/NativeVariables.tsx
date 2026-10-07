import { useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger, FormField, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@momoi-labs/kiso-react";
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
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Report | null>(null);
  const loadFailure = list.isError ? { error: "Could not load variable names.", caused_by: [] } : null;

  function edit(mode: typeof editor, name = "") {
    setEditor(mode); setKey(name); setValue(""); setFailure(null);
    onEditingChange(mode !== null);
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    event.stopPropagation();
    if (busy || disabled || !editor) return;
    setBusy(true); setFailure(null);
    try {
      const removing = editor === "remove";
      const url = `/apps/id/${encodeURIComponent(app.id)}/env${removing ? `/${encodeURIComponent(key)}` : ""}`;
      const response = await api(url, { method: removing ? "DELETE" : "POST", ...(!removing ? { body: JSON.stringify({ key: key.trim(), value }) } : {}) });
      if (!response.ok) { setFailure(await failureOf(response)); return; }
      const accepted = await response.json() as { task_id: string };
      const outcome = await waitForTask(accepted.task_id, { events: fetchEvents });
      if (outcome.status === "failed") setFailure(outcome.error ?? { error: "Could not update the variable.", caused_by: [] });
      else edit(null);
      await list.refetch();
      await reload?.();
    } catch { setFailure({ error: "Could not update the variable.", caused_by: [] }); }
    finally { setBusy(false); }
  }

  return <>
    <div className="row"><Button size="sm" type="button" disabled={disabled || loading} onClick={() => edit("add")}><Icon name="plus" />Add variable</Button></div>
    {loading ? <p className="muted">Loading...</p> : names.length ? <div className="table-wrap"><div className="table-scroll"><Table aria-label="Application variables">
      <TableHeader><TableRow><TableHead>Name</TableHead><TableHead>Value</TableHead><TableHead className="col-tight">Actions</TableHead></TableRow></TableHeader>
      <TableBody>{names.map((name) => <TableRow key={name}>
        <TableCell><code>{name}</code></TableCell><TableCell className="muted">Hidden</TableCell>
        <TableCell className="col-tight"><DropdownMenu><DropdownMenuTrigger asChild><Button size="sm" type="button" variant="ghost" className="btn-icon" disabled={disabled} aria-label={`Actions for ${name}`}><Icon name="more" /></Button></DropdownMenuTrigger><DropdownMenuContent align="end"><DropdownMenuItem onSelect={() => edit("replace", name)}>Replace</DropdownMenuItem><DropdownMenuItem variant="destructive" onSelect={() => edit("remove", name)}>Remove</DropdownMenuItem></DropdownMenuContent></DropdownMenu></TableCell>
      </TableRow>)}</TableBody>
    </Table></div></div> : <p className="muted">No variables.</p>}
    {!editor && (failure ?? loadFailure) ? <Failure failure={failure ?? loadFailure} /> : null}
    <Dialog open={editor !== null} onOpenChange={(open) => { if (!open && !busy) edit(null); }}>
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
          <DialogFooter><Button size="sm" type="button" disabled={busy} onClick={() => edit(null)}>Cancel</Button><Button size="sm" type="submit" variant={editor === "remove" ? "destructive" : "primary"} disabled={busy || disabled || !key.trim()}>{busy ? "Saving..." : editor === "remove" ? "Remove" : "Save variable"}</Button></DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  </>;
}
