import type { Report } from "./types.js";

/** One task, as the Events page shows it. The id is the task's id. */
export type PlatformEvent = {
  id: string;
  action: "create" | "delete" | "stop" | "start" | "restart" | "configure";
  status: "pending" | "running" | "completed" | "failed";
  occurredAt: string;
  startedAt?: string | null;
  finishedAt?: string | null;
  updatedAt?: string | null;
  apiName?: string | null;
  description: string;
  /** Why a failed task failed. */
  error?: Report | null;
  subject: {
    kind: "application" | "virtual-machine" | "custom-image" | "api-key" | "dns-record";
    id: string;
    name: string;
    service?: string;
    available?: boolean;
  };
};

export function eventSubjectHref(subject: PlatformEvent["subject"]): string | null {
  if (!subject.id || subject.available === false) return null;
  // A Record has no screen of its own yet (#124).
  if (subject.kind === "dns-record") return null;
  if (subject.kind === "api-key") return "/console/api-keys.html";
  if (subject.kind === "custom-image") return `/console/#custom-image-${encodeURIComponent(subject.id)}`;
  const prefix = subject.kind === "virtual-machine" ? "environment" : "app";
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
  const actions = { create: "Create", delete: "Delete", stop: "Stop", start: "Start", restart: "Restart", configure: "Configure" };
  const resources = { application: "application", "virtual-machine": "virtual machine", "custom-image": "custom image", "api-key": "API key", "dns-record": "DNS record" };
  return `${actions[event.action]} ${resources[event.subject.kind]}`;
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
