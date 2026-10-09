import { useEffect, useRef, useState, type FormEvent } from "react";
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
  Form,
  FormActions,
  FormField,
  GridPane,
  Input,
  KV,
  KVKey,
  KVValue,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from "@momoi-labs/kiso-react";

import { Changes, type Change } from "../../components/Changes.js";
import { Icon } from "../../components/Icon.js";
import { api, failureOf } from "../../lib/api.js";
import { settingsQuery } from "../../lib/queries.js";
import type { Settings } from "../../lib/types.js";

const DEFAULT_RETENTION = "30d";

type Unit = "d" | "h" | "m";
const units: { value: Unit; label: string }[] = [
  { value: "d", label: "days" },
  { value: "h", label: "hours" },
  { value: "m", label: "minutes" },
];

/** `45d` as the two controls hold it. Anything else reads as empty. */
function split(text: string): { amount: string; unit: Unit } {
  const match = /^(\d+)([dhm])$/.exec(text.trim());
  return match ? { amount: match[1], unit: match[2] as Unit } : { amount: "", unit: "d" };
}

function join(amount: string, unit: Unit): string {
  return amount.trim() ? `${amount.trim()}${unit}` : "";
}

/** Where a settings pane sits on the grid: its id, and its width out of twelve. */
export type PaneProps = { id: string; size?: number };

/**
 * How long the Events page keeps what happened. Precedence is PostgreSQL's:
 * the daemon flag outranks what is saved here, and what is saved here
 * outranks the default.
 *
 * It is a form: Save opens a confirmation that lists every setting about to
 * move, from what to what, because a shorter retention deletes events.
 */
export function AuditHistory({ id, size }: PaneProps) {
  const { data: settings, refetch } = useQuery(settingsQuery);
  const [amount, setAmount] = useState("");
  const [unit, setUnit] = useState<Unit>("d");
  const value = join(amount, unit);
  const setValue = (text: string) => {
    const parts = split(text);
    setAmount(parts.amount);
    setUnit(parts.unit);
  };
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  /** The write waiting for confirmation, or null. */
  const [pending, setPending] = useState<{ retention: string | null; changes: Change[] } | null>(null);

  // The field starts from what is saved, once, so a later read never undoes
  // an edit in progress.
  const started = useRef(false);
  useEffect(() => {
    if (!settings || started.current) return;
    started.current = true;
    setValue(settings.auditEventsMaxAge.setting ?? "");
  }, [settings]);

  const retention = settings?.auditEventsMaxAge;
  const pinned = retention?.source === "command-line";
  const saved = retention?.setting ?? "";
  const dirty = value.trim() !== saved;

  /** A retention as the confirmation reads it. */
  const describe = (setting: string) => (setting ? setting : `${DEFAULT_RETENTION} (default)`);

  function propose(next: string | null, event?: FormEvent) {
    event?.preventDefault();
    setError(null);
    setPending({
      retention: next,
      changes: [{ setting: "audit_events_max_age", from: describe(saved), to: describe(next ?? "") }],
    });
  }

  async function confirm() {
    if (!pending) return;
    setSaving(true);
    try {
      const res = await api("/settings", {
        method: "PUT",
        body: JSON.stringify({ auditEventsMaxAge: pending.retention }),
      });
      if (!res.ok) throw new Error((await failureOf(res)).error);
      const next = await refetch();
      setValue(next.data?.auditEventsMaxAge.setting ?? "");
      setPending(null);
    } catch (cause) {
      setError((cause as Error).message);
      setPending(null);
    } finally {
      setSaving(false);
    }
  }

  const message = pinned ? (
    <>
      <strong>Set by the daemon flag.</strong> Remove <code>--audit-events-max-age</code> from the
      service to change it here.
      {retention?.setting ? ` The value saved here, ${retention.setting}, applies again once the flag is gone.` : ""}
    </>
  ) : dirty ? (
    <><strong>Unsaved changes.</strong> Save shows what will change before anything is written.</>
  ) : retention?.source === "operator" ? (
    `Saved. Events are kept for ${retention.effective}.`
  ) : (
    `Using the default. Events are kept for ${DEFAULT_RETENTION}.`
  );

  return (
    <GridPane id={id} size={size} title="Audit history">
      <Form id="general-settings" onSubmit={(event) => propose(value.trim() || null, event)}>
        <div className="form-body">
          <p className="muted t-label">How long the Events page keeps what happened.</p>
          <FormField
            id="audit-events-max-age"
            label="Keep audit events for"
            hint="At least one day. Events older than this are deleted on the next change to the history, so a shorter value deletes them as soon as it is saved."
          >
            <div className="settings-duration">
              <Input
                id="audit-events-max-age"
                type="number"
                min={1}
                inputMode="numeric"
                placeholder={pinned ? split(retention?.effective ?? "").amount : "30"}
                value={pinned ? split(retention?.effective ?? "").amount : amount}
                disabled={pinned || settings == null}
                onChange={(event) => setAmount(event.target.value)}
              />
              <Select
                value={pinned ? split(retention?.effective ?? "").unit : unit}
                disabled={pinned || settings == null}
                onValueChange={(next) => setUnit(next as Unit)}
              >
                <SelectTrigger aria-label="Unit"><SelectValue /></SelectTrigger>
                <SelectContent>
                  {units.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
                </SelectContent>
              </Select>
              {/* Each setting finds its own way back; the footer only saves
                  the form as a whole. */}
              {saved && !pinned ? (
                <Button size="sm" variant="ghost" type="button" disabled={saving} onClick={() => propose(null)}>
                  Reset to default ({DEFAULT_RETENTION})
                </Button>
              ) : null}
            </div>
          </FormField>

          {error ? (
            <Alert variant="error">
              <Icon name="alert" size="md" />
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
        </div>

        <FormActions tone={dirty && !pinned ? "warning" : "neutral"} message={message}>
          {dirty && !pinned ? (
            <Button size="sm" type="button" onClick={() => setValue(saved)}>
              Discard
            </Button>
          ) : null}
          <Button size="sm" variant="primary" type="submit" disabled={pinned || saving || !dirty}>
            Save
          </Button>
        </FormActions>
      </Form>

      <AlertDialog open={pending !== null} onOpenChange={(open) => { if (!open) setPending(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Apply these settings?</AlertDialogTitle>
          </AlertDialogHeader>
          <div className="dialog-body">
            <AlertDialogDescription>
              The change applies as soon as it is confirmed.
            </AlertDialogDescription>
            {pending ? <Changes changes={pending.changes} /> : null}
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="primary" disabled={saving} onClick={() => void confirm()}>
              Apply
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </GridPane>
  );
}

/** Defaults every Application follows unless it, or the request, chooses otherwise. A switch applies as soon as it is flipped. */
export function ApplicationDefaults({ id, size }: PaneProps) {
  const queryClient = useQueryClient();
  const { data: settings } = useQuery(settingsQuery);
  const [toggling, setToggling] = useState(false);
  const [toggleError, setToggleError] = useState<string | null>(null);

  async function toggle(change: { pullNewerImages: boolean } | { rewriteHost: boolean }) {
    setToggling(true);
    setToggleError(null);
    try {
      const res = await api("/settings", {
        method: "PUT",
        body: JSON.stringify(change),
      });
      if (!res.ok) throw new Error((await failureOf(res)).error);
      queryClient.setQueryData(settingsQuery.queryKey, (await res.json()) as Settings);
    } catch (cause) {
      setToggleError((cause as Error).message);
    } finally {
      setToggling(false);
    }
  }

  return (
    <GridPane id={id} size={size} title="Applications">
      <KV>
        <KVKey className="kv-setting">
          <Label htmlFor="pull-newer-images">Pull newer images</Label>
          <p className="muted t-label">Restart and Save and redeploy pull each image from its registry first.</p>
        </KVKey>
        <KVValue className="kv-switch">
          <Switch
            id="pull-newer-images"
            checked={settings?.pullNewerImages.effective ?? false}
            disabled={settings == null || toggling}
            onCheckedChange={(checked) => void toggle({ pullNewerImages: checked })}
          />
        </KVValue>
        <KVKey className="kv-setting">
          <Label htmlFor="rewrite-host">Send the target address as Host</Label>
          <p className="muted t-label">For apps that refuse any Host but their loopback address.</p>
        </KVKey>
        <KVValue className="kv-switch">
          <Switch
            id="rewrite-host"
            checked={settings?.rewriteHost.effective ?? false}
            disabled={settings == null || toggling}
            onCheckedChange={(checked) => void toggle({ rewriteHost: checked })}
          />
        </KVValue>
      </KV>
      {toggleError ? (
        <Alert variant="error">
          <Icon name="alert" size="md" />
          <AlertDescription>{toggleError}</AlertDescription>
        </Alert>
      ) : null}
    </GridPane>
  );
}
