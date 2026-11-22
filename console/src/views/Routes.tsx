import { useState } from "react";
import {
  Button, ListPage, Search, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { RedirectDialog, type RedirectEditor } from "../components/RedirectDialog.js";
import { RouteDialog, type RouteEditor } from "../components/RouteDialog.js";
import { RouteRows, type RedirectEntry } from "../components/RouteTable.js";
import { useToast } from "../components/Toasts.js";
import { isPublished, kindLabel, redirectDraft, redirectFrom, redirectTo, routesOf } from "../lib/routes.js";
import type { App } from "../lib/types.js";

/** The HTTP proxy's whole table: every hostname and path, and the Application it reaches. */
export function Routes({ apps, ready, dnsSuffix, reload, onOpenApp }: {
  apps: App[];
  ready: boolean;
  dnsSuffix: string;
  reload: () => Promise<App[]>;
  onOpenApp: (id: string) => void;
}) {
  const notify = useToast();
  const [query, setQuery] = useState("");
  const [application, setApplication] = useState("all");
  const [kind, setKind] = useState("all");
  const [editor, setEditor] = useState<RouteEditor | null>(null);
  const [redirectEditor, setRedirectEditor] = useState<RedirectEditor | null>(null);
  const published = apps.filter(isPublished);
  const routes = published.flatMap(routesOf)
    .sort((a, b) => a.hostname.localeCompare(b.hostname) || a.path.localeCompare(b.path));
  const visible = routes.filter((route) =>
    `${route.hostname}${route.path}`.includes(query.trim().toLowerCase()) &&
    (application === "all" || route.app.id === application || (application === "external" && route.app.runtime?.kind === "external")) &&
    (kind === "all" || route.kind === kind));
  const redirects: RedirectEntry[] = published.flatMap((app) => (app.redirects ?? []).map((rule, index) => ({ app, rule, index })))
    .filter(({ app, rule }) =>
      `${redirectFrom(rule)} ${redirectTo(rule)}`.toLowerCase().includes(query.trim().toLowerCase()) &&
      (application === "all" || app.id === application) &&
      (kind === "all" || kind === "redirect"));
  const total = routes.length + published.reduce((count, app) => count + (app.redirects?.length ?? 0), 0);
  // A redirect the route dialog can say opens there; one written as a pattern opens in the pattern editor.
  const editRedirect = ({ app, rule, index }: RedirectEntry) =>
    redirectDraft(rule) ? setEditor({ app, redirect: index }) : setRedirectEditor({ app, index });
  const filtering = query.trim() !== "" || application !== "all" || kind !== "all";
  const clear = () => { setQuery(""); setApplication("all"); setKind("all"); };

  return (
    <>
      <ListPage title="Routes"
        action={{ label: "Add route", icon: <Icon name="plus" />, onClick: () => setEditor({ app: null }) }}
        state={!ready ? "loading" : total ? "ready" : "empty"}
        empty={{ title: "No routes yet", text: "Add a route to an Application with a Hostname, or to an External app on your LAN." }}
        count={`${visible.length + redirects.length} of ${total} routes`}
        filters={<div className="list-filters">
          <Search aria-label="Search routes" placeholder="Search routes..." value={query} onChange={(event) => setQuery(event.target.value)} />
          <Select value={application} onValueChange={setApplication}>
            <SelectTrigger aria-label="Filter by application"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All applications</SelectItem>
              {published.filter((app) => app.runtime?.kind !== "external").map((app) => <SelectItem key={app.id} value={app.id}>{app.name}</SelectItem>)}
              {published.some((app) => app.runtime?.kind === "external") ? <SelectItem value="external">External apps</SelectItem> : null}
            </SelectContent>
          </Select>
          <Select value={kind} onValueChange={setKind}>
            <SelectTrigger aria-label="Filter by type"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All types</SelectItem>
              {Object.entries(kindLabel).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}
              <SelectItem value="redirect">Redirect</SelectItem>
            </SelectContent>
          </Select>
          {filtering ? <Button size="sm" variant="ghost" onClick={clear}>Clear filters</Button> : null}
        </div>}>
        <RouteRows routes={visible} redirects={redirects} ready showApplication
          onEditRedirect={editRedirect}
          reload={reload}
          onEdit={(route) => setEditor({ app: route.app, route })}
          onOpenApp={onOpenApp} />
      </ListPage>
      {!ready || total ? <details className="disclosure">
        <summary>How the proxy picks a route</summary>
        <ol>
          <li>It matches the hostname, in any letter case.</li>
          <li>Among that hostname's routes, the longest matching path wins. <code>/app</code> takes <code>/app/assets</code> but not <code>/apple</code>.</li>
          <li>Each Hostname and alias has a <code>/</code> route to its Application unless a path route says otherwise.</li>
          <li>A redirect answers before any of this, on the hostname and path it names.</li>
          <li>A hostname with no route answers 404. <code>console.{dnsSuffix}</code> always belongs to the Platform.</li>
        </ol>
      </details> : null}
      <RouteDialog editor={editor} apps={apps} dnsSuffix={dnsSuffix} onClose={() => setEditor(null)}
        onPattern={(app) => { setEditor(null); setRedirectEditor({ app }); }}
        onSaved={(message) => {
          setEditor(null);
          void reload().then(() => notify("success", message));
        }} />
      <RedirectDialog editor={redirectEditor} onClose={() => setRedirectEditor(null)}
        onSaved={(message) => {
          setRedirectEditor(null);
          void reload().then(() => notify("success", message));
        }} />
    </>
  );
}
