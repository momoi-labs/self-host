import type { Report } from "./types.js";

export function apiKey(): string {
  return sessionStorage.getItem("api_key") || "";
}

export function authHeaders(): Record<string, string> {
  return { Authorization: "Bearer " + apiKey(), "Content-Type": "application/json" };
}

export function logout(): void {
  sessionStorage.removeItem("api_key");
  window.location.href = "/console";
}

/** Sends the console back to the login page when there is no key to send. */
export function requireKey(): boolean {
  if (apiKey()) return true;
  window.location.href = "/console";
  return false;
}

/**
 * Every failure reaches the console as { error, caused_by }: the API answers
 * that shape and a stored last_error keeps it. Anything else — a network
 * error, a body that is not JSON — is a failure with nothing underneath.
 */
export function asReport(value: unknown): Report {
  if (value && typeof value === "object" && typeof (value as Report).error === "string") {
    const report = value as Report;
    return { error: report.error, caused_by: report.caused_by || [] };
  }
  return { error: String(value || "Something went wrong."), caused_by: [] };
}

/**
 * Reads a failed response as a report. A body that is not JSON — a proxy
 * error, an empty 502 — still says something, so it becomes the failure.
 */
export async function failureOf(res: Response): Promise<Report> {
  const body = await res.text();
  try {
    return asReport(JSON.parse(body));
  } catch {
    return asReport(body.trim() || res.statusText);
  }
}

export function api(path: string, init: RequestInit = {}): Promise<Response> {
  return fetch(path, { ...init, headers: { ...authHeaders(), ...(init.headers || {}) } });
}

export async function getJson<T>(path: string): Promise<T | null> {
  try {
    const res = await api(path);
    if (!res.ok) return null;
    return (await res.json()) as T;
  } catch {
    return null;
  }
}
