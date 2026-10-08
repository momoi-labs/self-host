import { useState, type FormEvent } from "react";
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
  EmptyState, EmptyStateTitle, FormField, PageHeader, PageHeaderTitle, Search, Skeleton,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue, StatusBadge,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";
import type { App, DatabaseConnection, Report } from "../lib/types.js";
import { api, asReport, failureOf } from "../lib/api.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";

import { useNativeCapabilities } from "../lib/useNativeCapabilities.js";

export function DatabaseConnections({ app, apps, connections, loading, busy, failure, onOperation }: {
  app: App; apps: App[]; connections: DatabaseConnection[]; loading: boolean; busy: boolean; failure: Report | null;
  onOperation: (path: string, method: string, body?: object) => Promise<boolean>;
}) {
  const { capabilities } = useNativeCapabilities();
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [editor, setEditor] = useState<"connect" | "view" | "disconnect" | null>(null);
  const [selected, setSelected] = useState<DatabaseConnection | null>(null);
  const [consumer, setConsumer] = useState("");
  const [variable, setVariable] = useState("DATABASE_URL");
  const [url, setUrl] = useState<string | null>(null);
  const [revealing, setRevealing] = useState(false);
  const [revealFailure, setRevealFailure] = useState<Report | null>(null);
  const choices = apps.filter((candidate) => !candidate.managed_postgres && (candidate.runtime?.kind !== "native" || capabilities?.managed_postgres));
  const appName = (connection: DatabaseConnection) => apps.find((candidate) => candidate.id === connection.consumer_application_id)?.name ?? "Removed application";
  const visible = connections.filter((connection) => `${appName(connection)} ${connection.database} ${connection.variable}`.toLowerCase().includes(query.trim().toLowerCase()) && (status === "all" || connection.status === status));
  function close() { setEditor(null); setSelected(null); setUrl(null); setRevealFailure(null); }
  async function submit(event: FormEvent) {
    event.preventDefault();
    const path = `/databases/${app.id}/connections`;
    const ok = editor === "connect"
      ? await onOperation(path, "POST", { consumer_application_id: consumer, variable })
      : selected ? await onOperation(`${path}/${selected.id}`, "DELETE") : false;
    if (ok) close();
  }
  async function reveal() {
    if (!selected || revealing) return;
    setRevealing(true); setRevealFailure(null);
    try {
      const response = await api(`/databases/${app.id}/connections/${selected.id}/reveal`, { method: "POST" });
      if (!response.ok) throw await failureOf(response);
      setUrl((await response.json() as { url: string }).url);
    } catch (error) { setRevealFailure(asReport(error)); }
    finally { setRevealing(false); }
  }

  return <div className="stack">
    <PageHeader actions={<Button size="sm" variant="primary" disabled={busy || loading || app.status !== "running"} onClick={() => { setConsumer(""); setVariable("DATABASE_URL"); setEditor("connect"); }}><Icon name="plus" />Connect application</Button>}>
      <PageHeaderTitle asChild><h2 className="t-h2">Connected applications</h2></PageHeaderTitle>
    </PageHeader>
    <div className="list-filters">
      <Search aria-label="Search connections" placeholder="Search connections..." value={query} onChange={(event) => setQuery(event.target.value)} />
      <Select value={status} onValueChange={setStatus}><SelectTrigger aria-label="Filter connection status"><SelectValue /></SelectTrigger><SelectContent>
        <SelectItem value="all">All statuses</SelectItem>
        {Array.from(new Set(connections.map((connection) => connection.status))).sort().map((value) => <SelectItem key={value} value={value}>{value}</SelectItem>)}
      </SelectContent></Select>
      {query || status !== "all" ? <Button size="sm" variant="ghost" onClick={() => { setQuery(""); setStatus("all"); }}>Clear filters</Button> : null}
    </div>
    {!loading && !connections.length ? <EmptyState variant="first-run"><EmptyStateTitle>No connections</EmptyStateTitle></EmptyState> : <div className="table-wrap"><div className="table-scroll"><Table aria-label="Database connections" aria-busy={loading}>
      <TableHeader><TableRow><TableHead>Application</TableHead><TableHead>Variable</TableHead><TableHead>Status</TableHead><TableHead className="col-tight">Actions</TableHead></TableRow></TableHeader>
      <TableBody>{loading ? <TableRow>{[0, 1, 2, 3].map((cell) => <TableCell key={cell}><Skeleton /></TableCell>)}</TableRow> : visible.length ? visible.map((connection) => <TableRow key={connection.id} aria-selected={selected?.id === connection.id}>
        <TableCell><div className="stack-xs"><strong>{appName(connection)}</strong><span className="muted">{apps.find((candidate) => candidate.id === connection.consumer_application_id)?.runtime?.kind === "native" ? "Native process" : "Container"}</span></div></TableCell>
        <TableCell className="mono">{connection.variable}</TableCell>
        <TableCell><StatusBadge tone={connection.status === "ready" ? "success" : connection.status === "failed" ? "danger" : "neutral"}>{connection.status}</StatusBadge></TableCell>
        <TableCell className="col-tight"><DropdownMenu><DropdownMenuTrigger asChild><Button size="sm" variant="ghost" className="btn-icon" aria-label={`Actions for ${appName(connection)}`}><Icon name="more" /></Button></DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => { setSelected(connection); setEditor("view"); }}>View connection</DropdownMenuItem>
            <DropdownMenuItem asChild><a href={`#app-${encodeURIComponent(connection.consumer_application_id)}`}>Open application</a></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" disabled={busy} onSelect={() => { setSelected(connection); setEditor("disconnect"); }}>Disconnect</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu></TableCell>
      </TableRow>) : <TableRow><TableCell colSpan={4}>No connections match your filters.</TableCell></TableRow>}</TableBody>
    </Table></div><p className="table-footer"><span>{visible.length} of {connections.length} connections</span></p></div>}
    <Dialog open={editor !== null} onOpenChange={(open) => { if (!open && !busy && !revealing) close(); }}>
      <DialogContent>
        <DialogHeader><DialogTitle>{editor === "connect" ? "Connect application" : editor === "disconnect" ? "Disconnect application" : selected ? appName(selected) : "Connection"}</DialogTitle></DialogHeader>
        {editor === "view" && selected ? <>
          <DialogBody>
            <dl className="summary-facts"><dt>Database</dt><dd><code>{selected.database}</code></dd><dt>User</dt><dd><code>{selected.role}</code></dd><dt>Variable</dt><dd><code>{selected.variable}</code></dd></dl>
            {url ? <FormField id="database-connection-url" label="Connection URL" readOnly value={url} autoComplete="off" /> : null}
            {revealFailure ? <Failure failure={revealFailure} /> : null}
          </DialogBody>
          <DialogFooter><Button size="sm" disabled={revealing} onClick={close}>Close</Button><Button size="sm" variant="primary" disabled={revealing || selected.status !== "ready"} onClick={() => url ? setUrl(null) : void reveal()}>{revealing ? "Loading..." : url ? "Hide URL" : "Reveal URL"}</Button></DialogFooter>
        </> : <form onSubmit={(event) => void submit(event)}>
          <DialogBody>
            {editor === "connect" ? <>
              <Select value={consumer} onValueChange={setConsumer} disabled={busy}>
                <FormField id="database-consumer" label="Application"><SelectTrigger><SelectValue placeholder="Choose an application" /></SelectTrigger></FormField>
                <SelectContent>{choices.map((candidate) => <SelectItem key={candidate.id} value={candidate.id}>{candidate.name}</SelectItem>)}</SelectContent>
              </Select>
              <FormField id="database-variable" label="Variable" value={variable} required pattern="[A-Za-z_][A-Za-z0-9_]*" disabled={busy} onChange={(event) => setVariable(event.target.value)} />
            </> : <p>Disconnect <strong>{selected ? appName(selected) : ""}</strong>? Its database data is kept.</p>}
            {failure ? <Failure failure={failure} /> : null}
          </DialogBody>
          <DialogFooter><Button size="sm" type="button" disabled={busy} onClick={close}>Cancel</Button><Button size="sm" type="submit" variant={editor === "disconnect" ? "destructive" : "primary"} disabled={busy || (editor === "connect" && (!consumer || !variable.trim()))}>{busy ? "Applying..." : editor === "connect" ? "Connect" : "Disconnect"}</Button></DialogFooter>
        </form>}
      </DialogContent>
    </Dialog>
  </div>;
}
