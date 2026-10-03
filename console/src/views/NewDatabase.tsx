import { useState, type FormEvent } from "react";
import { Button, Card, Form, FormActions, FormField, PageHeader, PageHeaderTitle, Spinner } from "@momoi-labs/kiso-react";

import { Failure } from "../components/Failure.js";
import { PostgresFields } from "../components/PostgresFields.js";
import { useToast } from "../components/Toasts.js";
import { api, asReport, failureOf } from "../lib/api.js";
import type { App, Report } from "../lib/types.js";

export function NewDatabase({ reload, onCancel, onCreated }: {
  reload: () => Promise<App[]>;
  onCancel: () => void;
  onCreated: (id: string) => void;
}) {
  const [name, setName] = useState("");
  const [major, setMajor] = useState(18);
  const [failure, setFailure] = useState<Report | null>(null);
  const [saving, setSaving] = useState(false);
  const notify = useToast();

  async function create(event: FormEvent) {
    event.preventDefault();
    if (saving) return;
    setSaving(true);
    setFailure(null);
    try {
      const response = await api("/databases", { method: "POST", body: JSON.stringify({ name, major }) });
      if (!response.ok) throw await failureOf(response);
      const accepted = await response.json() as { id: string };
      await reload();
      onCreated(accepted.id);
      notify("success", `Creating ${name}`);
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setSaving(false);
    }
  }

  return <>
    <PageHeader><PageHeaderTitle>New database</PageHeaderTitle></PageHeader>
    <Card className="form-page">
      <Form onSubmit={create} aria-busy={saving}>
        <div className="form-body">
          <FormField id="database-name" label="Name" value={name} onChange={(event) => setName(event.target.value)} required autoFocus />
          <PostgresFields major={major} onChange={setMajor} />
          {failure ? <Failure failure={failure} /> : null}
        </div>
        <FormActions sticky tone={failure ? "danger" : "neutral"}>
          <Button size="sm" type="button" onClick={onCancel}>Cancel</Button>
          <Button size="sm" type="submit" variant="primary" disabled={saving || !name.trim()}>{saving ? <><Spinner size="sm" />Creating...</> : "Create database"}</Button>
        </FormActions>
      </Form>
    </Card>
  </>;
}
