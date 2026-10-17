import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, Button, Card, Checkbox, FormField, Label, Lifecycle, PageHeader, PageHeaderTitle, StatusBadge, Tabs, TabsContent, TabsList, TabsTrigger } from "@momoi-labs/kiso-react";

import { api, asReport, failureOf } from "../lib/api.js";
import { databaseImportsQuery, databaseQuery } from "../lib/queries.js";
import type { App, Metrics, Report } from "../lib/types.js";
import { Glance } from "../components/Glance.js";
import { seriesFor } from "../lib/useMetrics.js";
import { useEvents } from "../lib/useEvents.js";
import { Failure } from "../components/Failure.js";
import { AppLogPane } from "../components/LogPane.js";

import { DatabaseConnections } from "../components/DatabaseConnections.js";
import { DatabaseImports } from "../components/DatabaseImports.js";
import { DatabaseOperation } from "../components/DatabaseOperation.js";
import { DatabaseSummary } from "../components/DatabaseSummary.js";

export function ManagedPostgres({ app, apps, metrics, reload, onRemoved, onOpenApp }: { app: App; apps: App[]; metrics: Metrics | null; reload: () => Promise<App[]>; onRemoved: () => void; onOpenApp: (id: string) => void }) {
  const detail = useQuery({ ...databaseQuery(app.id), refetchInterval: 3000 });
  const imports = useQuery({ ...databaseImportsQuery(app.id), refetchInterval: 3000 });
  const database = detail.data ?? null;
  const loadFailure = detail.error ? asReport(detail.error) : null;
  const { refetch: load } = detail;
  const [failure, setFailure] = useState<Report | null>(null);
  const [tab, setTab] = useState(app.status === "pending" ? "last-operation" : "summary");
  const [busy, setBusy] = useState(false);
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

  useEffect(() => { if (!taskId || !currentTask || taskBusy || handledTasks.current.has(currentTask.id)) return; handledTasks.current.add(currentTask.id); void reload().then((next) => { if (!next.some((candidate) => candidate.id === app.id)) onRemoved(); }); }, [currentTask, taskId, taskBusy, app.id, reload, onRemoved]);

  async function operation(path: string, method: string, body?: object | File, extraHeaders?: Record<string, string>): Promise<string | false> {
    if (busy || taskBusy) return false;
    setBusy(true); setFailure(null);
    try {
      const response = await api(path, { method, body: body instanceof File ? body : body ? JSON.stringify(body) : undefined, headers: body instanceof File ? { "Content-Type": "application/octet-stream", "X-Upload-Name": encodeURIComponent(body.name), ...extraHeaders } : undefined });
      if (!response.ok) throw await failureOf(response);
      const accepted = await response.json() as { task_id: string; connection_id?: string };
      const connectionId = accepted.connection_id ?? (method === "DELETE" ? path.split("/connections/")[1] : undefined);
      setConnectionOperation(connectionId ? { id: connectionId, previousTask: database?.connections.find((connection) => connection.id === connectionId)?.task_id ?? null } : null);
      setOperationLabel(path.endsWith("/import") || path.endsWith("/import-path") ? "Importing" : path.includes("/connections") ? method === "DELETE" ? "Disconnecting" : "Connecting" : path.endsWith("/stop") ? "Stopping" : path.endsWith("/start") ? "Starting" : path.endsWith("/restart") ? "Restarting" : path.endsWith("/redeploy") ? "Recreating" : "Removing");
      // An import's steps show on the Imports tab, next to the list it joins.
      setTaskId(accepted.task_id); if (!path.endsWith("/import") && !path.endsWith("/import-path")) setTab("last-operation"); reloadEvents();
      await load(); await reload();
      return accepted.task_id;
    } catch (error) { setFailure(asReport(error)); return false; }
    finally { setBusy(false); }
  }

  const samples = seriesFor(metrics, app.id);
  const active = database?.connections.filter((connection) => connection.status !== "revoked") ?? [];
  const ready = active.filter((connection) => connection.status === "ready");

  return <>
    <div className="between">
      <PageHeader><PageHeaderTitle>{app.name}</PageHeaderTitle>{samples ? <Glance samples={samples} /> : null}</PageHeader>
      <Lifecycle status={<><StatusBadge tone={app.status === "running" ? "success" : app.status === "failed" ? "danger" : "neutral"}>{app.status}</StatusBadge><StatusBadge tone={database?.readiness === "healthy" ? "success" : database?.readiness === "unhealthy" ? "danger" : "neutral"}>{database?.readiness === "healthy" ? "Ready" : database?.readiness ?? "Checking"}</StatusBadge>{taskBusy ? <StatusBadge tone="success" pulse>{phase}</StatusBadge> : null}</>}
        actions={taskBusy ? undefined : <>{app.status !== "running" ? <Button size="sm" disabled={busy || taskBusy} onClick={() => void operation(`/apps/id/${app.id}/start`, "POST")}>Start</Button> : <Button size="sm" disabled={busy || taskBusy} onClick={() => void operation(`/apps/id/${app.id}/stop`, "POST")}>Stop</Button>}<Button size="sm" disabled={busy || taskBusy} onClick={() => void operation(`/apps/id/${app.id}/restart`, "POST", { pull: false })}>Restart</Button><Button size="sm" disabled={busy || taskBusy} onClick={() => void operation(`/databases/${app.id}/redeploy`, "POST")}>Recreate</Button></>}
        destructive={<Button size="sm" variant="ghost" className="btn-danger-ghost" disabled={busy || taskBusy} onClick={() => { setConfirmName(""); setDeleteData(false); setRemoving(true); }}>Remove</Button>} />
    </div>
    {failure ? <Failure failure={failure} /> : null}
    {loadFailure ? <Failure failure={loadFailure} /> : null}
    {app.last_error ? <Failure failure={app.last_error} /> : null}
    <Card className="detail-tabs"><Tabs sections value={tab} onValueChange={setTab}>
      <TabsList aria-label="Database details"><TabsTrigger value="summary">Summary</TabsTrigger><TabsTrigger value="connections">Connections</TabsTrigger><TabsTrigger value="imports">Imports</TabsTrigger><TabsTrigger value="last-operation">Last operation</TabsTrigger><TabsTrigger value="logs">Logs</TabsTrigger></TabsList>
      <TabsContent value="summary"><DatabaseSummary app={app} apps={apps} database={database} onOpenApp={onOpenApp} onOpenConnections={() => setTab("connections")} /></TabsContent>
      <TabsContent value="connections">
        <DatabaseConnections app={app} apps={apps} connections={active} loading={database === null} busy={busy || taskBusy} failure={failure} onOperation={async (path, method, body) => Boolean(await operation(path, method, body))} />
      </TabsContent>
      <TabsContent value="imports">
        <DatabaseImports apps={apps} onOpenConnections={() => setTab("connections")} connections={ready} imports={imports.data ?? []} loading={imports.isPending && !imports.error} failure={imports.error ? asReport(imports.error) : null} busy={busy || taskBusy}
          onImport={async (connectionId, source, format) => {
            const accepted = "file" in source
              ? await operation(`/databases/${app.id}/connections/${connectionId}/import`, "POST", source.file, { "X-Import-Format": format })
              : await operation(`/databases/${app.id}/connections/${connectionId}/import-path`, "POST", { path: source.path, format });
            await imports.refetch();
            return accepted || null;
          }} />
      </TabsContent>
      <TabsContent value="last-operation" className="detail-run"><DatabaseOperation event={acceptedTask} child={currentTask !== acceptedTask ? currentTask : null} error={eventsError} /></TabsContent>
      <TabsContent value="logs" className="detail-logs"><div className="detail-log-pane"><AppLogPane id={app.id} /></div></TabsContent>
    </Tabs></Card>
    <AlertDialog open={removing} onOpenChange={setRemoving}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Remove database</AlertDialogTitle></AlertDialogHeader><div className="dialog-body stack"><AlertDialogDescription>{active.length ? "Disconnect all consumers before removing this database." : "The container will be removed. Keep its data unless you explicitly choose deletion below."}</AlertDialogDescription><FormField id="confirm-database-name" label="Type the database name" value={confirmName} onChange={(event) => setConfirmName(event.target.value)} /><div className="check"><Checkbox id="delete-database-data" checked={deleteData} onCheckedChange={(checked) => setDeleteData(checked === true)} /><Label htmlFor="delete-database-data">Permanently delete the stored database data</Label></div></div><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><AlertDialogAction className="btn-danger" disabled={active.length > 0 || confirmName !== app.name || busy} onClick={() => { setRemoving(false); void operation(`/databases/${app.id}`, "DELETE", { confirm_name: confirmName, delete_data: deleteData }); }}>Remove database</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </>;
}
