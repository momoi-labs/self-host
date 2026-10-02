import { useEffect, useSyncExternalStore } from "react";
import { api, failureOf } from "./api.js";

export type GitProvider = "github" | "gitlab";
export type GitConnection = {
  id: string;
  name: string;
  provider: GitProvider;
  credential_id: string;
  status: "connected" | "expired";
  authentication?: "token" | "github-app" | "oauth";
};
export type IntegrationSettings = {
  github: { configured: boolean; app_slug: string | null; console_url: string | null };
  gitlab: { configured: boolean; client_id: string | null; console_url: string | null };
};
export type AuthorizationStart = {
  state: string;
  url: string | null;
  form: { action: string; manifest: string } | null;
  expires_in: number;
};
export type AuthorizationCompletion = {
  state: string;
  code?: string;
  installation_id?: number;
  error?: string;
};
export type AuthorizationResult = {
  connection: GitConnection | null;
  integrations: IntegrationSettings | null;
  authorization: AuthorizationStart | null;
};
export type GitRepository = {
  full_name: string;
  clone_url: string;
  default_branch: string | null;
  private: boolean;
};
export type GitRepositoryPage = {
  repositories: GitRepository[];
  next_page: number | null;
};
export type GitConnectionRequest = {
  name: string;
  provider: GitProvider;
  token: string;
};

type Snapshot = { connections: GitConnection[]; loading: boolean; error: string | null };
let snapshot: Snapshot = { connections: [], loading: true, error: null };
let loaded = false;
let pending: Promise<void> | null = null;
const listeners = new Set<() => void>();
type IntegrationSnapshot = { integrations: IntegrationSettings | null; loading: boolean; error: string | null };
let integrationSnapshot: IntegrationSnapshot = { integrations: null, loading: true, error: null };
let integrationsLoaded = false;
let integrationsPending: Promise<void> | null = null;
const integrationListeners = new Set<() => void>();

function setIntegrations(next: IntegrationSnapshot) {
  integrationSnapshot = next;
  for (const listener of integrationListeners) listener();
}

function setSnapshot(next: Snapshot) {
  snapshot = next;
  for (const listener of listeners) listener();
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response;
  try {
    response = await api(path, init);
  } catch {
    throw new Error("Could not reach the Host. Try again.");
  }
  if (!response.ok) throw new Error((await failureOf(response)).error);
  return await response.json() as T;
}

export function refreshGitConnections(): Promise<void> {
  if (pending) return pending;
  setSnapshot({ ...snapshot, loading: true, error: null });
  pending = request<GitConnection[]>("/source/connections").then(
    (connections) => setSnapshot({ connections, loading: false, error: null }),
    (error: Error) => setSnapshot({ ...snapshot, loading: false, error: error.message }),
  ).finally(() => { loaded = true; pending = null; });
  return pending;
}

export function useGitConnections() {
  const state = useSyncExternalStore(
    (listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    () => snapshot,
  );
  useEffect(() => { if (!loaded) void refreshGitConnections(); }, []);
  return { ...state, refresh: refreshGitConnections };
}

export async function saveGitConnection(requestBody: GitConnectionRequest, id?: string): Promise<GitConnection> {
  const result = await request<GitConnection>(
    `/source/connections${id ? `/${encodeURIComponent(id)}` : ""}`,
    { method: id ? "PUT" : "POST", body: JSON.stringify(requestBody) },
  );
  await refreshGitConnections();
  return result;
}

export async function deleteGitConnection(id: string): Promise<void> {
  const response = await api(`/source/connections/${encodeURIComponent(id)}`, { method: "DELETE" });
  if (!response.ok) throw new Error((await failureOf(response)).error);
  await refreshGitConnections();
}

export async function listGitRepositories(id: string, page = 1): Promise<GitRepositoryPage> {
  try {
    return await request<GitRepositoryPage>(`/source/connections/${encodeURIComponent(id)}/repositories?page=${page}`);
  } catch (error) {
    await refreshGitConnections();
    throw error;
  }
}

export function refreshGitIntegrations(): Promise<void> {
  if (integrationsPending) return integrationsPending;
  setIntegrations({ ...integrationSnapshot, loading: true, error: null });
  integrationsPending = request<IntegrationSettings>("/source/integrations").then(
    (integrations) => setIntegrations({ integrations, loading: false, error: null }),
    (error: Error) => setIntegrations({ ...integrationSnapshot, loading: false, error: error.message }),
  ).finally(() => { integrationsLoaded = true; integrationsPending = null; });
  return integrationsPending;
}

export function useGitIntegrations() {
  const state = useSyncExternalStore(
    (listener) => { integrationListeners.add(listener); return () => { integrationListeners.delete(listener); }; },
    () => integrationSnapshot,
  );
  useEffect(() => { if (!integrationsLoaded) void refreshGitIntegrations(); }, []);
  return { ...state, refresh: refreshGitIntegrations };
}

export function startGitAuthorization(provider: GitProvider, name: string, connectionId?: string, useExistingInstallation = false) {
  return request<AuthorizationStart>("/source/authorization/start", {
    method: "POST", body: JSON.stringify({ provider, name, connection_id: connectionId, use_existing_installation: useExistingInstallation }),
  });
}

export function registerGithubIntegration(consoleUrl: string, organization?: string) {
  return request<AuthorizationStart>("/source/integrations/github/register", {
    method: "POST", body: JSON.stringify({ console_url: consoleUrl, organization }),
  });
}

export async function saveGitlabIntegration(body: { console_url: string; client_id: string; client_secret: string }) {
  const integrations = await request<IntegrationSettings>("/source/integrations/gitlab", { method: "PUT", body: JSON.stringify(body) });
  setIntegrations({ integrations, loading: false, error: null });
  integrationsLoaded = true;
  return integrations;
}

export async function completeGitAuthorization(body: AuthorizationCompletion) {
  const result = await request<AuthorizationResult>("/source/authorization/complete", { method: "POST", body: JSON.stringify(body) });
  if (result.integrations) {
    setIntegrations({ integrations: result.integrations, loading: false, error: null });
    integrationsLoaded = true;
  }
  if (result.connection) await refreshGitConnections();
  return result;
}

export async function cancelGitAuthorization(state: string) {
  const response = await api("/source/authorization/cancel", { method: "POST", body: JSON.stringify({ state }) });
  if (!response.ok) throw new Error((await failureOf(response)).error);
}
