import { useQuery } from "@tanstack/react-query";
import {
  Badge, Button, Card, CardAction, CardContent, CardHeader, CardTitle, Dot,
  EmptyState, EmptyStateActions, EmptyStateDescription, EmptyStateTitle, GridPane, Meter,
  PageHeader, PageHeaderDescription, PageHeaderTitle, PaneGrid, Skeleton, Sparkline,
  Stat, StatFoot, StatHeader, StatLabel, StatValue, StatusBadge, StepBar,
  Table, TableBody, TableCell, TableFrame, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { CreateResource } from "../components/CreateResource.js";
import { Icon } from "../components/Icon.js";
import { plainClick } from "../components/WorkloadCells.js";
import { formatAgo, formatBytes, formatWindow } from "../lib/format.js";
import {
  countOf, errorRate, needsAttention, perMinute, tallies, topWorkloads, workloadHref,
  type Workload, type WorkloadKind,
} from "../lib/overview.js";
import { eventActionLabel, eventSubjectHref, eventUpdatedAt, type PlatformEvent } from "../lib/platformEvents.js";
import { dnsRecordsQuery } from "../lib/queries.js";
import { routesOf } from "../lib/routes.js";
import { TITLES, barSteps, phase } from "../lib/runSteps.js";
import type { App, Environment, Metrics } from "../lib/types.js";
import { useEvents } from "../lib/useEvents.js";
import { hostTotals } from "../lib/useMetrics.js";

const KIND_NAME: Record<WorkloadKind, string> = { application: "Application", database: "Database", machine: "Machine" };
const KIND_SHORT: Record<WorkloadKind, string> = { application: "App", database: "DB", machine: "VM" };
const STAGE_STATE = { running: "running", completed: "done", failed: "failed" } as const;

export function Overview({
  ready, domain, apps, environments, metrics,
  onOpenApp, onOpenEnvironment, onOpenEvents, onOpenSubject, onDeploy, onNewDatabase, onNewMachine,
}: {
  ready: boolean;
  domain: string | null;
  apps: App[];
  environments: Environment[];
  metrics: Metrics | null;
  onOpenApp: (id: string) => void;
  onOpenEnvironment: (id: string) => void;
  onOpenEvents: () => void;
  onOpenSubject: (subject: PlatformEvent["subject"]) => void;
  onDeploy: () => void;
  onNewDatabase: () => void;
  onNewMachine: () => void;
}) {
  const { events } = useEvents();
  const records = useQuery({ ...dnsRecordsQuery, refetchInterval: 15_000 }).data;
  const tally = tallies(apps, environments);
  const empty = tally.all.total === 0;
  const failed = needsAttention(apps, environments);
  const open = (workload: Workload) => workload.kind === "machine" ? onOpenEnvironment(workload.id) : onOpenApp(workload.id);

  const totals = hostTotals(metrics);
  const platform = metrics?.platform ?? [];
  const interval = metrics?.interval_seconds ?? 0;
  const latest = platform[platform.length - 1];
  const window = platform.length > 1 ? `Last ${formatWindow((platform.length - 1) * interval)}` : null;
  const routes = apps.reduce((sum, app) => sum + routesOf(app).length, 0);
  const errors = errorRate(platform);
  const cores = metrics?.host_cpus ?? 0;
  const top = topWorkloads(apps, environments, metrics);
  const span = Math.max(0, ...top.map((row) => (row.samples?.length ?? 1) - 1)) * interval;
  const trend = span > 0 ? `Last ${formatWindow(span)}` : "Trend";

  return (
    <>
      <PageHeader actions={ready ? <CreateResource onDeploy={onDeploy} onCreateMachine={onNewMachine} onCreateDatabase={onNewDatabase} /> : undefined}>
        <PageHeaderTitle>Overview</PageHeaderTitle>
        {ready ? <PageHeaderDescription>
          {empty ? "Nothing runs on this Host yet." : `${domain ? `${domain} · ` : ""}${countOf(tally.application.total, "app")}, ${countOf(tally.database.total, "database")}, ${countOf(tally.machine.total, "machine")}`}
        </PageHeaderDescription> : null}
      </PageHeader>

      {!ready ? (
        <Card><CardContent><Skeleton /></CardContent></Card>
      ) : <>
        {failed.length ? (
          <Card className="needs-attention">
            <CardHeader>
              <div className="between">
                <CardTitle><span className="row"><Icon name="alert" />Needs attention</span></CardTitle>
                <CardAction><Badge variant="danger">{failed.length} failed</Badge></CardAction>
              </div>
            </CardHeader>
            <CardContent>
              <ul className="attention-list">
                {failed.map((one) => (
                  <li key={`${one.kind}-${one.id}`}>
                    <StatusBadge tone="danger">Failed</StatusBadge>
                    <div className="attention-what">
                      <strong>{one.name}</strong> <span className="muted">{KIND_NAME[one.kind]}</span>
                      {one.summary ? <p>{one.summary}</p> : null}
                    </div>
                    <Button size="sm" aria-label={one.kind === "application" ? undefined : `Open ${KIND_NAME[one.kind].toLowerCase()} ${one.name}`} onClick={() => open(one)}>
                      {one.kind === "application" ? `Open ${one.name}` : `Open ${KIND_NAME[one.kind].toLowerCase()}`}
                    </Button>
                  </li>
                ))}
              </ul>
            </CardContent>
          </Card>
        ) : null}

        {empty ? (
          <Card>
            <CardContent>
              <EmptyState variant="first-run">
                <EmptyStateTitle>Deploy the first workload</EmptyStateTitle>
                <EmptyStateDescription>An application, a database or a machine. The Overview fills in when something runs.</EmptyStateDescription>
                <EmptyStateActions>
                  <Button variant="primary" onClick={onDeploy}>Deploy application</Button>
                  <Button onClick={onNewDatabase}>Create database</Button>
                </EmptyStateActions>
              </EmptyState>
            </CardContent>
          </Card>
        ) : null}

        <Card className="layout-stats">
          <Stat>
            <StatHeader><StatLabel>Workloads</StatLabel>{failed.length ? <Dot variant="danger" /> : null}</StatHeader>
            <StatValue>{empty ? "0" : `${tally.all.running}/${tally.all.total}`}</StatValue>
            <StatFoot>{empty ? "No apps, databases or machines"
              : `${tally.application.running}/${tally.application.total} apps · ${tally.database.running}/${tally.database.total} DBs · ${tally.machine.running}/${tally.machine.total} VMs`}</StatFoot>
          </Stat>
          <Stat>
            <StatHeader><StatLabel>Host load</StatLabel></StatHeader>
            <div className="stack-sm">
              <Meter label="CPU" value={totals && cores ? Math.min(100, totals.cpu / cores) : null}
                valueText={totals && cores ? `${(totals.cpu / cores).toFixed(0)}% of ${countOf(cores, "core")}` : "—"} />
              <Meter label="Memory" value={totals?.memoryLimit ? Math.min(100, (totals.memory / totals.memoryLimit) * 100) : null}
                valueText={!totals ? "—" : totals.memoryLimit ? `${formatBytes(totals.memory)} of ${formatBytes(totals.memoryLimit)}` : formatBytes(totals.memory)} />
            </div>
          </Stat>
          <Stat>
            <StatHeader><StatLabel>Proxy traffic</StatLabel></StatHeader>
            <StatValue>{latest ? perMinute(latest.proxy_requests, interval) : "—"} <small className="muted">req/min</small></StatValue>
            {window ? <Sparkline values={platform.map((sample) => sample.proxy_requests)} height={28} label={`Proxy requests. ${window}`} /> : null}
            <StatFoot>{routes ? countOf(routes, "route") : "No routes"}{errors === null ? "" : ` · ${errors.toFixed(1)}% errors`}</StatFoot>
          </Stat>
          <Stat>
            <StatHeader><StatLabel>DNS queries</StatLabel></StatHeader>
            <StatValue>{latest ? perMinute(latest.dns_queries, interval) : "—"} <small className="muted">q/min</small></StatValue>
            {window ? <Sparkline values={platform.map((sample) => sample.dns_queries)} height={28} label={`DNS queries. ${window}`} /> : null}
            <StatFoot>{records ? (records.length ? countOf(records.length, "record") : "No records") : window ?? "No samples yet"}</StatFoot>
          </Stat>
        </Card>

        {empty ? null : (
          <PaneGrid static>
            <GridPane id="top" title="Top workloads by resource use" frame="none">
              <TableFrame>
                <Table density="compact" aria-label="Top workloads by resource use">
                  <TableHeader><TableRow>
                    <TableHead scope="col">Workload</TableHead>
                    <TableHead scope="col">Kind</TableHead>
                    <TableHead scope="col" className="num">CPU</TableHead>
                    <TableHead scope="col" className="num">Memory</TableHead>
                    <TableHead scope="col" className="spark-col">{trend}</TableHead>
                  </TableRow></TableHeader>
                  <TableBody>
                    {top.map((row) => (
                      <TableRow key={`${row.kind}-${row.id}`}>
                        <TableCell><a href={workloadHref(row)} onClick={(event) => {
                          if (!plainClick(event)) return;
                          event.preventDefault();
                          open(row);
                        }}>{row.name}</a></TableCell>
                        <TableCell><Badge variant="neutral">{KIND_SHORT[row.kind]}</Badge></TableCell>
                        <TableCell className="num">{row.cpu === null ? <span className="muted">—</span> : `${row.cpu.toFixed(1)}%`}</TableCell>
                        <TableCell className="num">{row.memory === null ? <span className="muted">—</span> : formatBytes(row.memory)}</TableCell>
                        <TableCell className="spark-col">
                          {row.samples && row.samples.length > 1
                            ? <Sparkline values={row.samples.map((sample) => sample.cpu_percent)} height={20} label={`${row.name} CPU. ${trend}`} />
                            : <span className="muted">—</span>}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
                <div className="table-footer"><span>{top.length} of {countOf(tally.all.total, "workload")}</span></div>
              </TableFrame>
            </GridPane>
            <GridPane id="activity" title="Recent activity" frame="none" actions={<Button size="sm" variant="ghost" asChild>
              <a href="/console/#events" onClick={(event) => {
                if (!plainClick(event)) return;
                event.preventDefault();
                onOpenEvents();
              }}>All events</a>
            </Button>}>
              <Card><CardContent>
                <RecentActivity events={events} machines={environments} onOpenMachine={onOpenEnvironment} onOpenSubject={onOpenSubject} />
              </CardContent></Card>
            </GridPane>
          </PaneGrid>
        )}
      </>}
    </>
  );
}

function RecentActivity({ events, machines, onOpenMachine, onOpenSubject }: {
  events: PlatformEvent[];
  machines: Environment[];
  onOpenMachine: (id: string) => void;
  onOpenSubject: (subject: PlatformEvent["subject"]) => void;
}) {
  const running = machines.filter((one) => one.operation?.status === "running");
  const recent = events
    .filter((event) => !(event.status === "running" && event.subject.kind === "virtual-machine" && running.some((one) => one.id === event.subject.id)))
    .sort((a, b) => Date.parse(eventUpdatedAt(b)) - Date.parse(eventUpdatedAt(a)))
    .slice(0, Math.max(0, 8 - running.length));

  if (!running.length && !recent.length) return <p className="muted">No activity yet.</p>;

  return (
    <ul className="activity-list">
      {running.map((one) => {
        const operation = one.operation!;
        const title = TITLES[operation.action] ?? operation.action;
        return (
          <li key={`machine-${one.id}`}>
            <Dot variant="success" pulse />
            <div>
              <div className="between">
                <span><a className="fg" href={`/console/#virtual-machine-${one.id}`} onClick={(event) => {
                  if (!plainClick(event)) return;
                  event.preventDefault();
                  onOpenMachine(one.id);
                }}><strong>{one.config.name}</strong></a> {title}</span>
                <span className="muted">now</span>
              </div>
              <StepBar label={title} steps={barSteps(operation.action, operation.step)} />
              <span className="muted">{phase(operation.action, operation.step)}</span>
            </div>
          </li>
        );
      })}
      {recent.map((event) => {
        const href = eventSubjectHref(event.subject);
        const at = eventUpdatedAt(event);
        const active = event.status === "running" || event.status === "pending";
        const stages = event.progress?.stages ?? [];
        const detail = event.status === "failed" ? (event.error?.error ?? event.description) : event.description;
        return (
          <li key={event.id}>
            <Dot variant={event.status === "failed" ? "danger" : active ? "success" : "neutral"} pulse={active} />
            <div>
              <div className="between">
                <span>
                  {eventActionLabel(event)}{" "}
                  {href ? <a className="fg" href={href} onClick={(click) => {
                    if (!plainClick(click)) return;
                    click.preventDefault();
                    onOpenSubject(event.subject);
                  }}><strong>{event.subject.name}</strong></a> : <strong>{event.subject.name}</strong>}
                  {event.status === "failed" ? " failed" : ""}
                </span>
                <span className="muted activity-when" title={new Date(at).toLocaleString()}>{active ? "now" : formatAgo(at)}</span>
              </div>
              {active && stages.length ? <StepBar label={eventActionLabel(event)}
                steps={stages.map((stage) => ({ key: stage.id, label: stage.label, state: STAGE_STATE[stage.status] }))} /> : null}
              {detail ? <span className="muted">{detail}</span> : null}
            </div>
          </li>
        );
      })}
    </ul>
  );
}
