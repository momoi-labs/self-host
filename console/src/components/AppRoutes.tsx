import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogTitle, Badge, Button, Card, CardContent, CardHeader, CardTitle,
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
  EmptyState, EmptyStateTitle, KV, KVKey, KVValue, Label, Search, Switch,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { settingsQuery } from "../lib/queries.js";
import { routesOf, saveRedirects, saveRewriteHost, type RouteKind } from "../lib/routes.js";
import type { App } from "../lib/types.js";
import { Icon } from "./Icon.js";
import { RedirectDialog, type RedirectEditor } from "./RedirectDialog.js";
import { RouteDialog, type RouteEditor } from "./RouteDialog.js";
import { RouteTable } from "./RouteTable.js";
import { useToast } from "./Toasts.js";

const order: Record<RouteKind, number> = { hostname: 0, alias: 1, path: 2 };

/** An Application's Routes tab: its Hostname first, then its aliases and paths, each changed without a redeploy. */
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
  const [removingRedirect, setRemovingRedirect] = useState<number | null>(null);
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

  const visible = routes.filter((route) => `${route.hostname}${route.path}`.includes(query.trim().toLowerCase()));
  const redirects = app.redirects ?? [];

  async function removeRedirect(index: number) {
    try {
      const refused = await saveRedirects(app, redirects.filter((_, at) => at !== index));
      if (refused) { notify("danger", "Could not remove the redirect", refused); return; }
      await reload();
      notify("success", "Redirect removed");
    } catch (cause) {
      notify("danger", "Could not remove the redirect", asReport(cause));
    }
  }

  return (
    <div className="stack">
      <div className="between">
        <p className="muted">The hostnames and paths that reach {app.name}. A change applies at once, without a redeploy.</p>
        <Button size="sm" variant="primary" disabled={busy} onClick={() => setEditor({ app })}><Icon name="plus" />Add route</Button>
      </div>
      <div className="list-filters">
        <Search aria-label="Search routes" placeholder="Search routes..." value={query} onChange={(event) => setQuery(event.target.value)} />
        {query.trim() ? <Button size="sm" variant="ghost" onClick={() => setQuery("")}>Clear filters</Button> : null}
      </div>
      <RouteTable routes={visible} total={routes.length} reload={reload}
        onEdit={(route) => setEditor({ app, route })}
        onRename={onRename}
        empty={<EmptyState variant="first-run"><EmptyStateTitle>No routes</EmptyStateTitle></EmptyState>} />
      <Card>
        <CardHeader className="between">
          <CardTitle>Redirects</CardTitle>
          <Button size="sm" disabled={busy} onClick={() => setRedirectEditor({ app })}><Icon name="plus" />Add redirect</Button>
        </CardHeader>
        <CardContent>
          <div className="table-wrap">
            <div className="table-scroll">
              <Table aria-label="Redirects">
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Pattern</TableHead>
                    <TableHead scope="col">Send to</TableHead>
                    <TableHead scope="col">Status</TableHead>
                    <TableHead scope="col" className="col-tight">Actions</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {redirects.length === 0 ? (
                    <TableRow><TableCell colSpan={4} className="muted">No redirects. One answers on every hostname of {app.name} before a request reaches it; the first match wins.</TableCell></TableRow>
                  ) : redirects.map((redirect, index) => (
                    <TableRow key={`${index}:${redirect.pattern}`}>
                      <TableCell className="mono">{redirect.pattern}</TableCell>
                      <TableCell className="mono">{redirect.replacement}</TableCell>
                      <TableCell><Badge variant="neutral">{redirect.permanent ? "301" : "302"}</Badge></TableCell>
                      <TableCell className="col-tight">
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button size="sm" variant="ghost" className="btn-icon" aria-label={`Actions for redirect ${redirect.pattern}`}><Icon name="more" /></Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end">
                            <DropdownMenuItem onSelect={() => setRedirectEditor({ app, index })}>Edit redirect</DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem className="is-danger" onSelect={() => setRemovingRedirect(index)}>Remove redirect</DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
            <p className="table-footer"><span>{redirects.length} {redirects.length === 1 ? "redirect" : "redirects"}</span></p>
          </div>
        </CardContent>
      </Card>
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
        onSaved={(message) => {
          setEditor(null);
          void reload().then(() => notify("success", message));
        }} />
      <RedirectDialog editor={redirectEditor} onClose={() => setRedirectEditor(null)}
        onSaved={(message) => {
          setRedirectEditor(null);
          void reload().then(() => notify("success", message));
        }} />
      <AlertDialog open={removingRedirect !== null} onOpenChange={(open) => { if (!open) setRemovingRedirect(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader><AlertDialogTitle>Remove redirect</AlertDialogTitle></AlertDialogHeader>
          <div className="dialog-body">
            <AlertDialogDescription>
              <code>{removingRedirect === null ? null : redirects[removingRedirect]?.pattern}</code> stops answering. Requests it matched reach {app.name} instead.
            </AlertDialogDescription>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="btn-danger" onClick={() => { if (removingRedirect !== null) void removeRedirect(removingRedirect); setRemovingRedirect(null); }}>Remove redirect</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
