import { useState } from "react";
import {
  Button, EmptyState, EmptyStateDescription, EmptyStateTitle, PageHeader, PageHeaderDescription, PageHeaderTitle,
  Search, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { RouteDialog, type RouteEditor } from "../components/RouteDialog.js";
import { RouteTable } from "../components/RouteTable.js";
import { useToast } from "../components/Toasts.js";
import { isPublished, kindLabel, routesOf } from "../lib/routes.js";
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
  const published = apps.filter(isPublished);
  const routes = published.flatMap(routesOf)
    .sort((a, b) => a.hostname.localeCompare(b.hostname) || a.path.localeCompare(b.path));
  const visible = routes.filter((route) =>
    `${route.hostname}${route.path}`.includes(query.trim().toLowerCase()) &&
    (application === "all" || route.app.id === application) &&
    (kind === "all" || route.kind === kind));
  const filtering = query.trim() !== "" || application !== "all" || kind !== "all";
  const clear = () => { setQuery(""); setApplication("all"); setKind("all"); };
  const add = (
    <Button size="sm" variant="primary" disabled={!published.length} onClick={() => setEditor({ app: null })}>
      <Icon name="plus" />Add route
    </Button>
  );

  return (
    <>
      <PageHeader actions={add}>
        <PageHeaderTitle>Routes</PageHeaderTitle>
        <PageHeaderDescription>Every hostname and path the HTTP proxy answers, and the Application it reaches.</PageHeaderDescription>
      </PageHeader>
      <div className="list-filters">
        <Search aria-label="Search routes" placeholder="Search routes..." value={query} onChange={(event) => setQuery(event.target.value)} />
        <Select value={application} onValueChange={setApplication}>
          <SelectTrigger aria-label="Filter by application"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All applications</SelectItem>
            {published.map((app) => <SelectItem key={app.id} value={app.id}>{app.name}</SelectItem>)}
          </SelectContent>
        </Select>
        <Select value={kind} onValueChange={setKind}>
          <SelectTrigger aria-label="Filter by type"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All types</SelectItem>
            {Object.entries(kindLabel).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}
          </SelectContent>
        </Select>
        {filtering ? <Button size="sm" variant="ghost" onClick={clear}>Clear filters</Button> : null}
      </div>
      <RouteTable routes={visible} total={routes.length} ready={ready} showApplication
        reload={reload}
        onEdit={(route) => setEditor({ app: route.app, route })}
        onOpenApp={onOpenApp}
        empty={
          <EmptyState variant="first-run">
            <EmptyStateTitle>No routes yet</EmptyStateTitle>
            <EmptyStateDescription>An Application deployed with a Hostname brings its first route.</EmptyStateDescription>
          </EmptyState>
        } />
      <details className="disclosure">
        <summary>How the proxy picks a route</summary>
        <ol>
          <li>It matches the hostname, in any letter case.</li>
          <li>Among that hostname's routes, the longest matching path wins. <code>/app</code> takes <code>/app/assets</code> but not <code>/apple</code>.</li>
          <li>Each Hostname and alias has a <code>/</code> route to its Application unless a path route says otherwise.</li>
          <li>A hostname with no route answers 404. <code>admin.{dnsSuffix}</code> always belongs to the Platform.</li>
        </ol>
      </details>
      <RouteDialog editor={editor} apps={apps} dnsSuffix={dnsSuffix} onClose={() => setEditor(null)}
        onSaved={(message) => {
          setEditor(null);
          void reload().then(() => notify("success", message));
        }} />
    </>
  );
}
