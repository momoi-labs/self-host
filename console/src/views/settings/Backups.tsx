import { useEffect, useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Alert,
  AlertDescription,
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
  Checkbox,
  Form,
  FormActions,
  FormField,
  GridPane,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@momoi-labs/kiso-react";

import { Failure } from "../../components/Failure.js";
import { useToast } from "../../components/Toasts.js";
import { api, asReport, failureOf } from "../../lib/api.js";
import { backupStorageQuery, maintenanceWindowQuery } from "../../lib/queries.js";
import type { BackupStorage as Storage, MaintenanceWindow as Window, Report } from "../../lib/types.js";
import type { PaneProps } from "./General.js";

const emptyStorage = { endpoint: "", region: "", bucket: "", prefix: "", pathStyle: true, verifyTls: true, accessKeyId: "", secret: "" };

/**
 * The Backup storage card of Settings: the one S3-compatible bucket this
 * Host's database backups go to. The secret key is written, never read back:
 * an empty field keeps the saved one.
 */
export function BackupStorage({ id, size }: PaneProps) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const { data: saved, error: loadError } = useQuery(backupStorageQuery);
  const [draft, setDraft] = useState(emptyStorage);
  const [dirty, setDirty] = useState(false);
  const [working, setWorking] = useState<"save" | "test" | "remove" | null>(null);
  const [failure, setFailure] = useState<Report | null>(null);
  const [removing, setRemoving] = useState(false);

  useEffect(() => {
    if (saved && !dirty) setDraft({ endpoint: saved.endpoint, region: saved.region, bucket: saved.bucket, prefix: saved.prefix, pathStyle: saved.path_style, verifyTls: saved.verify_tls, accessKeyId: saved.access_key_id, secret: "" });
  }, [saved, dirty]);

  const change = (next: Partial<typeof draft>) => { setDraft({ ...draft, ...next }); setDirty(true); };
  const body = () => JSON.stringify({
    endpoint: draft.endpoint, region: draft.region, bucket: draft.bucket, prefix: draft.prefix,
    path_style: draft.pathStyle, verify_tls: draft.verifyTls, access_key_id: draft.accessKeyId,
    ...(draft.secret ? { secret_access_key: draft.secret } : {}),
  });
  const complete = Boolean(draft.region.trim() && draft.bucket.trim() && draft.accessKeyId.trim() && (draft.secret || saved?.has_secret));
  const disabled = !saved || working !== null;

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!complete || working) return;
    setWorking("save"); setFailure(null);
    try {
      const response = await api("/backups/storage", { method: "PUT", body: body() });
      if (!response.ok) throw await failureOf(response);
      queryClient.setQueryData(backupStorageQuery.queryKey, await response.json() as Storage);
      setDirty(false);
      void queryClient.invalidateQueries({ queryKey: ["databases"] });
    } catch (error) { setFailure(asReport(error)); }
    finally { setWorking(null); }
  }

  /** Tests what the form holds, saved or not: writes, reads and deletes a small object. */
  async function test() {
    if (!complete || working) return;
    setWorking("test"); setFailure(null);
    try {
      const response = await api("/backups/storage/test", { method: "POST", body: body() });
      if (!response.ok) throw await failureOf(response);
      notify("success", "The backup storage works", { error: "A test object was written, read and deleted.", caused_by: [] });
    } catch (error) { setFailure(asReport(error)); }
    finally { setWorking(null); }
  }

  async function remove() {
    setWorking("remove"); setFailure(null);
    try {
      const response = await api("/backups/storage", { method: "DELETE" });
      if (!response.ok) throw await failureOf(response);
      setDirty(false);
      await queryClient.invalidateQueries({ queryKey: backupStorageQuery.queryKey });
      void queryClient.invalidateQueries({ queryKey: ["databases"] });
    } catch (error) { setFailure(asReport(error)); }
    finally { setWorking(null); }
  }

  const message = dirty ? "Unsaved changes." : !saved ? "Loading..." : saved.configured ? "Saved." : "Not configured. Backups cannot run without it.";

  return (
    <GridPane id={id} size={size} title="Backup storage">
      <Form onSubmit={(event: FormEvent) => void save(event)}>
        <div className="form-body">
          <p className="muted t-label">Every backup of this Host goes to this S3-compatible bucket, over HTTPS. self-host does not encrypt backups, so use a private bucket.</p>
          {loadError ? <Failure failure={asReport(loadError)} /> : null}
          {failure ? <Failure failure={failure} /> : null}
          <FormField id="backup-storage-endpoint" label="Endpoint" className="mono" placeholder="https://s3.example.com" value={draft.endpoint} disabled={disabled}
            hint="An HTTPS address. Leave it empty for AWS S3." onChange={(event) => change({ endpoint: event.target.value })} />
          <FormField id="backup-storage-region" label="Region" className="mono" placeholder="us-east-1" value={draft.region} required disabled={disabled}
            onChange={(event) => change({ region: event.target.value })} />
          <FormField id="backup-storage-bucket" label="Bucket" className="mono" value={draft.bucket} required disabled={disabled}
            hint="The bucket must exist. self-host does not create it." onChange={(event) => change({ bucket: event.target.value })} />
          <FormField id="backup-storage-prefix" label="Prefix" className="mono" placeholder="backups/home" value={draft.prefix} disabled={disabled}
            hint="A folder inside the bucket. Use one per Host when Hosts share a bucket." onChange={(event) => change({ prefix: event.target.value })} />
          <div className="field">
            <div className="check">
              <Checkbox id="backup-storage-path-style" checked={draft.pathStyle} disabled={disabled} aria-describedby="backup-storage-path-style-help"
                onCheckedChange={(checked) => change({ pathStyle: checked === true })} />
              <Label htmlFor="backup-storage-path-style">Path-style addresses</Label>
            </div>
            <small className="field-hint" id="backup-storage-path-style-help">Most S3-compatible services need it. Turn it off for a service that puts the bucket in the hostname.</small>
          </div>
          <div className="field">
            <div className="check">
              <Checkbox id="backup-storage-verify-tls" checked={draft.verifyTls} disabled={disabled} aria-describedby="backup-storage-verify-tls-help"
                onCheckedChange={(checked) => change({ verifyTls: checked === true })} />
              <Label htmlFor="backup-storage-verify-tls">Verify the TLS certificate</Label>
            </div>
            <small className="field-hint" id="backup-storage-verify-tls-help">Turn it off only for a bucket on your own network with a self-signed certificate.</small>
          </div>
          <FormField id="backup-storage-access-key" label="Access key id" className="mono" autoComplete="off" value={draft.accessKeyId} required disabled={disabled}
            onChange={(event) => change({ accessKeyId: event.target.value })} />
          <FormField id="backup-storage-secret" label="Secret access key" type="password" autoComplete="new-password" value={draft.secret} disabled={disabled}
            placeholder={saved?.has_secret ? "Saved. Type a new one to replace it" : ""}
            hint="Kept in the Host's private state. The console and the API never show it again." onChange={(event) => change({ secret: event.target.value })} />
        </div>
        <FormActions tone={failure ? "danger" : dirty ? "warning" : "neutral"} message={message}>
          {saved?.configured ? <Button size="sm" type="button" variant="ghost" className="btn-danger-ghost" disabled={disabled} onClick={() => setRemoving(true)}>Remove</Button> : null}
          {dirty ? <Button size="sm" type="button" disabled={disabled} onClick={() => { setDirty(false); setFailure(null); }}>Discard</Button> : null}
          <Button size="sm" type="button" disabled={disabled || !complete} onClick={() => void test()}>{working === "test" ? "Testing..." : "Test connection"}</Button>
          <Button size="sm" type="submit" variant="primary" disabled={disabled || !dirty || !complete}>{working === "save" ? "Saving..." : "Save storage"}</Button>
        </FormActions>
      </Form>
      <AlertDialog open={removing} onOpenChange={setRemoving}><AlertDialogContent>
        <AlertDialogHeader><AlertDialogTitle>Remove backup storage</AlertDialogTitle></AlertDialogHeader>
        <div className="dialog-body stack"><AlertDialogDescription>
          The Host forgets the bucket and its keys. The repositories stay in the bucket. No backup, restore or verification runs until you save a storage again.
        </AlertDialogDescription></div>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction className="btn-danger" onClick={() => { setRemoving(false); void remove(); }}>Remove storage</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent></AlertDialog>
    </GridPane>
  );
}

const startHours = Array.from({ length: 24 }, (_, hour) => hour);
const lengths = Array.from({ length: 24 }, (_, index) => index + 1);
const clock = (hour: number) => `${String(hour % 24).padStart(2, "0")}:00`;

/**
 * The Maintenance window card of Settings: the hours, in the Host's own
 * time, in which planned database work such as a scheduled backup starts.
 */
export function MaintenanceWindow({ id, size }: PaneProps) {
  const queryClient = useQueryClient();
  const { data: saved, error: loadError } = useQuery(maintenanceWindowQuery);
  const [draft, setDraft] = useState({ start: 2, hours: 4 });
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<Report | null>(null);

  useEffect(() => {
    if (saved && !dirty) setDraft({ start: saved.start_hour, hours: saved.hours });
  }, [saved, dirty]);

  async function save(event: FormEvent) {
    event.preventDefault();
    if (saving) return;
    setSaving(true); setFailure(null);
    try {
      const response = await api("/maintenance-window", { method: "PUT", body: JSON.stringify({ start_hour: draft.start, hours: draft.hours }) });
      if (!response.ok) throw await failureOf(response);
      queryClient.setQueryData(maintenanceWindowQuery.queryKey, await response.json() as Window);
      setDirty(false);
      void queryClient.invalidateQueries({ queryKey: ["databases"] });
    } catch (error) { setFailure(asReport(error)); }
    finally { setSaving(false); }
  }

  const message = dirty ? "Unsaved changes."
    : saved ? `Saved. Planned work starts between ${clock(saved.start_hour)} and ${clock(saved.start_hour + saved.hours)}, Host time (UTC${saved.utc_offset}).`
    : "Loading...";

  return (
    <GridPane id={id} size={size} title="Maintenance window">
      <Form onSubmit={(event: FormEvent) => void save(event)}>
        <div className="form-body">
          <p className="muted t-label">The Host starts planned database work, such as scheduled backups, in these hours. Times use the Host's clock, not this browser's.</p>
          {loadError ? <Failure failure={asReport(loadError)} /> : null}
          {failure ? <Alert variant="error"><AlertDescription>{failure.error}</AlertDescription></Alert> : null}
          <Select value={String(draft.start)} disabled={!saved || saving} onValueChange={(start) => { setDraft({ ...draft, start: Number(start) }); setDirty(true); }}>
            <FormField id="maintenance-window-start" label="Starts at"><SelectTrigger><SelectValue /></SelectTrigger></FormField>
            <SelectContent>{startHours.map((hour) => <SelectItem key={hour} value={String(hour)}>{clock(hour)}</SelectItem>)}</SelectContent>
          </Select>
          <Select value={String(draft.hours)} disabled={!saved || saving} onValueChange={(hours) => { setDraft({ ...draft, hours: Number(hours) }); setDirty(true); }}>
            <FormField id="maintenance-window-hours" label="Lasts" hint="Work that starts inside the window may end after it."><SelectTrigger><SelectValue /></SelectTrigger></FormField>
            <SelectContent>{lengths.map((hours) => <SelectItem key={hours} value={String(hours)}>{hours === 1 ? "1 hour" : `${hours} hours`}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        <FormActions tone={failure ? "danger" : dirty ? "warning" : "neutral"} message={message}>
          {dirty ? <Button size="sm" type="button" disabled={saving} onClick={() => { setDirty(false); setFailure(null); }}>Discard</Button> : null}
          <Button size="sm" type="submit" variant="primary" disabled={!saved || !dirty || saving}>{saving ? "Saving..." : "Save window"}</Button>
        </FormActions>
      </Form>
    </GridPane>
  );
}
