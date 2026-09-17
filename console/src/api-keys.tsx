import { StrictMode, useCallback, useEffect, useState, type FormEvent } from "react";
import { createRoot } from "react-dom/client";
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertTitle,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  FormField,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Search,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Toasts,
} from "@momoi-labs/kiso-react";

import { Icon } from "./components/Icon.js";
import { Shell } from "./components/Shell.js";
import { api, getJson, requireKey } from "./lib/api.js";
import { useEnvironments } from "./lib/useEnvironments.js";
import { usePlatform } from "./lib/usePlatform.js";
import type { ApiKey } from "./lib/types.js";
import "./console.css";

function ApiKeys() {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [label, setLabel] = useState("");
  const [created, setCreated] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [query, setQuery] = useState("");

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
      setCreating(false);
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

  const visible = keys.filter((key) =>
    `${key.label} ${key.id}`.toLowerCase().includes(query.trim().toLowerCase()),
  );

  return (
    <section className="page">
      <PageHeader
        actions={
          <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
            <Icon name="plus" />
            New key
          </Button>
        }
      >
        <PageHeaderTitle>API keys</PageHeaderTitle>
        <PageHeaderDescription>Manage access for other operators.</PageHeaderDescription>
      </PageHeader>

      {created ? (
        <Alert variant="info">
          <Icon name="info" size="md" />
          <AlertContent>
            <AlertTitle>Copy this key now. It will not be shown again.</AlertTitle>
            <AlertDescription className="mono">{created}</AlertDescription>
          </AlertContent>
        </Alert>
      ) : null}

      <div className="list-filters">
        <Search
          aria-label="Search by label"
          placeholder="Search by label..."
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        {query ? (
          <Button size="sm" variant="ghost" onClick={() => setQuery("")}>
            Clear filters
          </Button>
        ) : null}
      </div>

      <div className="table-wrap">
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
              {visible.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={4} className="muted">
                    {keys.length === 0 ? "No API keys yet." : "No keys match your filters."}
                  </TableCell>
                </TableRow>
              ) : (
                visible.map((key) => (
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
        <p className="table-footer">
          <span>
            {visible.length} of {keys.length} {keys.length === 1 ? "key" : "keys"}
          </span>
        </p>
      </div>

      {/* Creating is one field, so it is a dialog rather than a card that sits
          above the list forever asking to be filled in. */}
      <Dialog
        open={creating}
        onOpenChange={(open) => {
          setCreating(open);
          if (!open) setError(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New API key</DialogTitle>
            <DialogDescription>
              The key is shown once, when it is created.
            </DialogDescription>
          </DialogHeader>
          <form id="new-key" onSubmit={create}>
            <DialogBody>
              <FormField
                label="Label"
                id="key-label"
                placeholder="Laptop"
                required
                autoFocus
                value={label}
                onChange={(event) => setLabel(event.target.value)}
              />
              {error ? (
                <Alert variant="error">
                  <Icon name="alert" size="md" />
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              ) : null}
            </DialogBody>
            <DialogFooter>
              <Button size="sm" type="button" onClick={() => setCreating(false)}>
                Cancel
              </Button>
              <Button size="sm" variant="primary" type="submit">
                Create key
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </section>
  );
}

function SettingsShell({ crumb, children }: { crumb: string; children: React.ReactNode }) {
  const { apps, dnsSuffix, healthy } = usePlatform();
  const { environments } = useEnvironments();
  return (
    <Shell
      crumb={crumb}
      dnsSuffix={dnsSuffix}
      apps={apps}
      environments={environments}
      virtualMachine={(machine) => ({ href: `/console/#environment-${machine.id}`, active: false })}
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
    <Toasts>
      <SettingsShell crumb="API keys">
        <ApiKeys />
      </SettingsShell>
    </Toasts>
  </StrictMode>,
);
