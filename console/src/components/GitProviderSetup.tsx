import { useState, type FormEvent } from "react";
import {
  Alert, AlertDescription, Button, Dialog, DialogBody, DialogContent,
  DialogDescription, DialogFooter, DialogHeader, DialogTitle, FormField,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@momoi-labs/kiso-react";
import {
  registerGithubIntegration, saveGitlabIntegration, useGitIntegrations,
  type GitProvider,
} from "../lib/gitConnections.js";
import { validateConsoleUrl } from "../lib/gitAuthorization.js";
import { useGitAuthorization } from "../lib/useGitAuthorization.js";

export function GitProviderSetup({ open, initialProvider = "github", onClose, onConfigured }: {
  open: boolean;
  initialProvider?: GitProvider;
  onClose: () => void;
  onConfigured?: () => void;
}) {
  const { integrations, loading, error: loadError, refresh } = useGitIntegrations();
  const [provider, setProvider] = useState<GitProvider>(initialProvider);
  const [consoleUrl, setConsoleUrl] = useState(`${window.location.origin}/console/`);
  const [organization, setOrganization] = useState("");
  const [clientId, setClientId] = useState(integrations?.gitlab.client_id ?? "");
  const [clientSecret, setClientSecret] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const authorization = useGitAuthorization((result) => {
    if (!result.integrations?.github.configured) {
      setError("GitHub setup did not finish. Try creating the app again.");
      return;
    }
    onConfigured?.();
    onClose();
  });
  const busy = saving || authorization.busy;
  const cannotClose = saving || authorization.phase === "completing";
  const callbackUrl = `${window.location.origin}/source/authorization/callback`;

  function close() {
    if (cannotClose) return;
    void authorization.cancel();
    setClientSecret("");
    onClose();
  }
  function registerGithub() {
    setError(null);
    try {
      const validUrl = validateConsoleUrl(consoleUrl, window.location.origin);
      authorization.authorize("github", () => registerGithubIntegration(validUrl, organization.trim() || undefined), true);
    } catch (cause) { setError(cause instanceof Error ? cause.message : "Check the console URL."); }
  }
  async function saveGitlab(event: FormEvent) {
    event.preventDefault();
    event.stopPropagation();
    if (busy) return;
    setError(null);
    try {
      const validUrl = validateConsoleUrl(consoleUrl, window.location.origin);
      setSaving(true);
      await saveGitlabIntegration({ console_url: validUrl, client_id: clientId.trim(), client_secret: clientSecret });
      setClientSecret("");
      onConfigured?.();
      onClose();
    } catch (cause) { setError(cause instanceof Error ? cause.message : "Could not save GitLab setup. Try again."); }
    finally { setSaving(false); }
  }

  return <Dialog open={open} onOpenChange={(next) => { if (!next) close(); }}>
    <DialogContent>
      <DialogHeader><DialogTitle>Configure Git providers</DialogTitle><DialogDescription>One-time setup for this Host. Applications and their draft settings stay in place.</DialogDescription></DialogHeader>
      <form onSubmit={(event) => {
        if (provider === "gitlab") void saveGitlab(event);
        else { event.preventDefault(); event.stopPropagation(); registerGithub(); }
      }}>
        <DialogBody>
          <FormField id="git-setup-provider" label="Provider">
            <Select value={provider} disabled={busy} onValueChange={(value) => {
              if (value !== "github" && value !== "gitlab") return;
              setProvider(value); setClientSecret(""); setError(null);
            }}><SelectTrigger id="git-setup-provider"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="github">GitHub</SelectItem><SelectItem value="gitlab">GitLab</SelectItem></SelectContent></Select>
          </FormField>
          {loadError ? <Alert variant="error"><AlertDescription>{loadError}</AlertDescription><Button size="sm" type="button" onClick={() => void refresh()}>Retry</Button></Alert> : null}
          <FormField id="git-setup-console" label="Console URL" type="url" required value={consoleUrl} disabled={busy} onChange={(event) => setConsoleUrl(event.target.value)} hint="Use this console's HTTPS address. HTTP is supported on localhost only." />
          {provider === "github" ? <>
            {integrations?.github.configured ? <p className="muted">GitHub App <strong>{integrations.github.app_slug}</strong> is configured. Replacing it requires removing its saved connections first.</p> : <p className="muted">Create a GitHub App, then choose which repositories it can read when connecting an account.</p>}
            <FormField id="git-setup-organization" label="GitHub organization (optional)" value={organization} disabled={busy} onChange={(event) => setOrganization(event.target.value)} placeholder="Leave empty for your personal account" />
            <p className="muted">GitHub will confirm the app registration in a popup. <a href="https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest" target="_blank" rel="noreferrer">Registration guide</a></p>
          </> : <>
            <p className="muted"><a href="https://gitlab.com/-/user_settings/applications" target="_blank" rel="noreferrer">Register an application in GitLab</a> with the callback below and scopes <code>read_user read_api read_repository</code>.</p>
            <FormField id="git-setup-callback" label="Callback URL" value={callbackUrl} readOnly className="mono" hint="Copy this exact URL into the application's Redirect URI." />
            <FormField id="git-setup-client-id" label="Application ID" required={provider === "gitlab"} value={clientId} disabled={busy} onChange={(event) => setClientId(event.target.value)} />
            <FormField id="git-setup-secret" label="Application secret" type="password" autoComplete="new-password" required={provider === "gitlab"} value={clientSecret} disabled={busy} onChange={(event) => setClientSecret(event.target.value)} hint="Saved privately on the Host. It is never shown again." />
            {integrations?.gitlab.configured ? <p className="muted">GitLab is configured. Replacing it requires removing its saved OAuth connections first.</p> : null}
          </>}
          {authorization.busy ? <p className="muted" role="status">{authorization.phase === "completing" ? "Saving GitHub setup... Keep the popup open." : "Finish registration in the GitHub popup. Keep it open until it closes automatically."}</p> : null}
          {error || authorization.error ? <Alert variant="error"><AlertDescription>{error || authorization.error}</AlertDescription></Alert> : null}
        </DialogBody>
        <DialogFooter>
          <Button type="button" size="sm" disabled={cannotClose} onClick={close}>Cancel</Button>
          {provider === "github" ? <Button type="button" size="sm" variant="primary" disabled={busy || loading || Boolean(loadError)} onClick={registerGithub}>{authorization.busy ? "Waiting for GitHub..." : integrations?.github.configured ? "Replace GitHub App" : "Create GitHub App"}</Button> : <Button type="submit" size="sm" variant="primary" disabled={busy || loading || Boolean(loadError) || !clientId.trim() || !clientSecret}>{saving ? "Saving..." : "Save GitLab setup"}</Button>}
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>;
}
