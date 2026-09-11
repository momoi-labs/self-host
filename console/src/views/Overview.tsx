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
import { Meters } from "../components/Meters.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Traffic } from "../components/Traffic.js";
import { formatBytes } from "../lib/format.js";
import { statusTone } from "../lib/status.js";
import { hostNetworkSeries, hostTotals, seriesFor } from "../lib/useMetrics.js";
import type { App, AppSample, Metrics } from "../lib/types.js";

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

export function Overview({
  apps,
  dnsSuffix,
  metrics,
  query,
  onQuery,
  onOpenApp,
  onDeploy,
}: {
  apps: App[];
  dnsSuffix: string;
  metrics: Metrics | null;
  query: string;
  onQuery: (query: string) => void;
  onOpenApp: (id: string) => void;
  onDeploy: () => void;
}) {
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

  const needle = query.trim().toLowerCase();
  const visible = rows.filter((row) => row.name.toLowerCase().includes(needle));
  const empty = rows.length === 0 && !needle;
  const running = apps.filter((app) => app.status === "running").length;
  const totals = hostTotals(metrics);

  const open = (row: Row) => onOpenApp(row.id);

  return (
    <>
      <PageHeader>
        <PageHeaderTitle>Overview</PageHeaderTitle>
        <PageHeaderDescription>
          {apps.length === 0
            ? `Nothing deployed on ${dnsSuffix} yet.`
            : `${apps.length} ${apps.length === 1 ? "application on" : "applications on"} ${dnsSuffix}`}
        </PageHeaderDescription>
      </PageHeader>

      <div className="dashboard-filters">
        <Search
          aria-label="Search by name"
          placeholder="Search by name…"
          value={query}
          onChange={(event) => onQuery(event.target.value)}
        />
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            onQuery("");
          }}
        >
          Clear filters
        </Button>
      </div>

      {empty ? (
        <Card>
          <EmptyState variant="first-run" className="hatch">
            <EmptyStateIcon>
              <Icon name="box" size="lg" />
            </EmptyStateIcon>
            <EmptyStateTitle>No applications yet</EmptyStateTitle>
            <EmptyStateDescription>
              Deploy your first application from a container image or a Compose file and it will be
              reachable on your LAN.
            </EmptyStateDescription>
            <EmptyStateActions>
              <Button size="sm" variant="primary" onClick={onDeploy}>
                <Icon name="plus" />
                Deploy application
              </Button>
            </EmptyStateActions>
          </EmptyState>
        </Card>
      ) : (
        <div className="stack">
          {/* How many are up, and the load that produces, side by side. */}
          <div className="summary-row">
            <Card>
              <Stat>
                <StatLabel>Applications running</StatLabel>
                <StatValue>
                  {running}
                  <span className="of-total">/{apps.length}</span>
                </StatValue>
              </Stat>
            </Card>
            <Card className="host-load">
              <div className="card-body">
                <p className="t-caps">Host load</p>
                {totals ? (
                  <Meters totals={totals} network={hostNetworkSeries(metrics)} />
                ) : (
                  <p className="muted">Collecting.</p>
                )}
              </div>
            </Card>
          </div>

          <Card>
            <div className="card-body">
              <Traffic
                platform={metrics?.platform ?? []}
                intervalSeconds={metrics?.interval_seconds ?? 10}
              />
            </div>
          </Card>

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
            <p className="table-footer">
              {visible.length} of {rows.length} applications
            </p>
          </div>
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
