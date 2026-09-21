import { useEffect, useState, type FormEvent } from "react";
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
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@momoi-labs/kiso-react";

import { Changes, type Change } from "../../components/Changes.js";
import { Icon } from "../../components/Icon.js";
import { api, failureOf, getJson } from "../../lib/api.js";

/** What `GET /settings` answers. */
export type Settings = {
  auditEventsMaxAge: {
    effective: string;
    source: "command-line" | "operator" | "default";
    /** The Operator's setting, kept even while the daemon flag pins the value. */
    setting: string | null;
  };
};

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

/**
 * The General tab of Settings: one form, one section per group of settings,
 * the way an Application's configuration reads. One group so far, the audit
 * history. Precedence is PostgreSQL's: the daemon flag outranks what is saved
 * here, and what is saved here outranks the default.
 *
 * Nothing is written until the Operator has seen the change spelled out:
 * Save opens a confirmation that lists every setting about to move, from
 * what to what. Each setting has its own way back to the default.
 */
export function General() {
  const [settings, setSettings] = useState<Settings | null | undefined>();
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

  async function load() {
    const current = await getJson<Settings>("/settings");
    setSettings(current);
    setValue(current?.auditEventsMaxAge.setting ?? "");
  }

  useEffect(() => {
    void load();
  }, []);

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
      await load();
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
    <>
      <Form id="general-settings" className="settings-form" onSubmit={(event) => propose(value.trim() || null, event)}>
        <div className="form-body">
          <p className="t-caps">Audit history</p>

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

        <FormActions sticky tone={dirty && !pinned ? "warning" : "neutral"} message={message}>
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
    </>
  );
}
