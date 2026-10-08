import { useState, type ReactNode } from "react";
import { useQueries } from "@tanstack/react-query";
import {
  StatusBadge,
  Button,
  Card,
  CardContent,
  EmptyState,
  EmptyStateActions,
  EmptyStateDescription,
  EmptyStateIcon,
  EmptyStateTitle,
  PageHeader,
  PageHeaderTitle,
  Search,
  Sparkline,
  StepBar,
  Table,
  TableBody,
  TableCell,
  TableFrame,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { definitionOf, describeDefinition } from "../components/DefinitionPicker.js";
import { Icon } from "../components/Icon.js";
import { formatBytes } from "../lib/format.js";
import { databaseQuery } from "../lib/queries.js";
import { routesOf } from "../lib/routes.js";
import { NOUNS, TITLES, barSteps, phase, position } from "../lib/runSteps.js";
import { statusTone } from "../lib/status.js";
import { hostTotals, machineSeriesFor, seriesFor } from "../lib/useMetrics.js";
import { useNativeCapabilities } from "../lib/useNativeCapabilities.js";
import type { App, AppSample, Environment, Metrics } from "../lib/types.js";

const matches = (name: string, query: string) =>
  name.toLowerCase().includes(query.trim().toLowerCase());

/** A status the way a badge says it: "Running", not "running". */
const statusLabel = (status: string) => status.charAt(0).toUpperCase() + status.slice(1);

/** What an Application's definition names: the image, the repository, the command. */
function sourceOf(app: App): string {
  if (app.runtime?.kind === "native") return app.runtime.command.join(" ");
  return app.git?.repository ?? app.image;
}

export function Overview({
  apps,
  environments,
  metrics,
  onOpenApp,
  onOpenEnvironment,
  onDeploy,
  onNewDatabase,
  onNewMachine,
  onOpenDatabases,
  onOpenRoutes,
}: {
  apps: App[];
  environments: Environment[];
  metrics: Metrics | null;
  onOpenApp: (id: string) => void;
  onOpenEnvironment: (id: string) => void;
  onDeploy: () => void;
  onNewDatabase: () => void;
  onNewMachine: () => void;
  onOpenDatabases: () => void;
  onOpenRoutes: () => void;
}) {
  // Each panel holds its own term. One box filtering every list said nothing
  // about which list it was thinning.
  const [appSearch, setAppSearch] = useState("");
  const [databaseSearch, setDatabaseSearch] = useState("");
  const [machineSearch, setMachineSearch] = useState("");
  const { capabilities } = useNativeCapabilities();
  const applications = apps.filter((app) => !app.managed_postgres);
  const databases = apps.filter((app) => app.managed_postgres);

  // A database's connections are the only record of who uses it.
  const details = useQueries({
    queries: databases.map((database) => ({ ...databaseQuery(database.id), refetchInterval: 15_000 })),
  });
  const usedBy = new Map(databases.map((database, index) => [
    database.id,
    details[index]?.data?.connections
      .filter((connection) => connection.status !== "revoked")
      .map((connection) => apps.find((app) => app.id === connection.consumer_application_id)?.name ?? "Removed application"),
  ]));

  const visibleApps = applications.filter((app) => matches(app.name, appSearch));
  const visibleDatabases = databases.filter((database) => matches(database.name, databaseSearch));
  const machines = environments.filter((one) => matches(one.config.name, machineSearch));
  const empty = apps.length === 0 && environments.length === 0;

  return (
    <>
      <PageHeader>
        <PageHeaderTitle>Overview</PageHeaderTitle>
      </PageHeader>

      {empty ? (
        <Card>
          <EmptyState variant="first-run" className="hatch">
            <EmptyStateIcon>
              <Icon name="box" size="lg" />
            </EmptyStateIcon>
            <EmptyStateTitle>Nothing running yet</EmptyStateTitle>
            <EmptyStateDescription>
              Deploy an application from a container image or a Compose file, or create a virtual
              machine to work in. Both are reachable from your LAN.
            </EmptyStateDescription>
            <EmptyStateActions>
              <Button size="sm" variant="primary" onClick={onDeploy}>
                <Icon name="plus" />
                Deploy application
              </Button>
              <Button size="sm" onClick={onNewMachine}>
                <Icon name="plus" />
                Create virtual machine
              </Button>
            </EmptyStateActions>
          </EmptyState>
        </Card>
      ) : (
        <div className="stack">
          <Card className="summary-panel">
            <CardContent>
              <GroupedSummary apps={applications} databases={databases} environments={environments} metrics={metrics} onOpenDatabases={onOpenDatabases} onOpenRoutes={onOpenRoutes} />
            </CardContent>
          </Card>

          <TableFrame>
            <div className="table-toolbar panel-toolbar">
              <h2 className="section-title">Applications</h2>
              <Search
                aria-label="Search applications"
                containerClassName="section-search"
                placeholder="Search applications…"
                value={appSearch}
                onChange={(event) => setAppSearch(event.target.value)}
              />
              <Button size="sm" variant="ghost" className="panel-action" onClick={onDeploy}>
                <Icon name="plus" />
                Deploy application
              </Button>
            </div>
            <div className="table-scroll">
              <Table aria-label="Applications">
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Application</TableHead>
                    <TableHead scope="col">Hostname</TableHead>
                    <TableHead scope="col">Status</TableHead>
                    <TableHead scope="col">CPU</TableHead>
                    <TableHead scope="col">Memory</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {visibleApps.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={5} className="muted">
                        {applications.length ? "No applications match your search." : "No applications yet."}
                      </TableCell>
                    </TableRow>
                  ) : (
                    visibleApps.map((app) => {
                      const definition = describeDefinition(definitionOf(app));
                      const samples = seriesFor(metrics, app.id);
                      // A native process tree is measured only where the Host has resource controls.
                      const unmeasured = app.runtime?.kind === "native" && capabilities?.metrics === false;
                      return (
                        <TableRow
                          key={app.id}
                          tabIndex={0}
                          role="button"
                          aria-label={`Open ${app.name}`}
                          onClick={() => onOpenApp(app.id)}
                          onKeyDown={(event) => {
                            if (event.key !== "Enter" && event.key !== " ") return;
                            event.preventDefault();
                            onOpenApp(app.id);
                          }}
                        >
                          <TableCell>
                            <span className="app-cell">
                              <Icon name={definition.icon} size="md" />
                              <span className="stack-xs">
                                <strong>{app.name}</strong>
                                <span className="muted t-label">{definition.label} · <span className="mono">{sourceOf(app)}</span></span>
                              </span>
                            </span>
                          </TableCell>
                          <TableCell className="mono">
                            {app.hostname || <span className="muted">Unpublished</span>}
                            {app.aliases?.length ? <span className="t-metadata muted"> +{app.aliases.length}</span> : null}
                          </TableCell>
                          <TableCell>
                            <StatusBadge tone={statusTone(app.status)}>{statusLabel(app.status)}</StatusBadge>
                          </TableCell>
                          <Usage
                            samples={samples}
                            unmeasured={unmeasured}
                            value={(sample) => sample.cpu_percent}
                            render={(sample) => `${sample.cpu_percent.toFixed(1)}%`}
                          />
                          <Usage
                            samples={samples}
                            unmeasured={unmeasured}
                            value={(sample) => sample.memory_bytes}
                            render={(sample) => formatBytes(sample.memory_bytes)}
                          />
                        </TableRow>
                      );
                    })
                  )}
                </TableBody>
              </Table>
            </div>
          </TableFrame>

          <TableFrame>
            <div className="table-toolbar panel-toolbar">
              <h2 className="section-title">Databases</h2>
              <Search
                aria-label="Search databases"
                containerClassName="section-search"
                placeholder="Search databases…"
                value={databaseSearch}
                disabled={!databases.length}
                onChange={(event) => setDatabaseSearch(event.target.value)}
              />
              <Button size="sm" variant="ghost" className="panel-action" onClick={onNewDatabase}>
                <Icon name="plus" />
                New database
              </Button>
            </div>
            {databases.length === 0 ? (
              <EmptyState variant="first-run" size="md">
                <EmptyStateTitle>No databases yet</EmptyStateTitle>
              </EmptyState>
            ) : (
              <div className="table-scroll">
                <Table aria-label="Databases">
                  <TableHeader>
                    <TableRow>
                      <TableHead scope="col">Database</TableHead>
                      <TableHead scope="col">Status</TableHead>
                      <TableHead scope="col">Used by</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {visibleDatabases.length === 0 ? (
                      <TableRow>
                        <TableCell colSpan={3} className="muted">No databases match your search.</TableCell>
                      </TableRow>
                    ) : (
                      visibleDatabases.map((database) => {
                        const consumers = usedBy.get(database.id);
                        return (
                          <TableRow
                            key={database.id}
                            tabIndex={0}
                            role="button"
                            aria-label={`Open ${database.name}`}
                            onClick={() => onOpenApp(database.id)}
                            onKeyDown={(event) => {
                              if (event.key !== "Enter" && event.key !== " ") return;
                              event.preventDefault();
                              onOpenApp(database.id);
                            }}
                          >
                            <TableCell>
                              <span className="stack-xs">
                                <strong>{database.name}</strong>
                                <span className="muted t-label">PostgreSQL {database.managed_postgres?.major}</span>
                              </span>
                            </TableCell>
                            <TableCell>
                              <StatusBadge tone={statusTone(database.status)}>{statusLabel(database.status)}</StatusBadge>
                            </TableCell>
                            <TableCell>
                              {consumers === undefined ? <span className="muted">—</span>
                                : consumers.length ? consumers.join(", ")
                                  : <span className="muted">No Application</span>}
                            </TableCell>
                          </TableRow>
                        );
                      })
                    )}
                  </TableBody>
                </Table>
              </div>
            )}
          </TableFrame>

          <TableFrame>
            <div className="table-toolbar panel-toolbar">
              <h2 className="section-title">Virtual machines</h2>
              <Search
                aria-label="Search virtual machines"
                containerClassName="section-search"
                placeholder="Search machines…"
                value={machineSearch}
                disabled={!environments.length}
                onChange={(event) => setMachineSearch(event.target.value)}
              />
              <Button size="sm" variant="ghost" className="panel-action" onClick={onNewMachine}>
                <Icon name="plus" />
                Create virtual machine
              </Button>
            </div>
            {environments.length === 0 ? (
              <EmptyState variant="first-run" size="md">
                <EmptyStateTitle>No virtual machines yet</EmptyStateTitle>
              </EmptyState>
            ) : (
              <div className="table-scroll">
                <Table aria-label="Virtual machines">
                  <TableHeader>
                    <TableRow>
                      <TableHead scope="col">Virtual machine</TableHead>
                      <TableHead scope="col">State</TableHead>
                      <TableHead scope="col">Service</TableHead>
                      <TableHead scope="col">Activity</TableHead>
                      <TableHead scope="col">CPU</TableHead>
                      <TableHead scope="col">Memory</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {machines.length === 0 ? (
                      <TableRow>
                        <TableCell colSpan={6} className="muted">
                          No virtual machines match your search.
                        </TableCell>
                      </TableRow>
                    ) : (
                      machines.map((one) => (
                        <TableRow
                          key={one.id}
                          tabIndex={0}
                          role="button"
                          aria-label={`Open ${one.config.name}`}
                          onClick={() => onOpenEnvironment(one.id)}
                          onKeyDown={(event) => {
                            if (event.key !== "Enter" && event.key !== " ") return;
                            event.preventDefault();
                            onOpenEnvironment(one.id);
                          }}
                        >
                          <TableCell>{one.config.name}</TableCell>
                          <TableCell>
                            <StatusBadge tone={statusTone(one.state)}>
                              {one.state}
                            </StatusBadge>
                          </TableCell>
                          <TableCell>
                            {/* No service yet on a machine still being created:
                                "disabled" would say something that is not so. */}
                            {one.operation?.status === "running" &&
                            one.operation.action === "create" &&
                            !one.service_ready ? (
                              <span className="muted">—</span>
                            ) : (
                              <StatusBadge
                                tone={one.service_ready ? "success" : "neutral"}
                              >
                                {one.service_ready ? "Ready" : "Disabled"}
                              </StatusBadge>
                            )}
                          </TableCell>
                          {/* What it is doing right now is the only reason to
                              look at a machine mid-bootstrap: the action, one
                              segment per step, and how far. An operation that
                              ended well is history: it said "create: ready" at
                              a machine being deleted. A failed one stays until
                              the retry, with its red segment where it stopped. */}
                          <TableCell>
                            {one.operation?.status === "running" ||
                            one.operation?.status === "failed" ? (
                              <Activity operation={one.operation} />
                            ) : (
                              <span className="muted">Idle</span>
                            )}
                          </TableCell>
                          <Usage
                            samples={machineSeriesFor(metrics, one.id)}
                            value={(sample) => sample.cpu_percent}
                            render={(sample) => `${sample.cpu_percent.toFixed(1)}%`}
                          />
                          <Usage
                            samples={machineSeriesFor(metrics, one.id)}
                            value={(sample) => sample.memory_bytes}
                            render={(sample) => formatBytes(sample.memory_bytes)}
                          />
                        </TableRow>
                      ))
                    )}
                  </TableBody>
                </Table>
              </div>
            )}
          </TableFrame>
        </div>
      )}
    </>
  );
}

/**
 * The Overview's summary, in one panel: four questions, a caps label over
 * each. What is up, what the measured workloads cost the Host, and what the
 * Platform's proxy and DNS handled. The reading is the answer; the panels
 * below are where anything deeper gets inspected.
 */
function GroupedSummary({ apps, databases, environments, metrics, onOpenDatabases, onOpenRoutes }: {
  apps: App[];
  databases: App[];
  environments: Environment[];
  metrics: Metrics | null;
  onOpenDatabases: () => void;
  onOpenRoutes: () => void;
}) {
  const running = apps.filter((app) => app.status === "running").length;
  const routes = apps.flatMap(routesOf).length;
  const awake = environments.filter((one) => one.state === "running").length;
  const totals = hostTotals(metrics);
  const platform = metrics?.platform ?? [];
  const latest = platform[platform.length - 1];
  // Proxy and DNS count events over the same interval and share one scale
  // (ADR-0020), so both speak per minute.
  const interval = Math.max(1, metrics?.interval_seconds ?? 10);
  const perMinute = (count: number) => Math.round((count * 60) / interval);
  const windowMinutes = Math.max(1, Math.round((platform.length * interval) / 60));

  return (
    <div className="summary-groups">
      <div className="summary-group">
        <p className="t-caps">Workloads</p>
        <p className="summary-line">
          <b>{running} of {apps.length}</b> apps running
          {databases.length ? <>
            <span className="sep">·</span>
            <a href="/console/#databases" onClick={(event) => {
              if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
              event.preventDefault();
              onOpenDatabases();
            }}><b>{databases.filter((database) => database.status === "running").length} of {databases.length}</b> databases</a>
          </> : null}
          {environments.length ? <>
            <span className="sep">·</span>
            <b>{awake} of {environments.length}</b> VMs
          </> : null}
        </p>
      </div>
      <div className="summary-group">
        <p className="t-caps">Measured use</p>
        <p className="summary-line">
          <span className="k">CPU</span> <b>{totals ? `${totals.cpu.toFixed(1)}%` : "—"}</b>
          <span className="sep">·</span>
          <span className="k">Memory</span> <b>{totals ? formatBytes(totals.memory) : "—"}</b>
          {totals ? ` of ${formatBytes(totals.memoryLimit)}` : ""}
        </p>
      </div>
      <Rate label="Proxy" noun="requests" counts={platform.map((sample) => perMinute(sample.proxy_requests))}
        latest={latest ? perMinute(latest.proxy_requests) : null} windowMinutes={windowMinutes}>
        {routes ? <>
          <span className="sep">·</span>
          <a href="/console/#routes" onClick={(event) => {
            if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
            event.preventDefault();
            onOpenRoutes();
          }}><b>{routes}</b> {routes === 1 ? "route" : "routes"}</a>
        </> : null}
      </Rate>
      <Rate label="DNS" noun="queries" counts={platform.map((sample) => perMinute(sample.dns_queries))}
        latest={latest ? perMinute(latest.dns_queries) : null} windowMinutes={windowMinutes} />
    </div>
  );
}

/** One server's rate and its shape, or a sentence when the window saw nothing, then what else the line says. */
function Rate({ label, noun, counts, latest, windowMinutes, children }: {
  label: string;
  noun: string;
  counts: number[];
  latest: number | null;
  windowMinutes: number;
  children?: ReactNode;
}) {
  return (
    <div className="summary-group">
      <p className="t-caps">{label}</p>
      <div className="summary-line">
        {latest === null ? <b>—</b>
          : counts.some(Boolean) ? <><b>{latest}/min</b><Sparkline values={counts} height={14} /></>
            : <span>No {noun} in the last {windowMinutes} {windowMinutes === 1 ? "minute" : "minutes"}</span>}
        {children}
      </div>
    </div>
  );
}

/**
 * A running or failed operation in a table cell: the badge says the phase
 * the run is in ("Starting", "Provisioning") or what went wrong ("Bootstrap
 * failed"), the bar shows where it is, the count says how far. No ticker
 * here; the machine's own screen has the output.
 */
function Activity({ operation }: { operation: NonNullable<Environment["operation"]> }) {
  const failed = operation.status === "failed";
  const said = failed
    ? `${NOUNS[operation.action] ?? operation.action} failed`
    : phase(operation.action, operation.step);
  const place = position(operation.action, operation.step);
  return (
    <span
      className="activity-cell"
      title={operation.step ? `${said}: ${operation.step}` : said}
    >
      <StatusBadge tone={failed ? "danger" : "success"} pulse={!failed}>
        {said}
      </StatusBadge>
      <StepBar
        label={TITLES[operation.action] ?? operation.action}
        steps={barSteps(operation.action, operation.step, failed)}
      />
      {place ? (
        <span className="mono muted t-label">
          {place.at}/{place.of}
        </span>
      ) : null}
    </span>
  );
}

/**
 * A workload's usage in a table cell: the last reading with the window's
 * shape beside it. A dash until the collector's first tick, which is up to a
 * minute after the workload starts, and "Not measured" for a native process
 * on a Host that cannot measure one.
 */
function Usage({
  samples,
  unmeasured,
  value,
  render,
}: {
  samples: AppSample[] | null;
  unmeasured?: boolean;
  value: (sample: AppSample) => number;
  render: (sample: AppSample) => string;
}) {
  if (unmeasured) {
    return (
      <TableCell className="muted" title="This Host has no resource controls for native processes">Not measured</TableCell>
    );
  }
  if (!samples) {
    return <TableCell className="muted">—</TableCell>;
  }
  return (
    <TableCell>
      <span className="usage-inline">
        <span className="usage-value">{render(samples[samples.length - 1])}</span>
        <Sparkline values={samples.map(value)} height={16} />
      </span>
    </TableCell>
  );
}
