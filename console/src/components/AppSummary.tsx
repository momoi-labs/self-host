import { Fragment, useState, type KeyboardEvent, type ReactNode } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import {
  Button, Diagram, DiagramColumn, DiagramEdge, DiagramNode, GridPane, KV, KVKey, KVValue, PaneGrid, Search, StatusBadge,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
  type DiagramKind, type DiagramStatus,
} from "@momoi-labs/kiso-react";
import { Layers, SquareTerminal, type LucideIcon } from "lucide-react";
import { join } from "shlex";

import { api, failureOf, readJson } from "../lib/api.js";
import { formatBytes } from "../lib/format.js";
import { databaseQuery } from "../lib/queries.js";
import type { App } from "../lib/types.js";
import { useNativeCapabilities } from "../lib/useNativeCapabilities.js";
import { useStoredLayout } from "../lib/useStoredLayout.js";
import { definitionOf, describeDefinition } from "./DefinitionPicker.js";
import { Icon } from "./Icon.js";
import { useToast } from "./Toasts.js";

/** How the console draws its diagrams: solid, rounded nodes without corner marks, on a quiet dot grid. */
export const diagramLook = {
  borderStyle: "solid",
  cornerStyle: "rounded",
  cornerSize: "medium",
  cornerMarks: "none",
  background: "dots",
  strength: "quiet",
} as const;

/** A way in, as the diagram draws it: the URL a Consumer opens. */
type Route = { key: string; label: string; url: string };

/** The Hostname, its aliases, then the paths routed to the Application. */
function routesOf(app: App): Route[] {
  if (app.publication?.kind === "unpublished" || !app.hostname) return [];
  const url = (host: string, path = "/") => `https://${host}${path === "/" ? "" : path}`;
  return [
    { key: `hostname:${app.hostname}`, label: "Hostname", url: url(app.hostname) },
    ...(app.aliases ?? []).map((alias) => ({ key: `alias:${alias}`, label: "Alias", url: url(alias) })),
    ...(app.route_rules ?? []).map((rule) => ({ key: `path:${rule.hostname}${rule.path_prefix}`, label: "Path", url: url(rule.hostname, rule.path_prefix) })),
  ];
}

/** How the diagram draws an Application: the kind kiso has for it, or an icon of its own. */
export function nodeOf(app: App): { kind: DiagramKind; icon?: LucideIcon; label: string } {
  const definition = definitionOf(app);
  const label = describeDefinition(definition).label;
  if (definition === "git") return { kind: "repository", label };
  if (definition === "compose") return { kind: "service", icon: Layers, label };
  if (definition === "native") return { kind: "service", icon: SquareTerminal, label };
  return { kind: "image", label };
}

/** A workload that is not up draws its node as stopped or failed. */
export function statusOf(status: string): DiagramStatus | undefined {
  return status === "stopped" ? "stopped" : status === "failed" ? "failed" : undefined;
}

/** Props that make a diagram node open something, by pointer or keyboard. */
export function opens(open: () => void) {
  return {
    role: "button" as const,
    tabIndex: 0,
    className: "card-interactive",
    onClick: open,
    onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      open();
    },
  };
}

/** Where the Hostname lands, as the edge into the Application says it. */
function portOf(app: App): string | undefined {
  if (app.runtime?.kind === "native") return app.runtime.port ? `:${app.runtime.port}` : undefined;
  const port = app.development?.web_port ?? app.web_port ?? 80;
  return app.web_service ? `${app.web_service}:${port}` : `:${port}`;
}

/** What the definition names: the command, the repository or the image. */
function sourceOf(app: App): string {
  if (app.runtime?.kind === "native") return join(app.runtime.command);
  return app.git?.repository ?? app.image;
}

/**
 * An Application's Summary as a working view: what reaches it and what it
 * uses drawn at the top, then panes the Operator can move and resize, each
 * with the way to change what it shows.
 */
export function AppSummary({ app, apps, busy, onOpenRoutes, onOpenConfiguration, onOpenApp, onRebuild }: {
  app: App;
  apps: App[];
  /** An operation is running, so nothing new starts from here. */
  busy: boolean;
  onOpenRoutes: () => void;
  onOpenConfiguration: () => void;
  onOpenApp: (id: string) => void;
  onRebuild: () => void;
}) {
  const { capabilities } = useNativeCapabilities();
  const native = app.runtime?.kind === "native" ? app.runtime : null;
  const [layout, saveLayout] = useStoredLayout(native ? "summary:native" : "summary:container");
  const routes = routesOf(app);
  const shown = routes.slice(0, 3);
  const node = nodeOf(app);

  // A database's connections are the only record of who uses it.
  const databases = apps.filter((one) => one.managed_postgres);
  const details = useQueries({
    queries: databases.map((database) => ({ ...databaseQuery(database.id), refetchInterval: 15_000 })),
  });
  const connected = databases.flatMap((database, index) =>
    (details[index]?.data?.connections ?? [])
      .filter((connection) => connection.consumer_application_id === app.id && connection.status !== "revoked")
      .map((connection) => ({ database, variable: connection.variable })));
  const edit = <Button size="sm" variant="ghost" onClick={onOpenConfiguration}>Edit</Button>;

  return (
    <div className="stack">
      <Diagram label={`${app.name}: the routes that reach it and the databases it uses`} {...diagramLook}>
        {routes.length ? (
          <DiagramColumn>
            {shown.map((route) => (
              <DiagramNode key={route.key} id={route.key} kind="route" label={route.label} title={route.url} {...opens(onOpenRoutes)} />
            ))}
            {routes.length > shown.length ? (
              <DiagramNode id="more" kind="route" label="Routes" title={`+${routes.length - shown.length} more`} {...opens(onOpenRoutes)} />
            ) : null}
          </DiagramColumn>
        ) : null}
        <DiagramColumn>
          <DiagramNode id="app" kind={node.kind} icon={node.icon} label={node.label} title={app.name} accent status={statusOf(app.status)}>
            {sourceOf(app)}
          </DiagramNode>
        </DiagramColumn>
        {/* A database shows only once the Application is connected to it. */}
        {connected.length ? (
          <DiagramColumn>
            {connected.map(({ database }) => (
              <DiagramNode key={database.id} id={database.id} kind="database" title={database.name} status={statusOf(database.status)}
                {...opens(() => onOpenApp(database.id))}>
                {database.image}
              </DiagramNode>
            ))}
          </DiagramColumn>
        ) : null}
        {routes.length ? (
          <DiagramEdge from={[...shown.map((route) => route.key), ...(routes.length > shown.length ? ["more"] : [])]} to="app" label={portOf(app)} />
        ) : null}
        {connected.map(({ database, variable }) => (
          <DiagramEdge key={database.id} from="app" to={database.id} label={variable} />
        ))}
      </Diagram>

      <PaneGrid aria-label={`${app.name} summary`} defaultLayout={layout} onLayoutChange={saveLayout}>
        {native ? (
          <GridPane id="start-command" title="Start command" size={12} actions={<>
            <Button size="sm" variant="ghost" onClick={() => void navigator.clipboard?.writeText(join(native.command))}>Copy</Button>
            {edit}
          </>}>
            <pre><code>{join(native.command)}</code></pre>
            <p className="muted t-label">Runs as <code>{native.account}</code> in {native.working_dir || "the Application home"}</p>
            {capabilities?.resource_limits ? <p className="muted t-label">{limitsOf(native.limits)}</p> : null}
          </GridPane>
        ) : (
          <GridPane id="what-runs" title="What runs" size={6} actions={<>
            {app.git_build ? <Button size="sm" variant="ghost" disabled={busy} onClick={onRebuild}>Rebuild</Button> : null}
            {edit}
          </>}>
            <WhatRuns app={app} />
          </GridPane>
        )}
        {native ? (
          <SearchPane id="packages" size={6} newRow title="Packages" noun="packages" action={edit} columns={["Package", "Version", "Options"]}
            rows={(native.recipe?.dependencies ?? []).map((dependency) => {
              const options = [
                ...(dependency.allow_builds?.length ? [`allow_builds=${dependency.allow_builds.join(",")}`] : []),
                ...Object.entries(dependency.options ?? {}).map(([name, values]) => `${name}=${values.join(",")}`),
              ].join(" ");
              return {
                key: dependency.tool,
                cells: [
                  <span className="mono">{dependency.tool}</span>,
                  <span className="mono">{dependency.version}</span>,
                  options ? <span className="mono">{options}</span> : <span className="muted">None</span>,
                ],
              };
            })} />
        ) : null}
        <Variables id="variables" size={6} app={app} />
      </PaneGrid>

      {native && capabilities?.metrics === false && capabilities.resource_limits === false ? (
        <p className="muted t-label summary-footnote">
          <Icon name="info" /> This Host has no resource controls for native processes, so CPU and memory are not measured or limited.
        </p>
      ) : null}
    </div>
  );
}

/** A native process tree's limits in one line; a limit that is not set is not one. */
function limitsOf(limits: { cpu_percent?: number; memory_bytes?: number; max_tasks?: number } | undefined): string {
  const set = [
    limits?.cpu_percent ? `CPU ${limits.cpu_percent}%` : null,
    limits?.memory_bytes ? `Memory ${formatBytes(limits.memory_bytes)}` : null,
    limits?.max_tasks ? `${limits.max_tasks} tasks` : null,
  ].filter(Boolean);
  return set.length ? `Limits: ${set.join(" · ")}` : "No resource limits";
}

/** The facts a container definition names: its image, its repository and build, or its services. */
function WhatRuns({ app }: { app: App }) {
  const definition = definitionOf(app);
  if (app.git) {
    return (
      <KV>
        <KVKey>Repository</KVKey><KVValue>{app.git.repository}</KVValue>
        <KVKey>Branch or tag</KVKey><KVValue>{app.git.git_ref}</KVValue>
        <KVKey>Built from</KVKey><KVValue>{app.git.compose_path || app.git.dockerfile}</KVValue>
        {app.git_build ? <>
          <KVKey>Deployed commit</KVKey><KVValue>{app.git_build.revision}</KVValue>
          <KVKey>Build result</KVKey>
          <KVValue><StatusBadge tone={app.git_build.status === "completed" ? "success" : "neutral"}>{app.git_build.status}</StatusBadge></KVValue>
          {Object.entries(app.git_build.images).map(([service, image]) => (
            <Fragment key={service}><KVKey>{service} image</KVKey><KVValue>{image}</KVValue></Fragment>
          ))}
        </> : null}
      </KV>
    );
  }
  if (definition === "compose") {
    const services = [...new Set((app.services ?? []).map((service) => service.service))];
    return (
      <KV>
        <KVKey>Services</KVKey>
        <KVValue>{services.length
          ? <ul className="stack-xs value-list">{services.map((service) => <li key={service}>{service}</li>)}</ul>
          : <span className="muted">None running</span>}</KVValue>
        {app.web_service ? <><KVKey>Web service</KVKey><KVValue>{app.web_service}</KVValue></> : null}
      </KV>
    );
  }
  return (
    <KV>
      <KVKey>Image</KVKey><KVValue>{app.image}</KVValue>
      {app.development ? <><KVKey>Start command</KVKey><KVValue>{app.development.command}</KVValue></> : null}
    </KV>
  );
}

/**
 * The Application's variables by name. Values stay hidden until the
 * Operator asks for them, and the columns keep their width either way.
 */
function Variables({ id, size, app }: { id: string; size: number; app: App }) {
  const notify = useToast();
  const names = useQuery({
    queryKey: ["apps", app.id, "variable-names"],
    queryFn: ({ signal }) => readJson<string[]>(`/apps/id/${encodeURIComponent(app.id)}/variable-names`, signal),
  });
  const [values, setValues] = useState<Map<string, string> | null>(null);
  const [revealing, setRevealing] = useState(false);
  const list = names.data ?? [];

  async function toggle() {
    if (values) { setValues(null); return; }
    setRevealing(true);
    try {
      const response = await api(`/apps/id/${encodeURIComponent(app.id)}/env`);
      if (!response.ok) { notify("danger", "Could not show the values", await failureOf(response)); return; }
      setValues(new Map(await response.json() as [string, string][]));
    } catch (cause) {
      notify("danger", "Could not show the values", (cause as Error).message);
    } finally {
      setRevealing(false);
    }
  }

  return (
    <SearchPane id={id} size={size} title="Variables" noun="variables" columns={["Name", "Value"]} className="variables-table"
      message={names.isPending ? "Loading variables…" : names.isError ? "Could not load the variable names." : undefined}
      action={
        <Button size="sm" variant="ghost" className="btn-icon" aria-pressed={values !== null} disabled={!list.length || revealing}
          aria-label={values ? "Hide values" : "Show values"} title={values ? "Hide values" : "Show values"}
          onClick={() => void toggle()}>
          <Icon name={values ? "eye-off" : "eye"} />
        </Button>
      }
      rows={list.map((name) => ({
        key: name,
        cells: [
          <span className="mono">{name}</span>,
          values?.has(name)
            ? <span className="mono variable-value" title={values.get(name)}>{values.get(name)}</span>
            : <span className="mono muted">***************</span>,
        ],
      }))} />
  );
}

/**
 * A pane holding a short table: its title, a search and an action on the
 * pane's head, then the rows the search leaves. The search reads a row's
 * key, its name.
 */
function SearchPane({ id, size, newRow, title, noun, action, columns, rows, className, message }: {
  id: string;
  size: number;
  newRow?: boolean;
  title: string;
  noun: string;
  action: ReactNode;
  columns: string[];
  rows: { key: string; cells: ReactNode[] }[];
  className?: string;
  /** Said in place of the rows while they load or when they could not. */
  message?: string;
}) {
  const [query, setQuery] = useState("");
  const visible = rows.filter((row) => row.key.toLowerCase().includes(query.trim().toLowerCase()));
  return (
    <GridPane id={id} size={size} newRow={newRow} title={title} actions={<>
      <Search aria-label={`Search ${noun}`} containerClassName="pane-search" placeholder={`Search ${noun}…`}
        value={query} disabled={!rows.length} onChange={(event) => setQuery(event.target.value)} />
      {action}
    </>}>
      <div className="table-scroll">
        <Table aria-label={title} className={className}>
          <TableHeader>
            <TableRow>{columns.map((column) => <TableHead key={column} scope="col">{column}</TableHead>)}</TableRow>
          </TableHeader>
          <TableBody>
            {message || visible.length === 0 ? (
              <TableRow>
                <TableCell colSpan={columns.length} className="muted">
                  {message ?? (rows.length ? `No ${noun} match your search.` : `No ${noun}.`)}
                </TableCell>
              </TableRow>
            ) : visible.map((row) => (
              <TableRow key={row.key}>{row.cells.map((cell, index) => <TableCell key={index}>{cell}</TableCell>)}</TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </GridPane>
  );
}
