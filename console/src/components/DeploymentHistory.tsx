import { Fragment, useEffect, useRef, useState } from "react";
import {
  AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
  Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
  EmptyState, EmptyStateTitle, FormField, KV, KVKey, KVValue,
  Search, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Skeleton, StatusBadge,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";
import { api, failureOf } from "../lib/api.js";
import { fetchEvents } from "../lib/useEvents.js";
import { waitForTask } from "../lib/tasks.js";
import type { App, Report } from "../lib/types.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";
import { useToast } from "./Toasts.js";

type Deployment = {
  id: string; created_at: string; status: string; readiness: string;
  images: Record<string, string>; revision?: string | null;
  error?: Report | null; recoverable: boolean;
};
type Trigger = { enabled: boolean; branch?: string; token?: string };

function healthLabel(value: string) {
  return value === "ready" ? "Health checks passed" : value === "checking" ? "Checking health" : value === "failed" ? "Health checks failed" : "Health unverified";
}

function deploymentTone(status: string) {
  return status === "failed" ? "danger" : status === "completed" || status === "checking" ? "success" : "neutral";
}

export function DeploymentReadiness({ app }: { app: App }) {
  const value = app.readiness ?? "unknown";
  return <StatusBadge tone={value === "failed" ? "danger" : (value === "ready" || value === "checking") ? "success" : "neutral"} pulse={value === "checking"}>{healthLabel(value)}</StatusBadge>;
}

export function DeploymentHistory({ app, reload }: { app: App; reload: () => Promise<App[]> }) {
  const [entries, setEntries] = useState<Deployment[]>([]);
  const [error, setError] = useState<Report | null>(null);
  const [loadError, setLoadError] = useState<Report | null>(null);
  const [loading, setLoading] = useState(true);
  const [attempt, setAttempt] = useState(0);
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [selected, setSelected] = useState<string | null>(null);
  const [recovery, setRecovery] = useState<Deployment | null>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const notify = useToast();
  const path = `/apps/id/${encodeURIComponent(app.id)}`;
  const details = entries.find((entry) => entry.id === selected);
  const filtering = query.trim() !== "" || status !== "all";
  const visible = entries.filter((entry) => {
    const searchable = [entry.created_at, new Date(entry.created_at).toLocaleString(), entry.revision, ...Object.keys(entry.images), ...Object.values(entry.images)].join(" ").toLowerCase();
    return searchable.includes(query.trim().toLowerCase()) && (status === "all" || entry.status === status);
  });
  const clear = () => { setQuery(""); setStatus("all"); };
  const retry = () => { setLoading(true); setAttempt((value) => value + 1); };

  useEffect(() => {
    const controller = new AbortController();
    let reading = false;
    async function read() {
      if (reading) return;
      reading = true;
      try {
        const response = await api(`${path}/deployments`, { signal: controller.signal });
        if (!response.ok) { setLoadError(await failureOf(response)); return; }
        setEntries(await response.json() as Deployment[]);
        setLoadError(null);
      } catch (cause) {
        if (!controller.signal.aborted) setLoadError({ error: "Could not read deployment history", caused_by: [(cause as Error).message] });
      } finally {
        reading = false;
        if (!controller.signal.aborted) setLoading(false);
      }
    }
    void read();
    const timer = window.setInterval(() => void read(), 3000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, [path, app.status, attempt]);

  async function restore() {
    if (!recovery || inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(null);
    const selected = recovery; setRecovery(null);
    try {
      const response = await api(`${path}/deployments/${encodeURIComponent(selected.id)}/restore`, { method: "POST" });
      if (!response.ok) { setError(await failureOf(response)); return; }
      const { task_id } = await response.json() as { task_id: string };
      const outcome = await waitForTask(task_id, { events: fetchEvents });
      if (outcome.status === "failed") setError(outcome.error ?? { error: "Recovery failed", caused_by: [] });
      else notify("success", app.status === "stopped" ? "Previous deployment saved. Application remains stopped." : "Previous deployment restored");
    } catch (cause) { setError({ error: "Could not recover application", caused_by: [(cause as Error).message] }); }
    finally { inFlight.current = false; setBusy(false); await reload(); }
  }

  return <div className="stack">
    {app.git ? <DeployTrigger app={app} /> : null}
    {error ? <Failure failure={error} /> : null}
    {loadError && entries.length ? <Failure failure={loadError} actionLabel="Retry" onAction={retry} /> : null}
    <div className="list-filters">
      <Search aria-label="Search deployments" placeholder="Search deployments..." value={query} onChange={(event) => setQuery(event.target.value)} />
      <Select value={status} onValueChange={setStatus}>
        <SelectTrigger aria-label="Filter deployment status"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">All statuses</SelectItem>
          <SelectItem value="completed">Completed</SelectItem>
          <SelectItem value="checking">Checking</SelectItem>
          <SelectItem value="failed">Failed</SelectItem>
        </SelectContent>
      </Select>
      {filtering ? <Button size="sm" variant="ghost" onClick={clear}>Clear filters</Button> : null}
    </div>
    <div className="table-wrap" aria-busy={loading}><div className="table-scroll"><Table aria-label="Deployments">
      <TableHeader><TableRow>
        <TableHead scope="col">Deployment</TableHead>
        <TableHead scope="col">Status</TableHead>
        <TableHead scope="col" className="col-tight">Actions</TableHead>
      </TableRow></TableHeader>
      <TableBody>{loading ? [0, 1, 2].map((row) => <TableRow key={row}>
        <TableCell><div className="stack-xs"><Skeleton /><Skeleton /></div></TableCell>
        <TableCell><Skeleton /></TableCell><TableCell><Skeleton /></TableCell>
      </TableRow>) : loadError && !entries.length ? <TableRow><TableCell colSpan={3}>
        <Failure failure={loadError} actionLabel="Retry" onAction={retry} />
      </TableCell></TableRow> : !entries.length ? <TableRow><TableCell colSpan={3}>
        <EmptyState variant="first-run"><EmptyStateTitle>No deployments yet</EmptyStateTitle></EmptyState>
      </TableCell></TableRow> : !visible.length ? <TableRow><TableCell colSpan={3}>
        No deployments match your filters.
      </TableCell></TableRow> : visible.map((entry) => <TableRow key={entry.id}>
        <TableCell><div className="stack-xs">
          <strong><time dateTime={entry.created_at}>{new Date(entry.created_at).toLocaleString()}</time></strong>
          <span className="muted">{entry.revision ? <code>{entry.revision.slice(0, 12)}</code> : `${Object.keys(entry.images).length} ${Object.keys(entry.images).length === 1 ? "image" : "images"}`}</span>
        </div></TableCell>
        <TableCell><div className="stack-xs">
          <div className="row"><StatusBadge tone={deploymentTone(entry.status)} pulse={entry.status === "checking"}>{entry.status[0].toUpperCase() + entry.status.slice(1)}</StatusBadge></div>
          <span className="muted">{healthLabel(entry.readiness)}</span>
        </div></TableCell>
        <TableCell className="col-tight"><DropdownMenu>
          <DropdownMenuTrigger asChild><Button size="sm" variant="ghost" className="btn-icon" aria-label={`Actions for deployment ${new Date(entry.created_at).toLocaleString()}`}><Icon name="more" /></Button></DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => setSelected(entry.id)}>View details</DropdownMenuItem>
            {entry.recoverable ? <DropdownMenuItem disabled={busy || app.status === "pending"} onSelect={() => setRecovery(entry)}>Recover deployment</DropdownMenuItem> : null}
          </DropdownMenuContent>
        </DropdownMenu></TableCell>
      </TableRow>)}</TableBody>
    </Table></div><p className="table-footer" role="status"><span>{loading ? "Loading deployments..." : `${visible.length} of ${entries.length} deployments`}</span>{busy ? <span>Recovering...</span> : null}</p></div>
    <Dialog open={!!details} onOpenChange={(open) => { if (!open) setSelected(null); }}>
      <DialogContent><DialogHeader><DialogTitle>Deployment details</DialogTitle>
        <DialogDescription>{details ? new Date(details.created_at).toLocaleString() : ""}</DialogDescription>
      </DialogHeader><DialogBody>{details ? <>
        <KV>
          <KVKey>Status</KVKey><KVValue>{details.status}</KVValue>
          <KVKey>Health</KVKey><KVValue>{healthLabel(details.readiness)}</KVValue>
          {details.revision ? <><KVKey>Revision</KVKey><KVValue>{details.revision}</KVValue></> : null}
          {Object.entries(details.images).map(([service, image]) => <Fragment key={service}><KVKey>{service}</KVKey><KVValue>{image}</KVValue></Fragment>)}
        </KV>
        {details.error ? <Failure failure={details.error} /> : null}
      </> : null}</DialogBody><DialogFooter><Button size="sm" onClick={() => setSelected(null)}>Close</Button></DialogFooter></DialogContent>
    </Dialog>
    <AlertDialog open={recovery !== null} onOpenChange={(open) => { if (!open) setRecovery(null); }}>
      <AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Recover deployment</AlertDialogTitle>
        <AlertDialogDescription>Replace containers with these recorded images. Requests may be interrupted.</AlertDialogDescription>
      </AlertDialogHeader><DialogBody>
        <KV><KVKey>Deployment</KVKey><KVValue>{recovery ? new Date(recovery.created_at).toLocaleString() : ""}</KVValue>
          {recovery?.revision ? <><KVKey>Revision</KVKey><KVValue>{recovery.revision.slice(0, 12)}</KVValue></> : null}
        </KV>
        <p className="muted">Current variables and data stay in place. Older code must support that data. A stopped application stays stopped.</p>
      </DialogBody><AlertDialogFooter><AlertDialogCancel size="sm">Cancel</AlertDialogCancel><Button variant="primary" size="sm" disabled={busy} onClick={() => void restore()}>Recover deployment</Button></AlertDialogFooter></AlertDialogContent>
    </AlertDialog>
  </div>;
}

function DeployTrigger({ app }: { app: App }) {
  const [trigger, setTrigger] = useState<Trigger | null>(null);
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Report | null>(null);
  const path = `/apps/id/${encodeURIComponent(app.id)}/deploy-trigger`;
  useEffect(() => {
    const controller = new AbortController();
    void api(path, { signal: controller.signal }).then(async (response) => {
      if (response.ok) setTrigger(await response.json() as Trigger); else setError(await failureOf(response));
    }).catch((cause: Error) => { if (!controller.signal.aborted) setError({ error: "Could not read deploy trigger", caused_by: [cause.message] }); });
    return () => controller.abort();
  }, [path]);
  async function change(enable: boolean) {
    setBusy(true); setError(null); setToken(null);
    try {
      const response = await api(path, { method: enable ? "POST" : "DELETE", ...(enable ? { body: JSON.stringify({ branch: app.git?.git_ref }) } : {}) });
      if (!response.ok) { setError(await failureOf(response)); return; }
      if (enable) { const created = await response.json() as Trigger; setTrigger({ enabled: true, branch: created.branch }); setToken(created.token ?? null); }
      else { setTrigger({ enabled: false }); setOpen(false); }
    } catch (cause) { setError({ error: "Could not change deploy trigger", caused_by: [(cause as Error).message] }); }
    finally { setBusy(false); }
  }
  return <>
    <div className="between">
      <div className="row-wrap">
        <StatusBadge tone={trigger?.enabled ? "success" : "neutral"}>{trigger === null ? "Loading trigger" : trigger.enabled ? "Automatic deployment" : "Manual review"}</StatusBadge>
        {trigger?.enabled ? <span className="muted">{trigger.branch}</span> : null}
      </div>
      <Button size="sm" disabled={!trigger || busy} onClick={() => setOpen(true)}>{trigger?.enabled ? "Manage trigger" : "Enable trigger"}</Button>
    </div>
    {error && !open ? <Failure failure={error} /> : null}
    <Dialog open={open} onOpenChange={(next) => { if (!busy) { setOpen(next); if (!next) { setToken(null); setError(null); } } }}>
      <DialogContent><DialogHeader><DialogTitle>Deploy trigger</DialogTitle>
        <DialogDescription>This credential deploys this application without manual review.</DialogDescription>
      </DialogHeader><DialogBody>
        <KV><KVKey>Branch</KVKey><KVValue>{app.git?.git_ref}</KVValue><KVKey>Endpoint</KVKey><KVValue>{window.location.origin}/deploy/{app.id}</KVValue></KV>
        <p className="muted">Send a unique <code>delivery_id</code> and this branch with each request.</p>
        {token ? <FormField label="Bearer token" id="deploy-trigger-token" value={token} readOnly autoComplete="off" hint="Shown once. Save it before closing. Previous tokens are revoked." /> : null}
        {error ? <Failure failure={error} /> : null}
      </DialogBody><DialogFooter>
        <Button size="sm" disabled={busy} onClick={() => { setToken(null); setOpen(false); }}>Close</Button>
        {trigger?.enabled ? <Button size="sm" disabled={busy} onClick={() => void change(false)}>Disable trigger</Button> : null}
        <Button size="sm" variant="primary" disabled={busy || Boolean(app.git?.revision)} onClick={() => void change(true)}>{trigger?.enabled ? "Rotate token" : "Enable trigger"}</Button>
      </DialogFooter></DialogContent>
    </Dialog>
  </>;
}
