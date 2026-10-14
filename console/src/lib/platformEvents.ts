import type { Report } from "./types.js";

/** One task, as the Events page shows it. The id is the task's id. */
export type PlatformEvent = {
  id: string;
  action: "create" | "delete" | "stop" | "start" | "restart" | "configure" | "prune";
  status: "pending" | "running" | "completed" | "failed";
  occurredAt: string;
  startedAt?: string | null;
  finishedAt?: string | null;
  updatedAt?: string | null;
  apiName?: string | null;
  description: string;
  /** Why a failed task failed. */
  error?: Report | null;
  /** What a configure changed, setting by setting. */
  changes?: { setting: string; from: string; to: string }[] | null;
  progress?: { stages: OperationStage[] } | null;
  subject: {
    kind: "application" | "database" | "virtual-machine" | "custom-image" | "api-key" | "dns-record" | "settings" | "platform";
    id: string;
    name: string;
    service?: string;
    available?: boolean;
  };
};

export type OperationStage = {
  id: string;
  label: string;
  status: "running" | "completed" | "failed";
  startedAt: string;
  finishedAt?: string | null;
  output: string[];
  error?: Report | null;
};

export function eventSubjectHref(subject: PlatformEvent["subject"]): string | null {
  if (!subject.id || subject.available === false) return null;
  // A Record has no screen of its own yet (#124).
  if (subject.kind === "dns-record") return null;
  if (subject.kind === "api-key") return "/console/#settings/api-keys";
  if (subject.kind === "settings") return "/console/#settings";
  // Platform work, such as a prune run, is set up under Scheduled tasks.
  if (subject.kind === "platform") return "/console/#settings/scheduled-tasks";
  if (subject.kind === "custom-image") return `/console/#custom-image-${encodeURIComponent(subject.id)}`;
  const prefix = subject.kind === "virtual-machine" ? "virtual-machine" : "app";
  return `/console/#${prefix}-${encodeURIComponent(subject.id)}`;
}

export function filterEvents(events: readonly PlatformEvent[], query: string, status: string): PlatformEvent[] {
  const term = query.trim().toLowerCase();
  return events.filter(event =>
    `${event.subject.name} ${event.subject.service ?? ""}`.toLowerCase().includes(term)
    && (status === "all" || event.status === status),
  ).sort((a, b) => Date.parse(eventUpdatedAt(b)) - Date.parse(eventUpdatedAt(a)));
}

export function eventActionLabel(event: PlatformEvent): string {
  // Work on the Platform itself reads as what it does, not as a verb on a resource.
  if (event.action === "prune") return "Remove unused data";
  const actions: Record<string, string> = { create: "Create", delete: "Delete", stop: "Stop", start: "Start", restart: "Restart", configure: "Configure" };
  const resources: Record<string, string> = { application: "application", database: "database", "virtual-machine": "virtual machine", "custom-image": "custom image", "api-key": "API key", "dns-record": "DNS record", settings: "settings", platform: "Platform" };
  // An action or kind this console does not know yet still reads as words, never as "undefined".
  const action = actions[event.action] ?? event.action.charAt(0).toUpperCase() + event.action.slice(1);
  return `${action} ${resources[event.subject.kind] ?? event.subject.kind}`;
}

/** What kind of thing an event is about, as the Events table names it. */
export function eventSubjectKindLabel(subject: PlatformEvent["subject"]): string {
  const kinds: Record<string, string> = { database: "Database", "virtual-machine": "Virtual machine", "custom-image": "Custom image", "api-key": "API key", "dns-record": "DNS record", settings: "Settings", platform: "Platform" };
  return kinds[subject.kind] ?? (subject.service ? subject.name : "Application");
}

export function eventUpdatedAt(event: PlatformEvent): string {
  return event.updatedAt ?? event.finishedAt ?? event.startedAt ?? event.occurredAt;
}

/** How the Events page names a task's status: what it is doing, not the scheduler's word. */
export function eventStatusLabel(status: PlatformEvent["status"]): string {
  return { pending: "Waiting", running: "Running", completed: "Done", failed: "Failed" }[status];
}

/**
 * The page numbers a pager shows: the first, the last, the current one and
 * its neighbours, with one gap wherever pages are skipped.
 */
export function pageItems(page: number, pages: number): (number | "gap")[] {
  const items: (number | "gap")[] = [];
  for (let n = 1; n <= pages; n++) {
    if (n === 1 || n === pages || Math.abs(n - page) <= 1) items.push(n);
    else if (items[items.length - 1] !== "gap") items.push("gap");
  }
  return items;
}
