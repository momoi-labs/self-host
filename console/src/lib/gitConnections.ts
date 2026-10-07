import { queryOptions, useQuery } from "@tanstack/react-query";
import { api, failureOf } from "./api.js";
import { queryClient } from "./queryClient.js";

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

/*
 * Read once per page, like the saved credentials they describe, and again
 * after anything here changes them.
 */
const connectionsQuery = queryOptions({
  queryKey: ["source", "connections"],
  queryFn: ({ signal }) => request<GitConnection[]>("/source/connections", { signal }),
  staleTime: Infinity,
});

const integrationsQuery = queryOptions({
  queryKey: ["source", "integrations"],
  queryFn: ({ signal }) => request<IntegrationSettings>("/source/integrations", { signal }),
  staleTime: Infinity,
});

const noConnections: GitConnection[] = [];

export function refreshGitConnections(): Promise<void> {
  return queryClient.invalidateQueries({ queryKey: connectionsQuery.queryKey });
}

export function useGitConnections() {
  const { data, isLoading, error } = useQuery(connectionsQuery);
  return { connections: data ?? noConnections, loading: isLoading, error: error?.message ?? null, refresh: refreshGitConnections };
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
  return queryClient.invalidateQueries({ queryKey: integrationsQuery.queryKey });
}

export function useGitIntegrations() {
  const { data, isLoading, error } = useQuery(integrationsQuery);
  return { integrations: data ?? null, loading: isLoading, error: error?.message ?? null, refresh: refreshGitIntegrations };
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
  queryClient.setQueryData(integrationsQuery.queryKey, integrations);
  return integrations;
}

export async function completeGitAuthorization(body: AuthorizationCompletion) {
  const result = await request<AuthorizationResult>("/source/authorization/complete", { method: "POST", body: JSON.stringify(body) });
  if (result.integrations) queryClient.setQueryData(integrationsQuery.queryKey, result.integrations);
  if (result.connection) await refreshGitConnections();
  return result;
}

export async function cancelGitAuthorization(state: string) {
  const response = await api("/source/authorization/cancel", { method: "POST", body: JSON.stringify({ state }) });
  if (!response.ok) throw new Error((await failureOf(response)).error);
}
