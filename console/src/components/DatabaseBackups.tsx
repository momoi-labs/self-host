import { useEffect, useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertTitle,
  Button,
  Checkbox,
  EmptyState,
  EmptyStateTitle,
  Form,
  FormActions,
  FormField,
  Input,
  KV,
  KVKey,
  KVValue,
  Label,
  PageHeader,
  PageHeaderTitle,
  Skeleton,
  StatusBadge,
  Table, TableFrame,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { api, asReport, failureOf } from "../lib/api.js";
import { formatAgo, formatBytes } from "../lib/format.js";
import { backupSettingsQuery, databaseBackupsQuery } from "../lib/queries.js";
import type { DatabaseBackup, Report } from "../lib/types.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";
import { RestoreDialog } from "./RestoreDialog.js";

const when = (iso: string) => new Date(iso).toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
const kinds: Record<DatabaseBackup["kind"], string> = { full: "Full", diff: "Differential", incr: "Incremental" };

/**
 * The Backups tab of a database server: whether its backups are on and what
 * its repository holds. Turning backups on or off, a backup, a restore and a
 * verification each run as a Task, and the Last operation tab shows its steps.
 */
export function DatabaseBackups({ databaseId, databaseName, onStarted, onOpenOperation, onRestoring }: {
  databaseId: string;
  databaseName: string;
  /** A Task was queued here, or picked to look at: Last operation shows it. */
  onStarted: (taskId: string, title: string) => void;
  /** Opens the Last operation tab. */
  onOpenOperation: () => void;
  /** A restore was queued. Its Task belongs to the new server, so the
   *  console opens that server, the way it does after creating one. */
  onRestoring: (id: string, name: string) => void;
}) {
  const backups = useQuery({ ...databaseBackupsQuery(databaseId), refetchInterval: 5000 });
  const settings = useQuery({ ...backupSettingsQuery(databaseId), refetchInterval: 5000 });
  const list = backups.data ?? [];
  const saved = settings.data;
  // A problem may end with lines to paste, such as Dockerfile instructions.
  const [problemText, ...problemLines] = (saved?.problem ?? "").split("\n");
  const problemCode = problemLines.join("\n").trim();
  const [draft, setDraft] = useState({ enabled: false, keepDays: "14" });
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Report | null>(null);
  const [restoring, setRestoring] = useState(false);

  useEffect(() => {
    if (saved && !dirty) setDraft({ enabled: saved.enabled, keepDays: String(saved.keep_days) });
  }, [saved, dirty]);

  const keepDays = Number(draft.keepDays);
  const keepError = draft.keepDays && !(Number.isInteger(keepDays) && keepDays >= 1) ? "Keep backups for at least 1 day." : undefined;
  const noStorage = saved ? !saved.storage_configured : false;
  const changing = saved?.change ?? null;
  const ready = Boolean(saved?.enabled) && !changing && !noStorage;
  // Restore and Verify read the bucket, so they work with backups off.
  const readable = Boolean(saved) && !changing && !noStorage && list.length > 0;
  const change = (next: Partial<typeof draft>) => { setDraft({ ...draft, ...next }); setDirty(true); };

  async function save(event: FormEvent) {
    event.preventDefault();
    if (saving || keepError || !draft.keepDays) return;
    setSaving(true); setFailure(null);
    try {
      const response = await api(`/databases/${databaseId}/backups/settings`, { method: "PUT", body: JSON.stringify({ enabled: draft.enabled, keep_days: keepDays }) });
      if (!response.ok) throw await failureOf(response);
      const answer = await response.json() as { task_id?: string };
      if (answer.task_id) { onStarted(answer.task_id, draft.enabled ? "Turning backups on" : "Turning backups off"); onOpenOperation(); }
      setDirty(false);
      await settings.refetch();
    } catch (error) { setFailure(asReport(error)); }
    finally { setSaving(false); }
  }

  /** Queues a Task and hands it to the Last operation tab. */
  async function start(path: string, title: string): Promise<Report | { task_id: string }> {
    const response = await api(path, { method: "POST" });
    if (!response.ok) return failureOf(response);
    const accepted = await response.json() as { task_id: string };
    onStarted(accepted.task_id, title);
    void settings.refetch();
    return accepted;
  }

  async function run(path: string, title: string) {
    if (busy) return;
    setBusy(true); setFailure(null);
    try { const started = await start(path, title); if ("task_id" in started) onOpenOperation(); else setFailure(started); }
    catch (error) { setFailure(asReport(error)); }
    finally { setBusy(false); }
  }

  const loading = backups.isPending && !backups.error;
  const verification = saved?.verification ?? null;
  const message = dirty ? (draft.enabled !== saved?.enabled ? <><strong>Unsaved changes.</strong> Saving restarts PostgreSQL once.</> : "Unsaved changes.")
    : !saved ? "Loading..."
    : changing ? (changing.enable ? "Turning backups on..." : "Turning backups off...")
    : saved.enabled ? "Backups are on." : "Backups are off.";

  return <div className="stack">
    <PageHeader actions={<>
      <Button size="sm" disabled={busy || !readable} onClick={() => setRestoring(true)}>Restore to a new server</Button>
      <Button size="sm" disabled={busy || !readable || verification?.status === "running"} onClick={() => void run(`/databases/${databaseId}/backups/verify`, "Verifying the latest backup")}>{verification?.status === "running" ? "Verifying..." : "Verify latest backup"}</Button>
      <Button size="sm" variant="primary" disabled={busy || !ready || Boolean(saved?.running)} onClick={() => void run(`/databases/${databaseId}/backups`, "Backing up")}>{saved?.running ? "Backing up..." : <><Icon name="plus" />Back up now</>}</Button>
    </>}>
      <PageHeaderTitle asChild><h2 className="t-h2">Backups</h2></PageHeaderTitle>
    </PageHeader>

    {failure ? <Failure failure={failure} /> : null}
    {backups.error ? <Failure failure={asReport(backups.error)} /> : null}
    {settings.error ? <Failure failure={asReport(settings.error)} /> : null}

    {saved?.problem ? <Alert variant="error" role="alert">
      <Icon name="info" size="md" />
      <AlertContent>
        <AlertTitle>Backups need attention</AlertTitle>
        <AlertDescription>{problemText}</AlertDescription>
        {problemCode ? <pre className="setup-command"><code>{problemCode}</code></pre> : null}
      </AlertContent>
    </Alert> : null}

    {noStorage ? <Alert variant="info">
      <Icon name="info" size="md" />
      <AlertContent>
        <AlertTitle>Set up backup storage first</AlertTitle>
        <AlertDescription>
          Backups go to one S3-compatible bucket, saved in Settings.
          {" "}<a href="/console/#settings/backup-storage">Open Backup storage</a>
        </AlertDescription>
      </AlertContent>
    </Alert> : null}

    <Form onSubmit={(event: FormEvent) => void save(event)}>
      <div className="form-body">
        <p className="muted t-label">Backups hold the whole server and every change since, so you can restore it to any moment in the days you keep. A full backup runs on Sunday, a differential on Wednesday and Saturday, and an incremental on the other days, in the Host's maintenance window.</p>
        <div className="field">
          <div className="check">
            <Checkbox id="backup-enabled" checked={draft.enabled} disabled={!saved || saving || Boolean(changing) || (noStorage && !saved.enabled)} aria-describedby="backup-enabled-help" onCheckedChange={(checked) => change({ enabled: checked === true })} />
            <Label htmlFor="backup-enabled">Back up this server</Label>
          </div>
          <small className="field-hint" id="backup-enabled-help">Turning backups on or off restarts PostgreSQL once. Turning them on also takes the first full backup.</small>
        </div>
        <FormField id="backup-keep-days" label="Days to keep" error={keepError} hint="How far back a restore can go. Older backups are deleted after each backup.">
          <Input id="backup-keep-days" type="number" min={1} step={1} inputMode="numeric" value={draft.keepDays} disabled={!saved || saving || Boolean(changing)} onChange={(event) => change({ keepDays: event.target.value })} />
        </FormField>
        {saved?.enabled ? <KV>
          <KVKey>Next backup</KVKey><KVValue>{saved.next_run_at && saved.next_kind ? `${kinds[saved.next_kind]}, ${when(saved.next_run_at)}` : "Not scheduled"}</KVValue>
          <KVKey>Last change sent to the bucket</KVKey><KVValue title={saved.archiving.last_archived_at ? new Date(saved.archiving.last_archived_at).toLocaleString() : undefined}>
            <span className="row">
              {saved.archiving.failing ? <StatusBadge tone="danger">Failing</StatusBadge> : null}
              <span>{saved.archiving.last_archived_at ? formatAgo(saved.archiving.last_archived_at) : "Nothing yet"}</span>
            </span>
          </KVValue>
          <KVKey>Last verification</KVKey><KVValue title={verification?.error ?? undefined}>{verification
            ? <span className="row">
                <button type="button" className="btn btn-link" aria-label="Show the steps of the last verification" onClick={() => { onStarted(verification.task_id, "Verifying the latest backup"); onOpenOperation(); }}>
                  <StatusBadge tone={verification.status === "failed" ? "danger" : "success"} pulse={verification.status === "running"}>{verification.status === "passed" ? "Passed" : verification.status === "failed" ? "Failed" : "Verifying"}</StatusBadge>
                </button>
                {verification.status === "running" ? null : <span>{formatAgo(verification.at)}</span>}
              </span>
            : <span className="muted">Not verified</span>}</KVValue>
        </KV> : null}
      </div>
      <FormActions tone={dirty ? "warning" : "neutral"} message={message}>
        {dirty ? <Button size="sm" type="button" disabled={saving} onClick={() => setDirty(false)}>Discard</Button> : null}
        <Button size="sm" variant="primary" type="submit" disabled={!dirty || saving || Boolean(keepError) || !draft.keepDays}>{saving ? "Saving..." : "Save"}</Button>
      </FormActions>
    </Form>

    {!loading && !list.length && !backups.error ? <EmptyState variant="first-run"><EmptyStateTitle>No backups yet</EmptyStateTitle></EmptyState> : null}

    {loading || list.length ? <TableFrame><div className="table-scroll"><Table aria-label="Backups of this server" aria-busy={loading}>
      <TableHeader><TableRow>
        <TableHead scope="col">Ended</TableHead>
        <TableHead scope="col">Type</TableHead>
        <TableHead scope="col">Server size</TableHead>
        <TableHead scope="col">Added to the bucket</TableHead>
      </TableRow></TableHeader>
      <TableBody>{loading ? <TableRow>{[0, 1, 2, 3].map((cell) => <TableCell key={cell}><Skeleton /></TableCell>)}</TableRow> : list.map((backup) =>
        <TableRow key={backup.label}>
          <TableCell title={backup.label}>{when(backup.finished_at)}</TableCell>
          <TableCell>{kinds[backup.kind]}</TableCell>
          <TableCell>{formatBytes(backup.bytes)}</TableCell>
          <TableCell>{formatBytes(backup.repository_bytes)}</TableCell>
        </TableRow>)}</TableBody>
    </Table></div><p className="table-footer" role="status"><span>{loading ? "Loading backups..." : `${list.length} ${list.length === 1 ? "backup" : "backups"}`}</span></p></TableFrame> : null}

    <RestoreDialog open={restoring} sources={[{ id: databaseId, name: databaseName }]} onClose={() => setRestoring(false)} onRestoring={onRestoring} />
  </div>;
}
