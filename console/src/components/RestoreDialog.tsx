import { useEffect, useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  FormField, Input, RadioGroup, RadioGroupItem,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@momoi-labs/kiso-react";

import { api, asReport, failureOf } from "../lib/api.js";
import { databaseBackupsQuery } from "../lib/queries.js";
import type { Report } from "../lib/types.js";
import { Failure } from "./Failure.js";

const when = (iso: string) => new Date(iso).toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
/** A `datetime-local` value for now, in the browser's time zone. */
const localNow = () => { const now = new Date(); return new Date(now.getTime() - now.getTimezoneOffset() * 60_000).toISOString().slice(0, 16); };

/**
 * Restores a server's backups into a new server. With one source the dialog
 * is about that server; with more, the Operator picks which one first.
 */
export function RestoreDialog({ open, sources, onClose, onRestoring }: {
  open: boolean;
  /** The servers that have a backup to restore. */
  sources: { id: string; name: string }[];
  onClose: () => void;
  /** A restore was queued: the new server's id and name. */
  onRestoring: (id: string, name: string, source: string) => void;
}) {
  const [sourceId, setSourceId] = useState("");
  const [name, setName] = useState("");
  const [target, setTarget] = useState("latest");
  const [time, setTime] = useState("");
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<Report | null>(null);
  const pick = sources.length > 1;
  const source = sources.find((item) => item.id === sourceId) ?? (pick ? undefined : sources[0]);
  const backups = useQuery({ ...databaseBackupsQuery(source?.id ?? ""), enabled: open && !!source });
  const oldest = backups.data?.[backups.data.length - 1];

  useEffect(() => {
    if (!open) return;
    setSourceId(""); setName(""); setTarget("latest"); setTime(localNow()); setRefusal(null);
  }, [open]);

  async function restore(event: FormEvent) {
    event.preventDefault();
    const at = target === "time" ? new Date(time) : null;
    if (busy || !source || !name.trim() || (at && Number.isNaN(at.getTime()))) return;
    setBusy(true); setRefusal(null);
    try {
      const response = await api(`/databases/${source.id}/backups/restore`, { method: "POST", body: JSON.stringify({ name: name.trim(), ...(at ? { time: at.toISOString() } : {}) }) });
      if (!response.ok) { setRefusal(await failureOf(response)); return; }
      const accepted = await response.json() as { id: string };
      onClose();
      onRestoring(accepted.id, name.trim(), source.name);
    } catch (error) { setRefusal(asReport(error)); }
    finally { setBusy(false); }
  }

  return <Dialog open={open} onOpenChange={(next) => { if (!next) onClose(); }}>
    <DialogContent>
      <DialogHeader><DialogTitle>Restore to a new server</DialogTitle></DialogHeader>
      <form onSubmit={(event: FormEvent) => void restore(event)}>
        <DialogBody>
          <div className="stack">
            {pick ? <FormField id="restore-source" label="Restore from" hint="Servers that have a backup.">
              <Select value={sourceId} disabled={busy} onValueChange={setSourceId}>
                <SelectTrigger id="restore-source"><SelectValue placeholder="Choose a server" /></SelectTrigger>
                <SelectContent>{sources.map((item) => <SelectItem key={item.id} value={item.id}>{item.name}</SelectItem>)}</SelectContent>
              </Select>
            </FormField> : null}
            <FormField id="restore-name" label="New server name" value={name} required disabled={busy}
              hint={`self-host creates a new server with the databases, roles and passwords ${source ? source.name : "the source"} had, and opens it. ${source ? source.name : "The source"} stays unchanged and keeps running.`}
              onChange={(event) => setName(event.target.value)} />
            <RadioGroup label="Restore to" value={target} disabled={busy} onValueChange={setTarget}>
              <RadioGroupItem value="latest" label="The latest state" description="Everything the bucket has, up to the last minute." />
              <RadioGroupItem value="time" label="A moment in the past" description={oldest ? `Any time after ${when(oldest.finished_at)}.` : undefined} />
            </RadioGroup>
            {target === "time" ? <FormField id="restore-time" label="Date and time" hint="In this browser's time zone.">
              <Input id="restore-time" type="datetime-local" step={1} required value={time} max={localNow()} disabled={busy} onChange={(event) => setTime(event.target.value)} />
            </FormField> : null}
            {refusal ? <Failure failure={refusal} /> : null}
          </div>
        </DialogBody>
        <DialogFooter>
          <Button size="sm" type="button" onClick={onClose}>Cancel</Button>
          <Button size="sm" type="submit" variant="primary" disabled={busy || !source || !name.trim() || (target === "time" && !time)}>{busy ? "Queuing..." : "Create and restore"}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>;
}
