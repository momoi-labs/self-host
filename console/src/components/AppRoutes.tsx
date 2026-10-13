import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Button, Card, CardContent, CardHeader, CardTitle,
  EmptyState, EmptyStateTitle, KV, KVKey, KVValue, Label, Search, Switch,
} from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { settingsQuery } from "../lib/queries.js";
import { redirectDraft, redirectFrom, redirectTo, routesOf, saveRewriteHost, type RouteKind } from "../lib/routes.js";
import type { App } from "../lib/types.js";
import { Icon } from "./Icon.js";
import { RedirectDialog, type RedirectEditor } from "./RedirectDialog.js";
import { RouteDialog, type RouteEditor } from "./RouteDialog.js";
import { RouteTable, type RedirectEntry } from "./RouteTable.js";
import { useToast } from "./Toasts.js";

const order: Record<RouteKind, number> = { hostname: 0, alias: 1, path: 2 };

/** An Application's Routes tab: its Hostname first, then its aliases, paths and redirects, each changed without a redeploy. */
export function AppRoutes({ app, apps, dnsSuffix, busy, reload, onRename }: {
  app: App;
  apps: App[];
  dnsSuffix: string;
  /** An operation is running, so nothing new starts from here. */
  busy: boolean;
  reload: () => Promise<App[]>;
  onRename: () => void;
}) {
  const notify = useToast();
  const [query, setQuery] = useState("");
  const [editor, setEditor] = useState<RouteEditor | null>(null);
  const [redirectEditor, setRedirectEditor] = useState<RedirectEditor | null>(null);
  const [toggling, setToggling] = useState(false);
  const settings = useQuery(settingsQuery).data;
  const rewriteHost = app.rewrite_host ?? settings?.rewriteHost.effective ?? false;
  const routes = routesOf(app)
    .sort((a, b) => order[a.kind] - order[b.kind] || a.hostname.localeCompare(b.hostname) || a.path.localeCompare(b.path));
  /** True or false sets this Application's choice; null follows the setting again. */
  async function chooseRewriteHost(next: boolean | null) {
    setToggling(true);
    try {
      const refused = await saveRewriteHost(app, next);
      if (refused) notify("danger", "Could not save the Host setting", refused);
      else await reload();
    } catch (cause) {
      notify("danger", "Could not save the Host setting", asReport(cause));
    } finally {
      setToggling(false);
    }
  }

  const needle = query.trim().toLowerCase();
  const visible = routes.filter((route) => `${route.hostname}${route.path}`.includes(needle));
  const redirects = app.redirects ?? [];
  const shownRedirects = redirects.map((rule, index) => ({ app, rule, index }))
    .filter(({ rule }) => `${redirectFrom(rule)} ${redirectTo(rule)}`.toLowerCase().includes(needle));
  // A redirect the route dialog can say opens there; one written as a pattern opens in the pattern editor.
  function editRedirect({ index }: RedirectEntry) {
    if (redirectDraft(redirects[index])) setEditor({ app, redirect: index });
    else setRedirectEditor({ app, index });
  }

  return (
    <div className="stack">
      <RouteTable routes={visible} redirects={shownRedirects} total={routes.length + redirects.length} reload={reload}
        onEditRedirect={editRedirect}
        toolbar={
          <div className="table-toolbar panel-toolbar">
            <h2 className="section-title">Routes</h2>
            <Search aria-label="Search routes" containerClassName="section-search" placeholder="Search routes…"
              value={query} onChange={(event) => setQuery(event.target.value)} />
            <Button size="sm" variant="ghost" className="panel-action" disabled={busy} onClick={() => setEditor({ app })}>
              <Icon name="plus" />
              Add route
            </Button>
          </div>
        }
        onEdit={(route) => setEditor({ app, route })}
        onRename={onRename}
        empty={<EmptyState variant="first-run"><EmptyStateTitle>No routes</EmptyStateTitle></EmptyState>} />
      <Card>
        <CardHeader><CardTitle>Proxy</CardTitle></CardHeader>
        <CardContent>
          <KV>
            <KVKey className="kv-setting">
              <Label htmlFor="route-rewrite-host">Send the target address as Host</Label>
              <p className="muted t-label">For an app that refuses any Host but its loopback address.</p>
            </KVKey>
            <KVValue className="kv-switch">
              <Switch id="route-rewrite-host" checked={rewriteHost} disabled={busy || toggling || !settings}
                onCheckedChange={(checked) => void chooseRewriteHost(checked === settings?.rewriteHost.effective ? null : checked)} />
            </KVValue>
          </KV>
        </CardContent>
      </Card>
      <RouteDialog editor={editor} apps={apps} dnsSuffix={dnsSuffix} onClose={() => setEditor(null)}
        onPattern={(one) => { setEditor(null); setRedirectEditor({ app: one }); }}
        onSaved={(message) => {
          setEditor(null);
          void reload().then(() => notify("success", message));
        }} />
      <RedirectDialog editor={redirectEditor} onClose={() => setRedirectEditor(null)}
        onSaved={(message) => {
          setRedirectEditor(null);
          void reload().then(() => notify("success", message));
        }} />
    </div>
  );
}
