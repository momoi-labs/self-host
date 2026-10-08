import { useCallback, useEffect, useRef } from "react";
import { useQuery } from "@tanstack/react-query";

import { useToast } from "../components/Toasts.js";
import { readJson } from "./api.js";
import { appsQuery, bootstrapStatusQuery } from "./queries.js";
import type { App, FailingWorkload, Health } from "./types.js";

export type Platform = {
  apps: App[];
  dnsSuffix: string;
  healthy: boolean;
  /** Workloads that should be up and are not; empty while the Host is healthy. */
  failing: FailingWorkload[];
  degraded: boolean;
  version: string | null;
  ready: boolean;
  reload: () => Promise<App[]>;
};

const noApps: App[] = [];
const noFailing: FailingWorkload[] = [];

/**
 * Everything the console knows about the Host: its Applications and the DNS
 * suffix they are published under.
 */
export function usePlatform(): Platform {
  const notify = useToast();
  const status = useQuery(bootstrapStatusQuery);
  // Asked again on a timer: a workload can fail long after the page loaded,
  // and the header is where the Operator sees it first.
  const health = useQuery({
    queryKey: ["health"],
    queryFn: ({ signal }) => readJson<Health>("/health", signal),
    refetchInterval: 15_000,
  });
  /*
   * A deploy is accepted before Docker starts pulling, so the row arrives as
   * pending and settles minutes later. Keep asking while anything is in flight.
   */
  const apps = useQuery({
    ...appsQuery,
    refetchInterval: (query) => (query.state.data?.some((app) => app.status === "pending") ? 2000 : false),
  });
  const known = useRef<Record<string, string>>({});

  /*
   * The dialog is long gone by the time a pull finishes, so the outcome has to
   * find the operator wherever they are.
   */
  useEffect(() => {
    const next = apps.data;
    if (!next) return;
    for (const app of next) {
      if (known.current[app.id] !== "pending" || app.status === "pending") continue;
      if (app.status === "running") {
        notify("success", app.managed_postgres ? "Database deployed" : "Application deployed", {
          error: app.hostname ? `${app.name} is running at ${app.hostname}.` : `${app.name} is running.`,
          caused_by: [],
        });
      } else if (app.status === "stopped") {
        notify("success", "Changes applied", {
          error: `${app.name} remains stopped.`,
          caused_by: [],
        });
      } else {
        notify("danger", `Could not deploy ${app.name}`, app.last_error || "The deploy failed.");
      }
    }
    known.current = Object.fromEntries(next.map((app) => [app.id, app.status]));
  }, [apps.data, notify]);

  const { refetch } = apps;
  const reload = useCallback(async () => (await refetch()).data ?? noApps, [refetch]);

  return {
    apps: apps.data ?? noApps,
    dnsSuffix: status.data?.dns_suffix || "…",
    healthy: health.isSuccess,
    failing: health.data?.failing ?? noFailing,
    degraded: health.data?.status === "degraded",
    version: health.data?.version ?? null,
    ready: apps.isFetched,
    reload,
  };
}
