import { useCallback, useEffect, useState, type FormEvent } from "react";
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
  Search,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../../components/Icon.js";
import { api, getJson, requireKey } from "../../lib/api.js";
import type { ApiKey } from "../../lib/types.js";

/** The API keys tab of Settings. */
export function ApiKeys() {
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
    <div className="stack">
      <div className="between">
        <p className="muted t-label">Keys other operators and the CLI use to reach the API.</p>
        <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
          <Icon name="plus" />
          New key
        </Button>
      </div>

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
    </div>
  );
}
