import { useState } from "react";
import { Button, EmptyState, EmptyStateTitle, Search } from "@momoi-labs/kiso-react";

import { routesOf, type RouteKind } from "../lib/routes.js";
import type { App } from "../lib/types.js";
import { Icon } from "./Icon.js";
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
  const routes = routesOf(app)
    .sort((a, b) => order[a.kind] - order[b.kind] || a.hostname.localeCompare(b.hostname) || a.path.localeCompare(b.path));
  const visible = routes.filter((route) => `${route.hostname}${route.path}`.includes(query.trim().toLowerCase()));

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
      <RouteDialog editor={editor} apps={apps} dnsSuffix={dnsSuffix} onClose={() => setEditor(null)}
        onSaved={(message) => {
          setEditor(null);
          void reload().then(() => notify("success", message));
        }} />
    </div>
  );
}
