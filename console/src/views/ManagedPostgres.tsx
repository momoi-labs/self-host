import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, Button, Card, Checkbox, Form, FormActions, FormField, Label, Lifecycle, PageHeader, PageHeaderTitle, StatusBadge, Tabs, TabsContent, TabsList, TabsTrigger } from "@momoi-labs/kiso-react";

import { api, asReport, failureOf } from "../lib/api.js";
import type { App, Metrics, Report } from "../lib/types.js";
import { Glance } from "../components/Glance.js";
import { seriesFor } from "../lib/useMetrics.js";
import { useMediaQuery } from "../lib/useMediaQuery.js";
import { eventActionLabel, type OperationStage, type PlatformEvent } from "../lib/platformEvents.js";
import { formatSeconds, seconds } from "../lib/runSteps.js";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@momoi-labs/kiso-react";
import { useEvents } from "../lib/useEvents.js";
import { Failure } from "../components/Failure.js";
import { AppLogPane } from "../components/LogPane.js";
import { Pane, Split, Splitter, LogView, LogViewLine, StepList } from "@momoi-labs/kiso-react";

import { DatabaseConnections, type DatabaseConnection } from "../components/DatabaseConnections.js";
type Database = { major: number; volume: string; readiness: string; connections: DatabaseConnection[] };

export function ManagedPostgres({ app, apps, metrics, reload, onRemoved }: { app: App; apps: App[]; metrics: Metrics | null; reload: () => Promise<App[]>; onRemoved: () => void }) {
  const wide = useMediaQuery("(min-width: 1024px)");
  const [database, setDatabase] = useState<Database | null>(null);
  const [failure, setFailure] = useState<Report | null>(null);
  const [loadFailure, setLoadFailure] = useState<Report | null>(null);
  const [tab, setTab] = useState(app.status === "pending" ? "last-operation" : "summary");
  const [busy, setBusy] = useState(false);
  const [importConnection, setImportConnection] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [connectionOperation, setConnectionOperation] = useState<{ id: string; previousTask: string | null } | null>(null);
  const [operationLabel, setOperationLabel] = useState(app.status === "pending" ? "Provisioning" : "Operation");
  const [taskId, setTaskId] = useState<string | null>(null);
  const handledTasks = useRef(new Set<string>());
  const [removing, setRemoving] = useState(false);
  const [confirmName, setConfirmName] = useState("");
  const [deleteData, setDeleteData] = useState(false);
  const { events, error: eventsError, reload: reloadEvents } = useEvents();
  const acceptedTask = taskId ? events.find((event) => event.id === taskId) ?? null : events.find((event) => event.subject.id === app.id && event.progress) ?? events.find((event) => event.subject.id === app.id) ?? null;
  const connectionTaskId = database?.connections.find((connection) => connection.id === connectionOperation?.id)?.task_id;
  const currentTask = acceptedTask?.status === "completed" && connectionTaskId && connectionTaskId !== connectionOperation?.previousTask ? events.find((event) => event.id === connectionTaskId) ?? acceptedTask : acceptedTask;
  const taskBusy = Boolean(taskId && !acceptedTask) || currentTask?.status === "pending" || currentTask?.status === "running";
  const phase = currentTask?.progress?.stages.find((stage) => stage.status === "running")?.label ?? (currentTask !== acceptedTask ? "Applying variable" : operationLabel);

  const load = useCallback(async () => {
    try {
      const response = await api(`/databases/${app.id}`);
      if (!response.ok) throw await failureOf(response);
      setDatabase(await response.json() as Database);
      setLoadFailure(null);
    } catch (error) { setLoadFailure(asReport(error)); }
  }, [app.id]);
  useEffect(() => { void load(); const timer = setInterval(() => { void load(); }, 3000); return () => clearInterval(timer); }, [load]);
  useEffect(() => { if (!taskId || !currentTask || taskBusy || handledTasks.current.has(currentTask.id)) return; handledTasks.current.add(currentTask.id); void reload().then((next) => { if (!next.some((candidate) => candidate.id === app.id)) onRemoved(); }); }, [currentTask, taskId, taskBusy, app.id, reload, onRemoved]);

  async function operation(path: string, method: string, body?: object | File) {
    if (busy || taskBusy) return false;
    setBusy(true); setFailure(null);
    try {
      const response = await api(path, { method, body: body instanceof File ? body : body ? JSON.stringify(body) : undefined, headers: body instanceof File ? { "Content-Type": "application/octet-stream" } : undefined });
      if (!response.ok) throw await failureOf(response);
      const accepted = await response.json() as { task_id: string; connection_id?: string };
      const connectionId = accepted.connection_id ?? (method === "DELETE" ? path.split("/connections/")[1] : undefined);
      setConnectionOperation(connectionId ? { id: connectionId, previousTask: database?.connections.find((connection) => connection.id === connectionId)?.task_id ?? null } : null);
      setOperationLabel(path.endsWith("/import") ? "Importing" : path.includes("/connections") ? method === "DELETE" ? "Disconnecting" : "Connecting" : path.endsWith("/stop") ? "Stopping" : path.endsWith("/start") ? "Starting" : path.endsWith("/restart") ? "Restarting" : path.endsWith("/redeploy") ? "Recreating" : "Removing");
      setTaskId(accepted.task_id); setTab("last-operation"); reloadEvents();
      await load(); await reload();
      return true;
    } catch (error) { setFailure(asReport(error)); return false; }
    finally { setBusy(false); }
  }

  const samples = seriesFor(metrics, app.id);
  const active = database?.connections.filter((connection) => connection.status !== "revoked") ?? [];
  const ready = active.filter((connection) => connection.status === "ready");
  const selectedImport = ready.find((connection) => connection.id === importConnection);
  const importStopped = apps.find((candidate) => candidate.id === selectedImport?.consumer_application_id)?.status === "stopped";

  return <>
    <div className="between">
      <PageHeader className={wide ? "grow" : undefined}><PageHeaderTitle>{app.name}</PageHeaderTitle>{samples ? <Glance samples={samples} /> : null}</PageHeader>
      <Lifecycle status={<><StatusBadge tone={app.status === "running" ? "success" : app.status === "failed" ? "danger" : "neutral"}>{app.status}</StatusBadge><StatusBadge tone={database?.readiness === "healthy" ? "success" : database?.readiness === "unhealthy" ? "danger" : "neutral"}>{database?.readiness === "healthy" ? "Ready" : database?.readiness ?? "Checking"}</StatusBadge>{taskBusy ? <StatusBadge tone="success" pulse>{phase}</StatusBadge> : null}</>}
        actions={taskBusy ? undefined : <>{app.status !== "running" ? <Button size="sm" disabled={busy || taskBusy} onClick={() => void operation(`/apps/id/${app.id}/start`, "POST")}>Start</Button> : <Button size="sm" disabled={busy || taskBusy} onClick={() => void operation(`/apps/id/${app.id}/stop`, "POST")}>Stop</Button>}<Button size="sm" disabled={busy || taskBusy} onClick={() => void operation(`/apps/id/${app.id}/restart`, "POST", { pull: false })}>Restart</Button><Button size="sm" disabled={busy || taskBusy} onClick={() => void operation(`/databases/${app.id}/redeploy`, "POST")}>Recreate</Button></>}
        destructive={<Button size="sm" variant="ghost" className="btn-danger-ghost" disabled={busy || taskBusy} onClick={() => { setConfirmName(""); setDeleteData(false); setRemoving(true); }}>Remove</Button>} />
    </div>
    {failure ? <Failure failure={failure} /> : null}
    {loadFailure ? <Failure failure={loadFailure} /> : null}
    {app.last_error ? <Failure failure={app.last_error} /> : null}
    <Card className="detail-tabs"><Tabs value={tab} onValueChange={setTab}>
      <TabsList aria-label="Database details"><TabsTrigger value="summary">Summary</TabsTrigger><TabsTrigger value="connections">Connections</TabsTrigger><TabsTrigger value="import">Import</TabsTrigger><TabsTrigger value="last-operation">Last operation</TabsTrigger><TabsTrigger value="logs">Logs</TabsTrigger></TabsList>
      <TabsContent value="summary"><dl className="summary-facts"><dt>Version</dt><dd>PostgreSQL {database?.major ?? app.managed_postgres?.major}</dd><dt>Storage</dt><dd><code>{database?.volume ?? "Loading"}</code></dd><dt>Access</dt><dd>Private connections only</dd><dt>Consumers</dt><dd>{active.length}</dd></dl></TabsContent>
      <TabsContent value="connections">
        <DatabaseConnections app={app} apps={apps} connections={active} loading={database === null} busy={busy || taskBusy} failure={failure} onOperation={operation} />
      </TabsContent>
      <TabsContent value="import">
        <Form onSubmit={(event: FormEvent) => { event.preventDefault(); if (file) void operation(`/databases/${app.id}/connections/${importConnection}/import`, "POST", file); }}>
          <div className="form-body">
            <Select value={importConnection} onValueChange={setImportConnection} disabled={busy || taskBusy}>
              <FormField id="import-connection" label="Consumer database"><SelectTrigger><SelectValue placeholder="Choose a connection" /></SelectTrigger></FormField>
              <SelectContent>{ready.map((connection) => <SelectItem key={connection.id} value={connection.id}>{apps.find((candidate) => candidate.id === connection.consumer_application_id)?.name ?? connection.database}</SelectItem>)}</SelectContent>
            </Select>
            <FormField id="database-import" label="Backup file" type="file" accept=".dump,.backup" onChange={(event) => setFile(event.target.files?.[0] ?? null)} hint="pg_dump custom archive, up to 64 MiB. Target database must be empty." />
            {selectedImport && !importStopped ? <p className="muted">Stop the selected application to import.</p> : null}
            {selectedImport?.imported_objects != null ? <p className="muted">Last import: {selectedImport.imported_objects} objects.</p> : null}
          </div>
          <FormActions sticky><Button size="sm" type="submit" variant="primary" disabled={!file || file.size > 64 * 1024 * 1024 || !importConnection || !importStopped || busy || taskBusy}>Import data</Button></FormActions>
        </Form>
      </TabsContent>
      <TabsContent value="last-operation" className="detail-run"><DatabaseOperation event={acceptedTask} child={currentTask !== acceptedTask ? currentTask : null} error={eventsError} /></TabsContent>
      <TabsContent value="logs" className="detail-logs"><div className="detail-log-pane"><AppLogPane id={app.id} /></div></TabsContent>
    </Tabs></Card>
    <AlertDialog open={removing} onOpenChange={setRemoving}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Remove database</AlertDialogTitle></AlertDialogHeader><div className="dialog-body stack"><AlertDialogDescription>{active.length ? "Disconnect all consumers before removing this database." : "The container will be removed. Keep its data unless you explicitly choose deletion below."}</AlertDialogDescription><FormField id="confirm-database-name" label="Type the database name" value={confirmName} onChange={(event) => setConfirmName(event.target.value)} /><div className="check"><Checkbox id="delete-database-data" checked={deleteData} onCheckedChange={(checked) => setDeleteData(checked === true)} /><Label htmlFor="delete-database-data">Permanently delete the stored database data</Label></div></div><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><AlertDialogAction className="btn-danger" disabled={active.length > 0 || confirmName !== app.name || busy} onClick={() => { setRemoving(false); void operation(`/databases/${app.id}`, "DELETE", { confirm_name: confirmName, delete_data: deleteData }); }}>Remove database</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </>;
}


function DatabaseOperation({ event, child, error }: { event: PlatformEvent | null; child: PlatformEvent | null; error: string | null }) {
  const wide = useMediaQuery("(min-width: 1024px)");
  const [chosen, setChosen] = useState<string | null>(null);
  useEffect(() => setChosen(null), [event?.id]);
  if (!event) return <p className="muted run-empty">{error ?? "Waiting for operation details."}</p>;
  const stages: (Omit<OperationStage, "status"> & { status: PlatformEvent["status"] })[] = event.progress?.stages.length ? [...event.progress.stages] : [{
    id: event.id, label: eventActionLabel(event), status: event.status,
    startedAt: event.startedAt ?? event.occurredAt, finishedAt: event.finishedAt,
    output: [event.description], error: event.error,
  }];
  if (child) stages.push({ id: child.id, label: "Apply application variable", status: child.status, startedAt: child.startedAt ?? child.occurredAt, finishedAt: child.finishedAt, output: [child.description], error: child.error });
  const active = stages.find((stage) => stage.status === "failed") ?? stages.find((stage) => stage.status === "running");
  const shown = stages.find((stage) => stage.id === chosen) ?? active ?? stages[stages.length - 1];
  const output = <LogView aria-label={`${shown.label} result`} follow={shown.status === "running"}>
    <LogViewLine className="log-time">Started {new Date(shown.startedAt).toLocaleString()}</LogViewLine>
    {shown.output.map((line, index) => <LogViewLine key={index}>{line}</LogViewLine>)}
    {shown.error ? <><LogViewLine className="log-error">{shown.error.error}</LogViewLine>{shown.error.caused_by.map((cause, index) => <LogViewLine key={index} className="log-error">{cause}</LogViewLine>)}</> : null}
  </LogView>;
  const list = <StepList label="Database operation" selected={shown.id} onSelect={setChosen} steps={stages.map((stage) => ({
    key: stage.id, label: stage.label,
    state: stage.status === "completed" ? "done" : stage.status === "failed" ? "failed" : stage.status === "running" ? "running" : "pending",
    meta: stage.finishedAt ? formatSeconds(seconds(stage.startedAt, stage.finishedAt)) : undefined,
    detail: !wide && stage.id === shown.id ? output : undefined,
  }))} />;
  return <div className="run">
    <div className="run-head"><p className="run-title">{eventActionLabel(event)} <StatusBadge tone={(child ?? event).status === "failed" ? "danger" : ["completed", "running"].includes((child ?? event).status) ? "success" : "neutral"} pulse={(child ?? event).status === "running"}>{(child ?? event).status === "running" ? active?.label ?? "Running" : (child ?? event).status}</StatusBadge></p></div>
    {error ? <p className="muted">{error}</p> : null}
    {wide ? <Split className="run-split"><Pane className="run-steps">{list}</Pane><Splitter defaultSize={36} aria-label="Resize the steps and output panes" /><Pane className="grow run-output">{output}</Pane></Split> : <div className="run-list">{list}</div>}
  </div>;
}
