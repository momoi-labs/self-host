import { useState } from "react";
import {
  Button,
  Card,
  EmptyState,
  EmptyStateActions,
  EmptyStateDescription,
  EmptyStateIcon,
  EmptyStateTitle,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Search,
  Sparkline,
  StepBar,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { formatBytes } from "../lib/format.js";
import { NOUNS, TITLES, barSteps, phase, position } from "../lib/runSteps.js";
import { statusTone } from "../lib/status.js";
import { hostNetworkSeries, hostTotals, machineSeriesFor, seriesFor } from "../lib/useMetrics.js";
import type { App, AppSample, Environment, Metrics } from "../lib/types.js";

type Row = {
  key: string;
  id: string;
  name: string;
  hostname?: string;
  aliases: number;
  image: string;
  status: string;
  restarts?: number | null;
  /** Absent until the collector's first tick names this Application. */
  samples: AppSample[] | null;
};

const matches = (name: string, query: string) =>
  name.toLowerCase().includes(query.trim().toLowerCase());

export function Overview({
  apps,
  environments,
  dnsSuffix,
  metrics,
  onOpenApp,
  onOpenEnvironment,
  onDeploy,
  onNewMachine,
}: {
  apps: App[];
  environments: Environment[];
  dnsSuffix: string;
  metrics: Metrics | null;
  onOpenApp: (id: string) => void;
  onOpenEnvironment: (id: string) => void;
  onDeploy: () => void;
  onNewMachine: () => void;
}) {
  // Each section holds its own term. The Overview lists two kinds of workload
  // and one box filtering both said nothing about which list it was thinning.
  const [appQuery, setAppQuery] = useState("");
  const [machineQuery, setMachineQuery] = useState("");

  const rows: Row[] = apps.map((app) => ({
    key: app.id,
    id: app.id,
    name: app.name,
    hostname: app.hostname,
    aliases: (app.aliases ?? []).length,
    image: app.image,
    status: app.status,
    restarts: app.restarts,
    samples: seriesFor(metrics, app.id),
  }));

  const visible = rows.filter((row) => matches(row.name, appQuery));
  const machines = environments.filter((one) =>
    matches(one.config.name, machineQuery),
  );
  const empty = rows.length === 0 && environments.length === 0;

  const open = (row: Row) => onOpenApp(row.id);

  return (
    <>
      <PageHeader>
        <PageHeaderTitle>Overview</PageHeaderTitle>
        <PageHeaderDescription>
          {apps.length === 0 && environments.length === 0
            ? `Nothing deployed on ${dnsSuffix} yet.`
            : [
                apps.length === 0
                  ? null
                  : `${apps.length} ${apps.length === 1 ? "application" : "applications"}`,
                environments.length === 0
                  ? null
                  : `${environments.length} ${environments.length === 1 ? "virtual machine" : "virtual machines"}`,
              ]
                .filter(Boolean)
                .join(" and ") + ` on ${dnsSuffix}`}
        </PageHeaderDescription>
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
          <GroupedSummary apps={apps} environments={environments} metrics={metrics} />

          <div className="section-heading">
            <h2 className="section-title">Applications</h2>
            <span className="muted t-label section-count">
              {visible.length} of {rows.length}
            </span>
            <Search
              aria-label="Search applications"
              containerClassName="section-search"
              placeholder="Search applications…"
              value={appQuery}
              onChange={(event) => setAppQuery(event.target.value)}
            />
            <Button size="sm" variant="ghost" onClick={onDeploy}>
              <Icon name="plus" />
              Deploy application
            </Button>
          </div>
          <div className="table-wrap">
            <div className="table-scroll">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Name</TableHead>
                    <TableHead scope="col">Hostname</TableHead>
                    <TableHead scope="col">Image</TableHead>
                    <TableHead scope="col">Status</TableHead>
                    <TableHead scope="col" className="num">
                      Restarts
                    </TableHead>
                    <TableHead scope="col">CPU</TableHead>
                    <TableHead scope="col">Memory</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {visible.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={7} className="muted">
                        No applications match your filters.
                      </TableCell>
                    </TableRow>
                  ) : (
                    visible.map((row) => (
                      <TableRow
                        key={row.key}
                        tabIndex={0}
                        role="button"
                        aria-label={`Open ${row.name}`}
                        onClick={() => open(row)}
                        onKeyDown={(event) => {
                          if (event.key !== "Enter" && event.key !== " ") return;
                          event.preventDefault();
                          open(row);
                        }}
                      >
                        <TableCell>{row.name}</TableCell>
                        <TableCell className="mono">
                          {row.hostname}
                          {row.aliases ? (
                            <span className="t-metadata muted"> +{row.aliases}</span>
                          ) : null}
                        </TableCell>
                        <TableCell className="mono">{row.image}</TableCell>
                        <TableCell>
                          <StatusBadge tone={statusTone(row.status)}>{row.status}</StatusBadge>
                        </TableCell>
                        <TableCell className="num">
                          {row.restarts === null || row.restarts === undefined ? (
                            <span className="muted">—</span>
                          ) : (
                            row.restarts
                          )}
                        </TableCell>
                        <Usage
                          samples={row.samples}
                          value={(sample) => sample.cpu_percent}
                          render={(sample) => `${sample.cpu_percent.toFixed(1)}%`}
                        />
                        <Usage
                          samples={row.samples}
                          value={(sample) => sample.memory_bytes}
                          render={(sample) => formatBytes(sample.memory_bytes)}
                        />
                      </TableRow>
                    ))
                  )}
                </TableBody>
              </Table>
            </div>
          </div>

          <div className="section-heading">
            <h2 className="section-title">Virtual machines</h2>
            <span className="muted t-label section-count">
              {machines.length} of {environments.length}
            </span>
            <Search
              aria-label="Search virtual machines"
              containerClassName="section-search"
              placeholder="Search machines…"
              value={machineQuery}
              onChange={(event) => setMachineQuery(event.target.value)}
            />
            <Button size="sm" variant="ghost" onClick={onNewMachine}>
              <Icon name="plus" />
              Create virtual machine
            </Button>
          </div>
          {environments.length === 0 ? (
            <p className="muted">No virtual machines yet.</p>
          ) : (
            <div className="table-wrap">
              <div className="table-scroll">
                <Table>
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
                          No virtual machines match your filters.
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
              <p className="table-footer">
                {machines.length} of {environments.length} virtual machines
              </p>
            </div>
          )}
        </div>
      )}
    </>
  );
}

/**
 * The Overview's summary: four questions on one line, a caps label over
 * each. What is up, what the workloads cost the Host, what crossed the
 * wire, and what the Platform's own servers handled. A line, not cards:
 * the reading is the answer, and the tables below are where anything
 * deeper gets inspected.
 */
function GroupedSummary({ apps, environments, metrics }: {
  apps: App[];
  environments: Environment[];
  metrics: Metrics | null;
}) {
  const running = apps.filter((app) => app.status === "running").length;
  const awake = environments.filter((one) => one.state === "running").length;
  const totals = hostTotals(metrics);
  const net = hostNetworkSeries(metrics);
  const latest = net[net.length - 1];
  const platform = metrics?.platform ?? [];
  const platformLatest = platform[platform.length - 1];
  // Proxy and DNS count events over the same interval and share one scale
  // (ADR-0020), so both speak per minute.
  const perMinute = (count: number) =>
    Math.round((count * 60) / Math.max(1, metrics?.interval_seconds ?? 10));
  const proxyPerMinute = platform.map((sample) => perMinute(sample.proxy_requests));
  const dnsPerMinute = platform.map((sample) => perMinute(sample.dns_queries));

  return (
    <div className="summary-groups">
      <div className="summary-group">
        <p className="t-caps">Workloads</p>
        <p className="summary-line">
          <b>{awake}</b>/{environments.length} VMs
          <span className="sep">·</span>
          <b>{running}</b>/{apps.length} apps
        </p>
      </div>
      <div className="summary-group">
        <p className="t-caps">Host</p>
        <p className="summary-line">
          <span className="k">CPU</span> <b>{totals ? `${totals.cpu.toFixed(1)}%` : "—"}</b>
          <span className="sep">·</span>
          <span className="k">Mem</span>{" "}
          <b>{totals ? formatBytes(totals.memory) : "—"}</b>
          {totals ? ` / ${formatBytes(totals.memoryLimit)}` : ""}
        </p>
      </div>
      <div className="summary-group">
        <p className="t-caps">Network</p>
        <p className="summary-line">
          <b>
            <span className="network-down">
              ↓ {latest ? formatBytes(latest.rx_bytes) : "—"}
            </span>
          </b>
          <Sparkline values={net.map((one) => one.rx_bytes)} height={14} />
          <b>
            <span className="network-up">
              ↑ {latest ? formatBytes(latest.tx_bytes) : "—"}
            </span>
          </b>
          <Sparkline values={net.map((one) => one.tx_bytes)} height={14} />
        </p>
      </div>
      <div className="summary-group">
        <p className="t-caps">Traffic</p>
        <p className="summary-line">
          <span className="k">Proxy</span>{" "}
          <b>{platformLatest ? `${perMinute(platformLatest.proxy_requests)}/min` : "—"}</b>
          <Sparkline values={proxyPerMinute} height={14} />
          <span className="sep">·</span>
          <span className="k">DNS</span>{" "}
          <b>{platformLatest ? `${perMinute(platformLatest.dns_queries)}/min` : "—"}</b>
          <Sparkline values={dnsPerMinute} height={14} />
        </p>
      </div>
    </div>
  );
}

/**
 * One Application's usage in a table cell: the last reading, with the shape
 * of the window under it. A dash until the collector's first tick, which is
 * up to a minute after the Application starts.
 */
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

function Usage({
  samples,
  value,
  render,
}: {
  samples: AppSample[] | null;
  value: (sample: AppSample) => number;
  render: (sample: AppSample) => string;
}) {
  if (!samples) {
    return (
      <TableCell className="muted">—</TableCell>
    );
  }
  return (
    <TableCell className="usage">
      <span className="usage-value">{render(samples[samples.length - 1])}</span>
      <Sparkline values={samples.map(value)} height={18} />
    </TableCell>
  );
}
