import { Fragment, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
  Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
  EmptyState, EmptyStateTitle, FormField, KV, KVKey, KVValue,
  Search, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Skeleton, StatusBadge,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";
import { api, asReport, failureOf } from "../lib/api.js";
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

const noDeployments: Deployment[] = [];

/** A read that never reached the Host says what it was reading. */
async function read<T>(path: string, failure: string, signal: AbortSignal): Promise<T> {
  let response: Response;
  try {
    response = await api(path, { signal });
  } catch (cause) {
    const report: Report = { error: failure, caused_by: [(cause as Error).message] };
    throw report;
  }
  if (!response.ok) throw await failureOf(response);
  return await response.json() as T;
}

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
  const path = `/apps/id/${encodeURIComponent(app.id)}`;
  const deployments = useQuery({
    queryKey: ["apps", app.id, "deployments"],
    queryFn: ({ signal }) => read<Deployment[]>(`${path}/deployments`, "Could not read deployment history", signal),
    refetchInterval: 3000,
  });
  const entries = deployments.data ?? noDeployments;
  const loadError = deployments.error ? asReport(deployments.error) : null;
  const loading = deployments.isPending;
  const [error, setError] = useState<Report | null>(null);
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [selected, setSelected] = useState<string | null>(null);
  const [recovery, setRecovery] = useState<Deployment | null>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const notify = useToast();
  const details = entries.find((entry) => entry.id === selected);
  const filtering = query.trim() !== "" || status !== "all";
  const visible = entries.filter((entry) => {
    const searchable = [entry.created_at, new Date(entry.created_at).toLocaleString(), entry.revision, ...Object.keys(entry.images), ...Object.values(entry.images)].join(" ").toLowerCase();
    return searchable.includes(query.trim().toLowerCase()) && (status === "all" || entry.status === status);
  });
  const clear = () => { setQuery(""); setStatus("all"); };
  const retry = () => void deployments.refetch();

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
    finally { inFlight.current = false; setBusy(false); void deployments.refetch(); await reload(); }
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
  const queryClient = useQueryClient();
  const path = `/apps/id/${encodeURIComponent(app.id)}/deploy-trigger`;
  const triggerKey = ["apps", app.id, "deploy-trigger"];
  const triggerQuery = useQuery({
    queryKey: triggerKey,
    queryFn: ({ signal }) => read<Trigger>(path, "Could not read deploy trigger", signal),
  });
  const trigger = triggerQuery.data ?? null;
  const setTrigger = (next: Trigger) => queryClient.setQueryData(triggerKey, next);
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [changeError, setChangeError] = useState<Report | null>(null);
  const error = changeError ?? (triggerQuery.error ? asReport(triggerQuery.error) : null);
  async function change(enable: boolean) {
    setBusy(true); setChangeError(null); setToken(null);
    try {
      const response = await api(path, { method: enable ? "POST" : "DELETE", ...(enable ? { body: JSON.stringify({ branch: app.git?.git_ref }) } : {}) });
      if (!response.ok) { setChangeError(await failureOf(response)); return; }
      if (enable) { const created = await response.json() as Trigger; setTrigger({ enabled: true, branch: created.branch }); setToken(created.token ?? null); }
      else { setTrigger({ enabled: false }); setOpen(false); }
    } catch (cause) { setChangeError({ error: "Could not change deploy trigger", caused_by: [(cause as Error).message] }); }
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
    <Dialog open={open} onOpenChange={(next) => { if (!busy) { setOpen(next); if (!next) { setToken(null); setChangeError(null); } } }}>
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
