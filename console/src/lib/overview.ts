import type { App, AppSample, Environment, Metrics, PlatformSample } from "./types.js";

export type WorkloadKind = "application" | "database" | "machine";

export type Workload = { id: string; name: string; kind: WorkloadKind };

export type Attention = Workload & { summary: string | null };

export type Ranked = Workload & {
  cpu: number | null;
  memory: number | null;
  samples: AppSample[] | null;
};

export type Tally = { running: number; total: number };

export const isApplication = (app: App) => !app.managed_postgres && app.runtime?.kind !== "external";

export const machineFailed = (machine: Environment) =>
  machine.state === "failed" || machine.operation?.status === "failed";

export function workloadHref(workload: Workload): string {
  return workload.kind === "machine"
    ? `/console/#virtual-machine-${workload.id}`
    : `/console/#app-${workload.id}`;
}

function summaryOf(failure: unknown): string | null {
  if (typeof failure === "string") return failure || null;
  if (failure && typeof failure === "object" && typeof (failure as { error?: unknown }).error === "string") {
    return (failure as { error: string }).error || null;
  }
  return null;
}

export function needsAttention(apps: App[], machines: Environment[]): Attention[] {
  const failedApps = apps
    .filter((app) => app.status === "failed" && (app.managed_postgres || isApplication(app)))
    .map((app): Attention => ({
      id: app.id,
      name: app.name,
      kind: app.managed_postgres ? "database" : "application",
      summary: summaryOf(app.last_error),
    }));
  const failedMachines = machines.filter(machineFailed).map((machine): Attention => ({
    id: machine.id,
    name: machine.config.name,
    kind: "machine",
    summary: summaryOf(machine.operation?.error),
  }));
  return [
    ...failedApps.filter((one) => one.kind === "application"),
    ...failedApps.filter((one) => one.kind === "database"),
    ...failedMachines,
  ];
}

export function failedCounts(apps: App[], machines: Environment[]): Record<WorkloadKind, number> {
  const failed = needsAttention(apps, machines);
  const count = (kind: WorkloadKind) => failed.filter((one) => one.kind === kind).length;
  return { application: count("application"), database: count("database"), machine: count("machine") };
}

export function tallies(apps: App[], machines: Environment[]): Record<WorkloadKind, Tally> & { all: Tally } {
  const applications = apps.filter(isApplication);
  const databases = apps.filter((app) => app.managed_postgres);
  const tally = (running: number, total: number): Tally => ({ running, total });
  const application = tally(applications.filter((app) => app.status === "running").length, applications.length);
  const database = tally(databases.filter((app) => app.status === "running").length, databases.length);
  const machine = tally(machines.filter((one) => one.state === "running").length, machines.length);
  return {
    application,
    database,
    machine,
    all: tally(application.running + database.running + machine.running, application.total + database.total + machine.total),
  };
}

export function topWorkloads(apps: App[], machines: Environment[], metrics: Metrics | null, limit = 6): Ranked[] {
  const rank = (workload: Workload, samples: AppSample[] | undefined): Ranked => {
    const latest = samples?.[samples.length - 1];
    return {
      ...workload,
      cpu: latest ? latest.cpu_percent : null,
      memory: latest ? latest.memory_bytes : null,
      samples: latest && samples ? samples : null,
    };
  };
  const rows = [
    ...apps.filter((app) => app.managed_postgres || isApplication(app)).map((app) => rank(
      { id: app.id, name: app.name, kind: app.managed_postgres ? "database" : "application" },
      metrics?.applications.find((series) => series.id === app.id)?.samples,
    )),
    ...machines.map((machine) => rank(
      { id: machine.id, name: machine.config.name, kind: "machine" },
      metrics?.machines.find((series) => series.id === machine.id)?.samples,
    )),
  ];
  return rows
    .map((row, index) => ({ row, index }))
    .sort((a, b) =>
      (b.row.cpu ?? -1) - (a.row.cpu ?? -1) ||
      (b.row.memory ?? -1) - (a.row.memory ?? -1) ||
      a.index - b.index)
    .slice(0, limit)
    .map(({ row }) => row);
}

export function perMinute(count: number, intervalSeconds: number): number {
  return Math.round((count * 60) / Math.max(1, intervalSeconds));
}

export function errorRate(samples: PlatformSample[]): number | null {
  const requests = samples.reduce((sum, sample) => sum + sample.proxy_requests, 0);
  if (!requests) return null;
  const errors = samples.reduce((sum, sample) => sum + sample.proxy_errors, 0);
  return (errors / requests) * 100;
}

export function countOf(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}
