import { useEffect, useRef, useState, type FormEvent } from "react";
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertTitle,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  EmptyState,
  EmptyStateTitle,
  FileDropzone,
  FormField,
  PageHeader,
  PageHeaderTitle,
  Progress,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import type { App, DatabaseConnection, DatabaseImport, Report } from "../lib/types.js";
import { api, failureOf } from "../lib/api.js";
import { detectFormat, type ArchiveFormat } from "../lib/archiveFormat.js";
import { formatBytes } from "../lib/format.js";
import { dismissUpload, onUploadFinished, retryUpload, startUpload, useImportUploads } from "../lib/importUploads.js";
import { DatabaseOperation } from "./DatabaseOperation.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";
import { Step, Steps } from "./Steps.js";

/** Where the dump comes from: a file the browser sends, or a path the daemon reads. */
export type ImportSource = { file: File } | { path: string };

const FORMATS: { value: ArchiveFormat; label: string; command: string }[] = [
  { value: "custom", label: "Custom (pg_dump --format=custom)", command: "pg_dump --format=custom --no-owner --no-privileges --file backup.dump" },
  { value: "tar", label: "Tar (pg_dump --format=tar)", command: "pg_dump --format=tar --no-owner --no-privileges --file backup.tar" },
  { value: "directory", label: "Directory (pg_dump --format=directory)", command: "pg_dump --format=directory --no-owner --no-privileges --file backup-dir" },
  { value: "sql", label: "Plain SQL (pg_dump --format=plain)", command: "pg_dump --format=plain --no-owner --no-privileges --file backup.sql" },
];


/**
 * Every import into this database, newest first, with the steps and log of
 * the one selected. The import runs as a Task in the daemon, so the list and
 * its logs read the same whether or not this screen was open while it ran.
 */
export function DatabaseImports({ databaseId, apps, connections, imports, loading, failure, busy, onImport, onImported, onOpenConnections }: {
  databaseId: string;
  apps: App[];
  /** Connections ready to take an import. */
  connections: DatabaseConnection[];
  imports: DatabaseImport[];
  loading: boolean;
  /** Why the list could not be read; the last list read stays shown. */
  failure: Report | null;
  busy: boolean;
  /** Opens the Connections tab, where an Application is connected. */
  onOpenConnections: () => void;
  /** Imports from a Host path: its Task id, or why it was refused. */
  onImport: (connectionId: string, path: string, format: ArchiveFormat) => Promise<{ taskId: string } | { failure: Report }>;
  /** An upload became an import: read the list again. */
  onImported: () => void;
}) {
  const [adding, setAdding] = useState(false);
  const [connection, setConnection] = useState("");
  const [from, setFrom] = useState<"upload" | "path">("upload");
  const [file, setFile] = useState<File | null>(null);
  const [path, setPath] = useState("");
  const [format, setFormat] = useState<ArchiveFormat | "">("");
  const [detection, setDetection] = useState<string | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [refusal, setRefusal] = useState<Report | null>(null);
  // Each change of source takes a number; a detection answers only if its
  // number is still the latest, so a slow one cannot pick the wrong type.
  const generation = useRef(0);
  const uploads = useImportUploads(databaseId);
  const close = () => { setAdding(false); setFile(null); setPath(""); setFormat(""); setDetection(null); setRefusal(null); };
  // A finished upload is an import now: select it as the list shows it.
  useEffect(() => onUploadFinished((finished) => {
    if (finished.databaseId !== databaseId) return;
    setChosen(finished.taskId);
    onImported();
  }), [databaseId, onImported]);
  const selected = imports.find((item) => item.task_id === chosen) ?? imports[0] ?? null;
  const appName = (item: { consumer_application_id: string; consumer_name?: string | null }) =>
    item.consumer_name ?? apps.find((app) => app.id === item.consumer_application_id)?.name ?? item.consumer_application_id;
  const source: ImportSource | null = from === "upload" ? (file ? { file } : null) : (path.trim() ? { path: path.trim() } : null);
  const command = FORMATS.find((item) => item.value === format)?.command ?? FORMATS[0].command;

  // The type is read from the file and preselected; when it cannot be told,
  // nothing is, and Import waits for the Operator to choose.
  async function pickFile(next: File | null) {
    const mine = ++generation.current;
    setFile(next);
    if (!next) { setFormat(""); setDetection(null); return; }
    const detected = detectFormat(new Uint8Array(await next.slice(0, 512).arrayBuffer()));
    if (mine !== generation.current) return;
    setFormat(detected ?? "");
    setDetection(detected ? null : "Could not tell what kind of dump this is. Choose its type.");
  }

  async function detectPath() {
    const value = path.trim();
    if (!value) return;
    const mine = ++generation.current;
    try {
      const response = await api("/databases/import-format", { method: "POST", body: JSON.stringify({ path: value }) });
      if (!response.ok) throw await failureOf(response);
      const { format: detected } = await response.json() as { format: ArchiveFormat | null };
      if (mine !== generation.current) return;
      setFormat(detected ?? "");
      setDetection(detected ? null : "Could not tell what kind of dump this is. Choose its type.");
    } catch (error) {
      if (mine !== generation.current) return;
      setFormat("");
      setDetection((error as { error?: string }).error ?? "Could not read that path.");
    }
  }

  // An upload runs in the background, outside this dialog, so the dialog
  // closes at once; a Host path is queued before it closes.
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!source || !connection || !format) return;
    if ("file" in source) {
      startUpload(databaseId, connection, source.file, format);
      close();
      return;
    }
    if (submitting) return;
    setSubmitting(true);
    setRefusal(null);
    const result = await onImport(connection, source.path, format);
    setSubmitting(false);
    if ("taskId" in result) {
      setChosen(result.taskId);
      close();
    } else {
      setRefusal(result.failure);
    }
  }

  return <div className="stack">
    <PageHeader actions={<Button size="sm" variant="primary" disabled={busy || !connections.length} onClick={() => setAdding(true)}><Icon name="plus" />New import</Button>}>
      <PageHeaderTitle asChild><h2 className="t-h2">Imports</h2></PageHeaderTitle>
    </PageHeader>

    {failure ? <Failure failure={failure} /> : null}

    {!connections.length ? <Alert variant="warning">
      <Icon name="info" size="md" />
      <AlertContent>
        <AlertTitle>No Application is connected to this database</AlertTitle>
        <AlertDescription>
          An import fills a connected Application's database. Connect one first, then come back here.
          {" "}<Button size="sm" variant="ghost" type="button" onClick={onOpenConnections}>Open Connections</Button>
        </AlertDescription>
      </AlertContent>
    </Alert> : null}

    <Dialog open={adding} onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent className="import-dialog">
        <DialogHeader><DialogTitle>New import</DialogTitle></DialogHeader>
        <form onSubmit={(event: FormEvent) => void submit(event)}>
          <DialogBody>
        <Steps>
          <Step title="Where the data goes" done={Boolean(connection)}>
            <Select value={connection} onValueChange={setConnection} disabled={busy || !connections.length}>
              <FormField id="import-connection" label="Consumer database"><SelectTrigger><SelectValue placeholder="Choose a connection" /></SelectTrigger></FormField>
              <SelectContent>{connections.map((item) => <SelectItem key={item.id} value={item.id}>{appName(item)}</SelectItem>)}</SelectContent>
            </Select>
          </Step>
          <Step title="The dump" done={Boolean(source)}>
            <div className="row import-source" role="group" aria-label="Where the dump comes from">
              {(["upload", "path"] as const).map((value) => <Button key={value} size="sm" type="button" variant={from === value ? "primary" : "default"} aria-pressed={from === value} disabled={busy}
                onClick={() => { if (from === value) return; generation.current += 1; setFrom(value); setFormat(""); setDetection(null); }}>
                {value === "upload" ? "Upload" : "Path on the Host"}
              </Button>)}
            </div>
            {from === "upload"
              ? <FileDropzone id="database-import" label={file ? file.name : "Drop a dump here, or choose one"} disabled={busy}
                  description={file ? `${formatBytes(file.size)} · saved privately on the Host and removed after the import` : "Any size. It is saved privately on the Host and removed after the import."}
                  onFilesSelected={(files) => void pickFile(files[0] ?? null)} />
              : <FormField id="database-import-path" label="Path on the Host" className="mono" placeholder="/srv/backups/app.dump" value={path} onChange={(event) => { generation.current += 1; setPath(event.target.value); setFormat(""); setDetection(null); }} onBlur={() => void detectPath()} hint="A file, or a directory for a directory dump, read where it is on the machine that runs self-host. It is never moved or removed." />}
          </Step>
          <Step title="What kind of dump" done={Boolean(source && format)}>
            <Select value={format} onValueChange={(next) => { generation.current += 1; setFormat(next as ArchiveFormat); setDetection(null); }} disabled={busy || !source}>
              <FormField id="import-format" label="Dump type" hint={detection ?? (format ? "Read from the file. Change it if it is wrong." : "Chosen from the file once you pick one.")}><SelectTrigger><SelectValue placeholder="Choose the dump type" /></SelectTrigger></FormField>
              <SelectContent>{FORMATS.filter((item) => from === "path" || item.value !== "directory").map((item) => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
            </Select>
            <details className="import-howto">
              <summary className="muted">How to make the dump</summary>
              <pre className="setup-command"><code>{command} "postgresql://user:password@host:5432/database"</code></pre>
            </details>
          </Step>
        </Steps>
            {refusal ? <Failure failure={refusal} /> : null}
          </DialogBody>
          <DialogFooter>
            <Button size="sm" type="button" onClick={close}>Cancel</Button>
            <Button size="sm" type="submit" variant="primary" disabled={!source || !format || !connection || busy || submitting}>{submitting ? "Queuing…" : "Import data"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>

    {!loading && !imports.length && !uploads.length && !failure ? <EmptyState variant="first-run"><EmptyStateTitle>No imports yet</EmptyStateTitle></EmptyState> : null}

    {loading || imports.length || uploads.length ? <div className="table-wrap"><div className="table-scroll"><Table aria-label="Database imports" aria-busy={loading}>
      <TableHeader><TableRow><TableHead>When</TableHead><TableHead>Application</TableHead><TableHead>File</TableHead><TableHead>Result</TableHead><TableHead className="col-tight">Objects</TableHead></TableRow></TableHeader>
      <TableBody>{uploads.map((upload) => <TableRow key={upload.key}>
        <TableCell className="muted">Now</TableCell>
        <TableCell>{appName({ consumer_application_id: connections.find((item) => item.id === upload.connectionId)?.consumer_application_id ?? "" })}</TableCell>
        <TableCell><span className="mono">{upload.file.name}</span> <span className="muted">· {formatBytes(upload.file.size)}</span></TableCell>
        <TableCell className="import-upload">{upload.status === "uploading"
          ? <Progress label="Uploading" value={upload.loaded} max={Math.max(upload.file.size, 1)}
              valueText={`${Math.floor((upload.loaded / Math.max(upload.file.size, 1)) * 100)}% · ${formatBytes(upload.loaded)} of ${formatBytes(upload.file.size)}`} />
          : <div className="stack-xs"><span className="field-error">Upload failed: {upload.error}</span>
              <span className="row"><Button size="sm" type="button" onClick={() => retryUpload(upload.key)}>Retry</Button><Button size="sm" variant="ghost" type="button" onClick={() => dismissUpload(upload.key)}>Dismiss</Button></span></div>}</TableCell>
        <TableCell className="col-tight">—</TableCell>
      </TableRow>)}
      {loading ? <TableRow>{[0, 1, 2, 3, 4].map((cell) => <TableCell key={cell}><Skeleton /></TableCell>)}</TableRow> : imports.map((item) => {
        const status = item.task?.status ?? "unknown";
        return <TableRow key={item.task_id} aria-selected={selected?.task_id === item.task_id}>
          <TableCell>{new Date(item.created_at).toLocaleString()}</TableCell>
          <TableCell>{appName(item)}</TableCell>
          <TableCell><button type="button" className="btn btn-link" aria-pressed={selected?.task_id === item.task_id} onClick={() => setChosen(item.task_id)}><span className="mono">{item.file_name}</span></button> <span className="muted">· {formatBytes(item.file_bytes)}</span></TableCell>
          <TableCell><StatusBadge tone={status === "failed" ? "danger" : status === "completed" || status === "running" ? "success" : "neutral"} pulse={status === "running"}>{status === "completed" ? "Imported" : status === "failed" ? "Failed" : status === "running" ? "Restoring" : status === "pending" ? "Queued" : "Unknown"}</StatusBadge></TableCell>
          <TableCell className="col-tight">{item.imported_objects ?? "—"}</TableCell>
        </TableRow>;
      })}</TableBody>
    </Table></div><p className="table-footer"><span>{imports.length} {imports.length === 1 ? "import" : "imports"}{uploads.length ? `, ${uploads.length} uploading` : ""}</span></p></div>
      : null}

    {selected ? <div className="detail-run import-run">
      {selected.task ? <DatabaseOperation event={selected.task} child={null} error={null} title={`Import of ${selected.file_name}`} />
        : <p className="muted run-empty">This import's steps are no longer in the Task history.</p>}
    </div> : null}
  </div>;
}
