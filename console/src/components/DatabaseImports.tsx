import { useState, type FormEvent } from "react";
import {
  Button,
  Form,
  FormActions,
  FormField,
  PageHeader,
  PageHeaderTitle,
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
import { formatBytes } from "../lib/format.js";
import { DatabaseOperation } from "./DatabaseOperation.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";

const MAX_BYTES = 64 * 1024 * 1024;

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
  /** Uploads the archive and answers with the import's Task id, or null. */
  onImport: (connectionId: string, file: File) => Promise<string | null>;
}) {
  const [adding, setAdding] = useState(false);
  const [connection, setConnection] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  // An unread history is not an empty one: the form opens by itself only when
  // the list was read and has nothing in it.
  const formOpen = adding || (!loading && !failure && !imports.length);
  const close = () => { setAdding(false); setFile(null); };
  const selected = imports.find((item) => item.task_id === chosen) ?? imports[0] ?? null;
  const appName = (item: { consumer_application_id: string; consumer_name?: string | null }) =>
    item.consumer_name ?? apps.find((app) => app.id === item.consumer_application_id)?.name ?? item.consumer_application_id;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!file || !connection) return;
    const task = await onImport(connection, file);
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
        <FormField id="database-import" label="Backup file" type="file" accept=".dump,.backup" onChange={(event) => setFile(event.target.files?.[0] ?? null)} hint="pg_dump custom archive, up to 64 MiB." />
        <div className="field">
          <p className="muted">Make the archive with pg_dump, from the database you are moving:</p>
          <pre className="setup-command"><code>pg_dump --format=custom --no-owner --no-privileges --file backup.dump "postgresql://user:password@host:5432/database"</code></pre>
        </div>
      </div>
      <FormActions sticky>
        {imports.length ? <Button size="sm" type="button" onClick={close}>Cancel</Button> : null}
        <Button size="sm" type="submit" variant="primary" disabled={!file || file.size > MAX_BYTES || !connection || busy}>Import data</Button>
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
