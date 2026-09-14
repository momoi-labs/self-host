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
  Stat,
  StatLabel,
  StatValue,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { NetworkChart } from "../components/NetworkChart.js";
import { Meters } from "../components/Meters.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Traffic } from "../components/Traffic.js";
import { formatBytes } from "../lib/format.js";
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
  const running = apps.filter((app) => app.status === "running").length;
  const awake = environments.filter((one) => one.state === "running").length;
  const totals = hostTotals(metrics);

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
          {/* What is up, what it costs the Host, and what the Platform's own
              servers did, on one line. */}
          <div className="summary-row">
            <Card className="summary-1">
              <Stat>
                <StatLabel>Apps running</StatLabel>
                <StatValue>
                  {running}
                  <span className="of-total">/{apps.length}</span>
                </StatValue>
              </Stat>
            </Card>
            <Card className="summary-1">
              <Stat>
                <StatLabel>VMs running</StatLabel>
                <StatValue>
                  {awake}
                  <span className="of-total">/{environments.length}</span>
                </StatValue>
              </Stat>
            </Card>
            <Card className="summary-4 host-load">
              <div className="card-body">
                <p className="t-caps">Host load</p>
                {totals ? (
                  <Meters totals={totals} />
                ) : (
                  <p className="muted">Collecting.</p>
                )}
              </div>
            </Card>
            <Card className="summary-3">
              <div className="card-body">
                <NetworkChart samples={hostNetworkSeries(metrics)} />
              </div>
            </Card>
            <Card className="summary-3">
              <div className="card-body">
                <Traffic
                  platform={metrics?.platform ?? []}
                  intervalSeconds={metrics?.interval_seconds ?? 10}
                />
              </div>
            </Card>
          </div>

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
                            <StatusBadge
                              tone={one.service_ready ? "success" : "neutral"}
                            >
                              {one.service_ready ? "Ready" : "Disabled"}
                            </StatusBadge>
                          </TableCell>
                          {/* What it is doing right now is the only reason to
                              look at a machine mid-bootstrap. An operation that
                              ended is history: it said "create: ready" at a
                              machine being deleted. */}
                          <TableCell>
                            {one.operation?.status === "running"
                              ? `${one.operation.action}: ${one.operation.step ?? "starting"}`
                              : one.operation?.status === "failed"
                                ? `${one.operation.action} failed`
                                : "Idle"}
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
 * One Application's usage in a table cell: the last reading, with the shape
 * of the window under it. A dash until the collector's first tick, which is
 * up to a minute after the Application starts.
 */
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
