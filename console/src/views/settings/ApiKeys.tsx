import { useEffect, useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Alert,
  AlertContent,
  AlertDescription,
  AlertTitle,
  Badge,
  Button,
  Checkbox,
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  EmptyState,
  EmptyStateDescription,
  EmptyStateTitle,
  FormField,
  Label,
  RadioGroup,
  RadioGroupItem,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Table, TableFrame,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../../components/Icon.js";
import type { PaneProps } from "./General.js";
import { api, failureOf, readJson, requireKey } from "../../lib/api.js";
import { formatAgo } from "../../lib/format.js";
import { apiKeysQuery } from "../../lib/queries.js";
import type { ApiKey, TailnetStatus } from "../../lib/types.js";
import { SettingCard } from "./SettingCard.js";

const noKeys: ApiKey[] = [];

const agentScopes = [
  { value: "read", label: "Read", description: "Required read access to Applications, databases, DNS, events and logs." },
  { value: "apps:write", label: "Configure Applications", description: "Deploy, update, start or stop Applications and change their routes and Application Variables." },
  { value: "dns:write", label: "Change DNS Records", description: "Add and edit DNS Records. Only a Person key can delete one." },
  { value: "databases:write", label: "Manage databases", description: "Create servers and connections, start backups." },
  { value: "vms:write", label: "Manage virtual machines", description: "Create, start and stop machines." },
];
const agentDefault = ["read", "apps:write", "dns:write", "databases:write"];
const expiries = [
  { value: "7", label: "7 days" },
  { value: "30", label: "30 days" },
  { value: "90", label: "90 days" },
  { value: "365", label: "365 days" },
];
const anyDevice = "any";

function day(iso: string) {
  return iso.slice(0, 10);
}

/** The API keys card of Settings. It is wide: a table needs the room. */
export function ApiKeys({ id }: PaneProps) {
  const { data: keys = noKeys, isSuccess, refetch: load } = useQuery(apiKeysQuery);
  const empty = isSuccess && !keys.length;
  const { data: tailnet } = useQuery({
    queryKey: ["tailnet"],
    queryFn: ({ signal }) => readJson<TailnetStatus>("/tailnet", signal),
  });
  const [label, setLabel] = useState("");
  const [kind, setKind] = useState<"agent" | "human">("agent");
  const [scopes, setScopes] = useState<string[]>(agentDefault);
  const [expiry, setExpiry] = useState("30");
  const [device, setDevice] = useState(anyDevice);
  const [created, setCreated] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  useEffect(() => {
    requireKey();
  }, []);

  async function create(event: FormEvent) {
    event.preventDefault();
    setError(null);
    try {
      const res = await api("/api-keys", {
        method: "POST",
        body: JSON.stringify(
          kind === "agent"
            ? {
                label: label.trim(),
                kind,
                scopes,
                expires_in_days: Number(expiry),
                node_id: device === anyDevice ? null : device,
              }
            : { label: label.trim(), kind, expires_in_days: expiry === "never" ? "never" : Number(expiry) },
        ),
      });
      if (!res.ok) throw new Error(res.status === 422 || res.status === 409 ? (await failureOf(res)).error : "Could not create the key. Try again.");
      const data = (await res.json()) as { key: string };
      setLabel("");
      setKind("agent");
      setScopes(agentDefault);
      setExpiry("30");
      setDevice(anyDevice);
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

  function chooseKind(next: string) {
    setKind(next as "agent" | "human");
    if (next === "agent" && expiry === "never") setExpiry("30");
  }

  function toggleScope(scope: string, on: boolean) {
    setScopes((current) => (on ? [...current, scope] : current.filter((value) => value !== scope)));
  }

  const nodeName = (id?: string) => tailnet?.nodes.find((node) => node.id === id)?.name ?? id;
  const scopesOf = (key: ApiKey) => (
    <>
      {key.scopes.length >= 9 ? "All" : key.scopes.join(", ")}
      {key.node_id ? <span className="muted"> on {nodeName(key.node_id)}</span> : null}
    </>
  );
  const kindOf = (key: ApiKey) => <Badge variant="neutral">{key.kind === "agent" ? "Agent" : "Person"}</Badge>;
  const lastUsed = (key: ApiKey) => (key.last_used_at ? formatAgo(key.last_used_at) : <span className="muted">Never</span>);
  const createdOn = (key: ApiKey) => (key.created_at ? day(key.created_at) : <span className="muted">At init</span>);

  return (
    <SettingCard id={id} title="API keys" actions={
      <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
        <Icon name="plus" />
        New key
      </Button>
    }>
      {empty ? null : <p className="muted t-label">Agent keys work only on this Host's tailnet name.</p>}
      {created ? (
        <Alert variant="info">
          <Icon name="info" size="md" />
          <AlertContent>
            <AlertTitle>Copy this key now. It will not be shown again.</AlertTitle>
            <AlertDescription className="mono">{created}</AlertDescription>
          </AlertContent>
        </Alert>
      ) : null}

      {empty ? (
        <div className="empty-frame"><EmptyState variant="first-run">
          <EmptyStateTitle>No API keys yet</EmptyStateTitle>
          <EmptyStateDescription>A key lets a person, the CLI or an AI agent reach the API.</EmptyStateDescription>
        </EmptyState></div>
      ) : <TableFrame frame="none">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Key</TableHead>
                <TableHead scope="col">Kind</TableHead>
                <TableHead scope="col">Expires</TableHead>
                <TableHead scope="col">Scopes</TableHead>
                <TableHead scope="col">Last used</TableHead>
                <TableHead scope="col">Created</TableHead>
                <TableHead scope="col" className="col-tight">
                  Action
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {keys.map((key) => (
                  <TableRow key={key.id}>
                    <TableCell>
                      <div>{key.primary ? <>{key.label} <span className="muted t-label">Platform key, made by init</span></> : key.label}</div>
                      <div className="mono muted t-label">{key.id}</div>
                    </TableCell>
                    <TableCell>{kindOf(key)}</TableCell>
                    <TableCell className="key-date">{key.expires_at ? day(key.expires_at) : <span className="muted">Never</span>}</TableCell>
                    <TableCell className="mono t-label">{scopesOf(key)}</TableCell>
                    <TableCell>{lastUsed(key)}</TableCell>
                    <TableCell className="key-date">{createdOn(key)}</TableCell>
                    <TableCell className="col-tight">
                      {/* The Platform's own key cannot be revoked; it is what the console and CLI use. */}
                      {key.primary ? null : (
                        <Button
                          size="sm"
                          variant="ghost"
                          className="btn-danger-ghost"
                          onClick={() => void revoke(key.id)}
                        >
                          Revoke
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
            </TableBody>
          </Table>
      </TableFrame>}

      <Dialog
        open={creating}
        onOpenChange={(open) => {
          setCreating(open);
          if (!open) setError(null);
        }}
      >
        <DialogContent className="key-dialog">
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
                placeholder={kind === "agent" ? "Claude on laptop" : "Laptop"}
                required
                autoFocus
                value={label}
                onChange={(event) => setLabel(event.target.value)}
              />
              <RadioGroup label="Kind" variant="tiles" value={kind} onValueChange={chooseKind}>
                <RadioGroupItem value="agent" label="Agent" description="For AI agents on the tailnet name. They cannot open the console or delete Applications, DNS Records, databases, VMs or custom images." />
                <RadioGroupItem value="human" label="Person" description="Full access, for a person or the CLI." />
              </RadioGroup>
              {kind === "agent" ? (
                <fieldset className="field">
                  <legend className="t-label">Scopes</legend>
                  <div className="stack-xs">
                    {agentScopes.map((scope) => (
                      <div className="check" key={scope.value}>
                        <Checkbox
                          id={`key-scope-${scope.value}`}
                          checked={scopes.includes(scope.value)}
                          disabled={scope.value === "read"}
                          aria-describedby={`key-scope-${scope.value}-help`}
                          onCheckedChange={(checked) => toggleScope(scope.value, checked === true)}
                        />
                        <div className="stack-xs">
                          <Label htmlFor={`key-scope-${scope.value}`}>{scope.label}</Label>
                          <small className="field-hint" id={`key-scope-${scope.value}-help`}>{scope.description}</small>
                        </div>
                      </div>
                    ))}
                  </div>
                </fieldset>
              ) : null}
              <Select value={expiry} onValueChange={setExpiry}>
                <FormField id="key-expiry" label="Expires after" hint={kind === "agent" ? "Agent keys expire in at most 365 days." : undefined}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                </FormField>
                <SelectContent>
                  {expiries.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
                  {kind === "human" ? <SelectItem value="never">Never</SelectItem> : null}
                </SelectContent>
              </Select>
              {kind === "agent" ? (
                tailnet?.name ? (
                  <Select value={device} onValueChange={setDevice}>
                    <FormField id="key-device" label="Allowed tailnet device" hint="Pick a device to limit the key to it.">
                      <SelectTrigger><SelectValue /></SelectTrigger>
                    </FormField>
                    <SelectContent>
                      <SelectItem value={anyDevice}>Any device on the tailnet</SelectItem>
                      {tailnet.nodes.map((node) => <SelectItem key={node.id} value={node.id}>{node.name}</SelectItem>)}
                    </SelectContent>
                  </Select>
                ) : (
                  <Alert variant="warning">
                    <Icon name="alert" size="md" />
                    <AlertDescription>
                      This Host has no tailnet name with HTTPS. An agent key works only there, so it cannot be used until Tailscale is set up.
                    </AlertDescription>
                  </Alert>
                )
              ) : null}
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
    </SettingCard>
  );
}
