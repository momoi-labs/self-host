import { useState, type FormEvent } from "react";
import {
  Alert, AlertDescription, Button,
  Dialog, DialogBody, DialogContent,
  DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  EmptyState, EmptyStateDescription, EmptyStateTitle, FormField, GridPane,
  Search,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  StatusBadge, Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";
import {
  deleteGitConnection, saveGitConnection, useGitConnections,
  startGitAuthorization, useGitIntegrations,
  type GitConnection, type GitProvider,
} from "../lib/gitConnections.js";
import { useGitAuthorization } from "../lib/useGitAuthorization.js";
import { GitProviderSetup } from "./GitProviderSetup.js";

function providerName(provider: GitProvider) {
  return provider === "github" ? "GitHub" : "GitLab";
}

function connectionMethod(connection: GitConnection) {
  return connection.authentication === "github-app" ? "GitHub App" : connection.authentication === "oauth" ? `${providerName(connection.provider)} OAuth` : `${providerName(connection.provider)} token`;
}

export function GitConnectionDialog({ open, connection, onClose, onSaved }: {
  open: boolean;
  connection?: GitConnection;
  onClose: () => void;
  onSaved: (connection: GitConnection) => void;
}) {
  const [name, setName] = useState(connection?.name ?? "");
  const [provider, setProvider] = useState<GitProvider>(connection?.provider ?? "github");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [setup, setSetup] = useState(false);
  const [advanced, setAdvanced] = useState(Boolean(connection && (!connection.authentication || connection.authentication === "token")));
  const tokenOnly = Boolean(connection && (!connection.authentication || connection.authentication === "token"));
  const allowToken = !connection || tokenOnly;
  const { integrations, loading: integrationsLoading, error: integrationError, refresh: refreshIntegrations } = useGitIntegrations();
  const authorization = useGitAuthorization((result) => {
    if (!result.connection) { setError("Authorization did not create a connection. Try again."); return; }
    onSaved(result.connection);
    onClose();
  });
  const working = busy || authorization.busy;
  const cannotClose = busy || authorization.phase === "completing";
  const configured = integrations?.[provider].configured ?? false;
  const integrationConsole = integrations?.[provider].console_url;
  const sameConsole = !integrationConsole || integrationConsole === `${window.location.origin}/console/`;
  const connectionName = name.trim() || providerName(provider);

  function close() {
    if (cannotClose) return;
    void authorization.cancel();
    setToken("");
    onClose();
  }

  function connect(useExistingInstallation = Boolean(connection && provider === "github")) {
    if (working) return;
    setError(null);
    authorization.authorize(provider, () => startGitAuthorization(provider, connectionName, connection?.id, useExistingInstallation));
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    event.stopPropagation();
    if (working) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await saveGitConnection({ name: connectionName, provider, token }, connection?.id);
      setToken("");
      onSaved(saved);
      onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not save Git access. Try again.");
    } finally {
      setBusy(false);
    }
  }

  return <Dialog open={open} onOpenChange={(next) => { if (!next) close(); }}>
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{connection ? "Reconnect Git" : "Connect Git"}</DialogTitle>
        <DialogDescription>Authorize access in your provider, then reuse it when importing and updating applications.</DialogDescription>
      </DialogHeader>
        <DialogBody>
          <FormField id="git-connection-name" label="Connection name (optional)" autoFocus maxLength={120}
            placeholder={providerName(provider)} value={name} disabled={working} onChange={(event) => setName(event.target.value)} />
          <FormField id="git-connection-provider" label="Provider">
            <Select value={provider} disabled={working || Boolean(connection)} onValueChange={(value) => {
              if (value !== "github" && value !== "gitlab") return;
              setProvider(value);
              setToken("");
              setError(null);
            }}>
              <SelectTrigger id="git-connection-provider"><SelectValue /></SelectTrigger>
              <SelectContent><SelectItem value="github">GitHub</SelectItem><SelectItem value="gitlab">GitLab</SelectItem></SelectContent>
            </Select>
          </FormField>
          {tokenOnly ? <p className="muted">Update this connection's saved token. To use provider authorization, create a new connection.</p> : integrationsLoading ? <p className="muted" role="status">Loading provider setup...</p> : integrationError ? <Alert variant="error"><AlertDescription>{integrationError}</AlertDescription><Button type="button" size="sm" onClick={() => void refreshIntegrations()}>Retry</Button></Alert> : !configured ? <p className="muted">{providerName(provider)} needs one-time setup on this Host. Your application draft stays in place.</p> : !sameConsole ? <Alert variant="error"><AlertDescription>Open the console at {integrationConsole} to authorize this provider. Its callback must return to the same origin.</AlertDescription></Alert> : <p className="muted">Choose the account and repositories to allow in the provider's popup.</p>}
          {!tokenOnly && !connection && provider === "github" && configured && sameConsole && !integrationsLoading && !integrationError ? <Button size="sm" type="button" variant="ghost" disabled={working} onClick={() => connect(true)}>App already installed? Use existing access</Button> : null}
          {authorization.busy ? <p className="muted" role="status">{authorization.phase === "completing" ? "Confirming authorization... Keep the popup open." : `Finish authorization in the ${providerName(provider)} popup.`}</p> : null}
          {allowToken ? <details className="disclosure" open={advanced} onToggle={(event) => setAdvanced(event.currentTarget.open)}>
            <summary>Advanced</summary>
            <form id="git-token-form" onSubmit={save}>
              <FormField id="git-connection-token" label="Access token" type="password" autoComplete="new-password"
                required value={token} disabled={working} onChange={(event) => setToken(event.target.value)}
                hint={provider === "github"
                  ? "Use a personal access token with read access to the repositories and their contents."
                  : "Use a personal access token with read_user, read_api and read_repository access."} />
              <p className="muted">The Host stores the token privately.</p>
              {!tokenOnly ? <Button size="sm" type="submit" disabled={working || !token}>{busy ? "Checking token..." : "Save token"}</Button> : null}
            </form>
          </details> : null}
          {error || authorization.error ? <Alert variant="error"><AlertDescription>{error || authorization.error}</AlertDescription></Alert> : null}
        </DialogBody>
        <DialogFooter>
          <Button size="sm" type="button" disabled={cannotClose} onClick={close}>Cancel</Button>
          {tokenOnly ? <Button size="sm" type="submit" form="git-token-form" variant="primary" disabled={working || !token}>{busy ? "Checking token..." : "Reconnect with token"}</Button> : !configured && !integrationsLoading && !integrationError ? <Button size="sm" type="button" variant="primary" disabled={working} onClick={() => setSetup(true)}>Configure {providerName(provider)}</Button> : <Button size="sm" type="button" variant="primary" disabled={working || integrationsLoading || Boolean(integrationError) || !configured || !sameConsole} onClick={() => connect()}>{authorization.busy ? "Waiting for authorization..." : `Connect with ${providerName(provider)}`}</Button>}
        </DialogFooter>
      {setup ? <GitProviderSetup open initialProvider={provider} onClose={() => setSetup(false)} onConfigured={() => setSetup(false)} /> : null}
    </DialogContent>
  </Dialog>;
}

export function GitConnectionSelector({ value, onChange, disabled }: {
  value: string;
  onChange: (credentialId: string) => void;
  disabled?: boolean;
}) {
  const { connections, loading, error, refresh } = useGitConnections();
  const [dialog, setDialog] = useState<"new" | GitConnection | null>(null);
  const selected = connections.find((connection) => connection.credential_id === value);

  return <>
    <FormField id="git-access" label="Connection (optional)">
      <Select value={value || "none"} disabled={disabled || loading} onValueChange={(next) => {
        if (next === "connect") setDialog("new");
        else if (next === "none") onChange("");
        else if (next && (next === value || connections.some((connection) => connection.credential_id === next))) onChange(next);
      }}>
        <SelectTrigger id="git-access"><SelectValue placeholder={loading ? "Loading connections..." : "No authentication"} /></SelectTrigger>
        <SelectContent>
          <SelectItem value="none">No authentication</SelectItem>
          {value && !selected ? <SelectItem value={value}>Saved Git credential</SelectItem> : null}
          {connections.map((connection) => <SelectItem key={connection.id} value={connection.credential_id}>
            {connection.name} ({connectionMethod(connection)}){connection.status === "expired" ? " (reconnect required)" : ""}
          </SelectItem>)}
          <SelectItem value="connect">Connect another Git account</SelectItem>
        </SelectContent>
      </Select>
    </FormField>
    {selected?.status === "expired" ? <Button size="sm" type="button" disabled={disabled} onClick={() => setDialog(selected)}>Reconnect {selected.name}</Button> : null}
    {error ? <Alert variant="error"><AlertDescription>{error}</AlertDescription><Button size="sm" type="button" onClick={() => void refresh()}>Retry</Button></Alert> : null}
    {dialog ? <GitConnectionDialog open connection={dialog === "new" ? undefined : dialog}
      onClose={() => setDialog(null)} onSaved={(connection) => onChange(connection.credential_id)} /> : null}
  </>;
}

export function GitConnectionsSettings({ id, size }: { id: string; size?: number }) {
  const { connections, loading, error, refresh } = useGitConnections();
  const [query, setQuery] = useState("");
  const [dialog, setDialog] = useState<"new" | GitConnection | null>(null);
  const [disconnecting, setDisconnecting] = useState<GitConnection | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [setup, setSetup] = useState(false);
  const visible = connections.filter((connection) => `${connection.name} ${providerName(connection.provider)}`.toLowerCase().includes(query.trim().toLowerCase()));

  async function disconnect() {
    if (!disconnecting || busy) return;
    setBusy(true);
    setActionError(null);
    try {
      await deleteGitConnection(disconnecting.id);
      setDisconnecting(null);
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : "Could not disconnect Git access. Try again.");
    } finally {
      setBusy(false);
    }
  }

  return <GridPane id={id} size={size} title="Git connections" actions={<>
    <Button size="sm" onClick={() => setSetup(true)}>Provider setup</Button>
    <Button size="sm" variant="primary" onClick={() => setDialog("new")}>Connect Git</Button>
  </>}>
    <p className="muted t-label">Saved access for importing and updating applications.</p>
    {error ? <Alert variant="error"><AlertDescription>{error}</AlertDescription><Button size="sm" onClick={() => void refresh()}>Retry</Button></Alert> : null}
    <div className="list-filters">
      <Search aria-label="Search Git connections" placeholder="Search Git connections..." value={query} onChange={(event) => setQuery(event.target.value)} />
      {query ? <Button size="sm" variant="ghost" onClick={() => setQuery("")}>Clear filters</Button> : null}
    </div>
    <div className="table-wrap">
      <div className="table-scroll"><Table>
        <TableHeader><TableRow><TableHead scope="col">Name</TableHead><TableHead scope="col">Provider</TableHead><TableHead scope="col">Status</TableHead><TableHead scope="col" className="col-tight">Access</TableHead></TableRow></TableHeader>
        <TableBody>
          {visible.map((connection) => <TableRow key={connection.id}>
            <TableCell>{connection.name}</TableCell><TableCell>{connectionMethod(connection)}</TableCell>
            <TableCell><StatusBadge tone={connection.status === "expired" ? "danger" : "success"}>{connection.status === "expired" ? "Reconnect required" : "Connected"}</StatusBadge></TableCell>
            <TableCell className="col-tight"><Button size="sm" variant="ghost" onClick={() => setDialog(connection)}>Reconnect</Button><Button size="sm" variant="ghost" className="btn-danger-ghost" onClick={() => { setActionError(null); setDisconnecting(connection); }}>Disconnect</Button></TableCell>
          </TableRow>)}
          {!visible.length ? <TableRow><TableCell colSpan={4}>
            {loading ? "Loading connections..." : error ? "Could not load connections." : connections.length ? "No connections match your filters." : <EmptyState variant="first-run"><EmptyStateTitle>No Git connections</EmptyStateTitle><EmptyStateDescription>Connect an account to choose its repositories. Public repositories can also use a URL.</EmptyStateDescription></EmptyState>}
          </TableCell></TableRow> : null}
        </TableBody>
      </Table></div>
      <p className="table-footer"><span>{visible.length} of {connections.length} connections</span></p>
    </div>
    {dialog ? <GitConnectionDialog open connection={dialog === "new" ? undefined : dialog} onClose={() => setDialog(null)} onSaved={() => {}} /> : null}
    {setup ? <GitProviderSetup open onClose={() => setSetup(false)} /> : null}
    <Dialog open={Boolean(disconnecting)} onOpenChange={(open) => { if (!open && !busy) setDisconnecting(null); }}>
      <DialogContent>
        <DialogHeader><DialogTitle>Disconnect {disconnecting?.name}?</DialogTitle><DialogDescription>Running applications stay in place. Their next private Git build needs another connection. Saved credentials will be removed from this Host.</DialogDescription></DialogHeader>
        {actionError ? <DialogBody><Alert variant="error"><AlertDescription>{actionError}</AlertDescription></Alert></DialogBody> : null}
        <DialogFooter><Button size="sm" disabled={busy} onClick={() => setDisconnecting(null)}>Cancel</Button><Button size="sm" variant="destructive" disabled={busy} onClick={() => void disconnect()}>{busy ? "Disconnecting..." : "Disconnect"}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </GridPane>;
}
