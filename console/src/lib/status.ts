import type { App, HttpReadiness } from "./types.js";

export type Tone = "success" | "danger" | "neutral";

/**
 * running is the only good outcome; pending and failed both need attention,
 * but only failed is an error.
 */
export function statusTone(status: string): Tone {
  if (status === "running") return "success";
  if (status === "failed") return "danger";
  return "neutral";
}

/** Docker's word for a container, in the Application's vocabulary. */
export function serviceTone(state: string): Tone {
  if (state === "running") return "success";
  if (state === "exited" || state === "dead" || state === "restarting" || state === "missing") {
    return "danger";
  }
  return "neutral";
}

export function readinessTone(readiness: HttpReadiness | undefined): Tone {
  if (readiness === "responding") return "success";
  if (readiness === "unreachable" || readiness === "error") return "danger";
  return "neutral";
}

export function readinessLabel(readiness: HttpReadiness | undefined): string {
  if (readiness === "responding") return "HTTP responding";
  if (readiness === "error") return "HTTP error";
  if (readiness === "unreachable") return "HTTP not responding";
  return "HTTP unknown";
}

export function isCompose(app: App): boolean {
  return app.source === "compose";
}

/** Every Hostname the application answers on: its Hostname, then its aliases. */
export function hostnames(app: App): string[] {
  return [app.hostname, ...(app.aliases || [])];
}

/**
 * A comma or a space both read as "and another one" to someone typing a list,
 * so accept either rather than rejecting the one that was not asked for.
 */
export function parseAliases(value: string): string[] {
  return value
    .split(/[,\s]+/)
    .map((alias) => alias.trim())
    .filter(Boolean);
}
