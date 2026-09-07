import { StrictMode, useCallback, useEffect, useState, type FormEvent } from "react";
import { createRoot } from "react-dom/client";
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertTitle,
  Button,
  Card,
  CardContent,
  CardHeader,
  FormField,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "./components/Icon.js";
import { Shell } from "./components/Shell.js";
import { api, getJson, requireKey } from "./lib/api.js";
import { usePlatform } from "./lib/usePlatform.js";
import type { ApiKey } from "./lib/types.js";
import "./console.css";

function ApiKeys() {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [label, setLabel] = useState("");
  const [created, setCreated] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setKeys((await getJson<ApiKey[]>("/api-keys")) ?? []);
  }, []);

  useEffect(() => {
    if (!requireKey()) return;
    void load();
  }, [load]);

  async function create(event: FormEvent) {
    event.preventDefault();
    setError(null);
    try {
      const res = await api("/api-keys", {
        method: "POST",
        body: JSON.stringify({ label: label.trim() }),
      });
      if (!res.ok) throw new Error("Could not create the key. Try again.");
      const data = (await res.json()) as { key: string };
      setLabel("");
      setCreated(data.key);
      await load();
    } catch (cause) {
      setError((cause as Error).message);
    }
  }

  async function revoke(id: string) {
    if (!confirm(`Revoke key ${id}?`)) return;
    await api(`/api-keys/${encodeURIComponent(id)}`, { method: "DELETE" });
    await load();
  }

  return (
    <section className="page">
      <PageHeader>
          <PageHeaderTitle>API keys</PageHeaderTitle>
          <PageHeaderDescription>Manage access for other operators.</PageHeaderDescription>
      </PageHeader>

      <Card aria-labelledby="create-heading">
        <CardHeader>
          <h2 className="t-h3" id="create-heading">
            Create a key
          </h2>
        </CardHeader>
        <CardContent>
          <form className="row" onSubmit={create}>
            <FormField
              label="Label"
              id="key-label"
              className="grow"
              placeholder="Laptop"
              required
              value={label}
              onChange={(event) => setLabel(event.target.value)}
            />
            <Button variant="primary" size="sm" type="submit">
              Create key
            </Button>
          </form>

          {error ? (
            <Alert variant="error">
              <Icon name="alert" size="md" />
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}

          {created ? (
            <Alert variant="info">
              <Icon name="info" size="md" />
              <AlertContent>
                <AlertTitle>Copy this key now. It will not be shown again.</AlertTitle>
                <AlertDescription className="mono">{created}</AlertDescription>
              </AlertContent>
            </Alert>
          ) : null}
        </CardContent>
      </Card>

      <section className="table-wrap" aria-labelledby="keys-heading">
        <h2 id="keys-heading" className="hidden">
          Existing API keys
        </h2>
        <div className="table-scroll">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">ID</TableHead>
                <TableHead scope="col">Label</TableHead>
                <TableHead scope="col">Created</TableHead>
                <TableHead scope="col" className="col-tight">
                  Action
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {keys.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={4} className="muted">
                    No API keys yet.
                  </TableCell>
                </TableRow>
              ) : (
                keys.map((key) => (
                  <TableRow key={key.id}>
                    <TableCell className="mono">{key.id}</TableCell>
                    <TableCell>{key.label}</TableCell>
                    <TableCell>{key.created_at}</TableCell>
                    <TableCell className="col-tight">
                      <Button
                        size="sm"
                        variant="ghost"
                        className="btn-danger-ghost"
                        onClick={() => void revoke(key.id)}
                      >
                        Revoke
                      </Button>
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>
      </section>
    </section>
  );
}

function SettingsShell({ crumb, children }: { crumb: string; children: React.ReactNode }) {
  const { apps, dnsSuffix, healthy } = usePlatform();
  return (
    <Shell
      crumb={crumb}
      dnsSuffix={dnsSuffix}
      apps={apps}
      healthy={healthy}
      overview={{ href: "/console/", active: false }}
      deploy={{ href: "/console/#new", active: false }}
      application={(app) => ({ href: `/console/#app-${app.id}`, active: false })}
    >
      {children}
    </Shell>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <SettingsShell crumb="API keys">
      <ApiKeys />
    </SettingsShell>
  </StrictMode>,
);
