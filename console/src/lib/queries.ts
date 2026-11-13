import { queryOptions } from "@tanstack/react-query";

import { api, failureOf, readJson } from "./api.js";
import type { App, BackupSettings, BackupStorage, BackupSummary, BootstrapStatus, ConsumerDatabase, DatabaseBackup, CustomImage, DatabaseDetail, DatabaseImport, HostStatus, MaintenanceWindow, Settings } from "./types.js";

/*
 * The reads more than one screen shares. A screen that polls one of these
 * adds its own refetchInterval; the cache, and every screen showing it,
 * follows the newest answer.
 */

export const appsQuery = queryOptions({
  queryKey: ["apps"],
  queryFn: ({ signal }) => readJson<App[]>("/apps", signal),
});

export const bootstrapStatusQuery = queryOptions({
  queryKey: ["bootstrap", "status"],
  queryFn: ({ signal }) => readJson<BootstrapStatus>("/bootstrap/status", signal),
});

export const settingsQuery = queryOptions({
  queryKey: ["settings"],
  queryFn: ({ signal }) => readJson<Settings>("/settings", signal),
});

export const hostQuery = queryOptions({
  queryKey: ["host"],
  queryFn: ({ signal }) => readJson<HostStatus>("/host", signal),
});

/** One managed database and its connections. */
export const databaseQuery = (id: string) => queryOptions({
  queryKey: ["databases", id],
  queryFn: ({ signal }) => readJson<DatabaseDetail>(`/databases/${id}`, signal),
});

/** The consumer databases a server holds that the Platform made, read live from PostgreSQL. */
export const consumerDatabasesQuery = (id: string) => queryOptions({
  queryKey: ["databases", id, "databases"],
  queryFn: ({ signal }) => readJson<ConsumerDatabase[]>(`/databases/${id}/databases`, signal),
});

export const databaseImportsQuery = (id: string) => queryOptions({
  queryKey: ["databases", id, "imports"],
  queryFn: ({ signal }) => readJson<DatabaseImport[]>(`/databases/${id}/imports`, signal),
});

export const databaseBackupsQuery = (id: string) => queryOptions({
  queryKey: ["databases", id, "backups"],
  queryFn: ({ signal }) => readJson<DatabaseBackup[]>(`/databases/${id}/backups`, signal),
});

export const backupSettingsQuery = (id: string) => queryOptions({
  queryKey: ["databases", id, "backup-settings"],
  queryFn: ({ signal }) => readJson<BackupSettings>(`/databases/${id}/backups/settings`, signal),
});

/** The last backup of every database that has one, for the list of databases. */
export const backupSummaryQuery = queryOptions({
  queryKey: ["backups"],
  queryFn: ({ signal }) => readJson<BackupSummary[]>("/backups", signal),
});

export const backupStorageQuery = queryOptions({
  queryKey: ["backup-storage"],
  queryFn: ({ signal }) => readJson<BackupStorage>("/backups/storage", signal),
});

export const maintenanceWindowQuery = queryOptions({
  queryKey: ["maintenance-window"],
  queryFn: ({ signal }) => readJson<MaintenanceWindow>("/maintenance-window", signal),
});

export const customImagesQuery = queryOptions({
  queryKey: ["custom-images"],
  queryFn: ({ signal }) => readJson<CustomImage[]>("/custom-images", signal),
});

export type MiseTool = { name: string; backends: string[] };

/**
 * The mise registry, read once per page: it does not change while the
 * console is open, and a failed read is asked again by the next screen that
 * needs it.
 */
export const toolCatalogQuery = queryOptions({
  queryKey: ["tool-catalog"],
  queryFn: async (): Promise<MiseTool[]> => {
    const response = await api("/custom-images/tools");
    if (!response.ok) throw await failureOf(response);
    const payload: unknown = await response.json();
    if (!Array.isArray(payload)) throw new Error("Invalid tool search response");
    return payload.flatMap((entry: unknown): MiseTool[] => {
      if (typeof entry === "string") return [{ name: entry, backends: [] }];
      if (!entry || typeof entry !== "object" || !("name" in entry) || typeof entry.name !== "string") return [];
      return [{
        name: entry.name,
        backends: "backends" in entry && Array.isArray(entry.backends)
          ? entry.backends.filter((key): key is string => typeof key === "string") : [],
      }];
    });
  },
  staleTime: Infinity,
  gcTime: Infinity,
});
