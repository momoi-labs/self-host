import { useRef, useState, type FormEvent } from "react";
import {
  Button,
  FileDropzone,
  Form,
  FormActions,
  FormField,
  PageHeader,
  PageHeaderTitle,
  RadioGroup,
  RadioGroupItem,
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
import { DatabaseOperation } from "./DatabaseOperation.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";

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
export function DatabaseImports({ apps, connections, imports, loading, failure, busy, onImport }: {
  apps: App[];
  /** Connections ready to take an import. */
  connections: DatabaseConnection[];
  imports: DatabaseImport[];
  loading: boolean;
  /** Why the list could not be read; the last list read stays shown. */
  failure: Report | null;
  busy: boolean;
  /** Starts the import and answers with its Task id, or null. */
  onImport: (connectionId: string, source: ImportSource, format: ArchiveFormat) => Promise<string | null>;
}) {
  const [adding, setAdding] = useState(false);
  const [connection, setConnection] = useState("");
  const [from, setFrom] = useState<"upload" | "path">("upload");
  const [file, setFile] = useState<File | null>(null);
  const [path, setPath] = useState("");
  const [format, setFormat] = useState<ArchiveFormat | "">("");
  const [detection, setDetection] = useState<string | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  // Each change of source takes a number; a detection answers only if its
  // number is still the latest, so a slow one cannot pick the wrong type.
  const generation = useRef(0);
  // An unread history is not an empty one: the form opens by itself only when
  // the list was read and has nothing in it.
  const formOpen = adding || (!loading && !failure && !imports.length);
  const close = () => { setAdding(false); setFile(null); setPath(""); setFormat(""); setDetection(null); };
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

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!source || !connection || !format) return;
    const task = await onImport(connection, source, format);
    if (task) {
      setChosen(task);
      close();
    }
  }

  return <div className="stack">
    <PageHeader actions={formOpen ? undefined : <Button size="sm" variant="primary" disabled={busy || !connections.length} onClick={() => setAdding(true)}><Icon name="plus" />New import</Button>}>
      <PageHeaderTitle asChild><h2 className="t-h2">Imports</h2></PageHeaderTitle>
    </PageHeader>

    {failure ? <Failure failure={failure} /> : null}

    {formOpen ? <Form className="import-form" onSubmit={(event: FormEvent) => void submit(event)}>
      <div className="form-body">
        <Select value={connection} onValueChange={setConnection} disabled={busy}>
          <FormField id="import-connection" label="Consumer database" hint={connections.length ? undefined : "Connect an Application first; the import fills its database."}><SelectTrigger><SelectValue placeholder="Choose a connection" /></SelectTrigger></FormField>
          <SelectContent>{connections.map((item) => <SelectItem key={item.id} value={item.id}>{appName(item)}</SelectItem>)}</SelectContent>
        </Select>
        <RadioGroup label="Dump" variant="segmented" value={from} disabled={busy} onValueChange={(next) => { generation.current += 1; setFrom(next as "upload" | "path"); setFormat(""); setDetection(null); }}>
          <RadioGroupItem value="upload" label="Upload a file" />
          <RadioGroupItem value="path" label="Path on the Host" />
        </RadioGroup>
        {from === "upload"
          ? <FileDropzone id="database-import" label={file ? file.name : "Drop a dump here, or choose one"} disabled={busy}
              description={file ? `${formatBytes(file.size)} · saved privately on the Host and removed after the import` : "Any size. It is saved privately on the Host and removed after the import."}
              onFilesSelected={(files) => void pickFile(files[0] ?? null)} />
          : <FormField id="database-import-path" label="Path on the Host" className="mono" placeholder="/srv/backups/app.dump" value={path} onChange={(event) => { generation.current += 1; setPath(event.target.value); setFormat(""); setDetection(null); }} onBlur={() => void detectPath()} hint="A file, or a directory for a directory dump, read where it is. It is never moved or removed." />}
        <Select value={format} onValueChange={(next) => { setFormat(next as ArchiveFormat); setDetection(null); }} disabled={busy || !source}>
          <FormField id="import-format" label="Dump type" hint={detection ?? (format ? "Read from the file. Change it if it is wrong." : "Chosen from the file once you pick one.")}><SelectTrigger><SelectValue placeholder="Choose the dump type" /></SelectTrigger></FormField>
          <SelectContent>{FORMATS.filter((item) => from === "path" || item.value !== "directory").map((item) => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
        </Select>
        <div className="field">
          <p className="muted">Make the dump with pg_dump, from the database you are moving:</p>
          <pre className="setup-command"><code>{command} "postgresql://user:password@host:5432/database"</code></pre>
        </div>
      </div>
      <FormActions sticky>
        {imports.length ? <Button size="sm" type="button" onClick={close}>Cancel</Button> : null}
        <Button size="sm" type="submit" variant="primary" disabled={!source || !format || !connection || busy}>Import data</Button>
      </FormActions>
    </Form> : null}

    {loading || imports.length ? <div className="table-wrap"><div className="table-scroll"><Table aria-label="Database imports" aria-busy={loading}>
      <TableHeader><TableRow><TableHead>When</TableHead><TableHead>Application</TableHead><TableHead>File</TableHead><TableHead>Result</TableHead><TableHead className="col-tight">Objects</TableHead></TableRow></TableHeader>
      <TableBody>{loading ? <TableRow>{[0, 1, 2, 3, 4].map((cell) => <TableCell key={cell}><Skeleton /></TableCell>)}</TableRow> : imports.map((item) => {
        const status = item.task?.status ?? "unknown";
        return <TableRow key={item.task_id} aria-selected={selected?.task_id === item.task_id}>
          <TableCell>{new Date(item.created_at).toLocaleString()}</TableCell>
          <TableCell>{appName(item)}</TableCell>
          <TableCell><button type="button" className="btn btn-link" aria-pressed={selected?.task_id === item.task_id} onClick={() => setChosen(item.task_id)}><span className="mono">{item.file_name}</span></button> <span className="muted">· {formatBytes(item.file_bytes)}</span></TableCell>
          <TableCell><StatusBadge tone={status === "failed" ? "danger" : status === "completed" || status === "running" ? "success" : "neutral"} pulse={status === "running"}>{status === "completed" ? "Imported" : status === "failed" ? "Failed" : status === "running" ? "Restoring" : status === "pending" ? "Queued" : "Unknown"}</StatusBadge></TableCell>
          <TableCell className="col-tight">{item.imported_objects ?? "—"}</TableCell>
        </TableRow>;
      })}</TableBody>
    </Table></div><p className="table-footer"><span>{imports.length} {imports.length === 1 ? "import" : "imports"}</span></p></div>
      : null}

    {selected ? <div className="detail-run import-run">
      {selected.task ? <DatabaseOperation event={selected.task} child={null} error={null} title={`Import of ${selected.file_name}`} />
        : <p className="muted run-empty">This import's steps are no longer in the Task history.</p>}
    </div> : null}
  </div>;
}
