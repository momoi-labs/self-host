import { useCallback, useEffect, useRef, useState } from "react";

import { useToast } from "../components/Toasts.js";
import { getJson } from "./api.js";
import type { App } from "./types.js";

export type Platform = {
  apps: App[];
  dnsSuffix: string;
  healthy: boolean;
  ready: boolean;
  reload: () => Promise<App[]>;
};

/**
 * Everything the console knows about the Host: its Applications and the DNS
 * suffix they are published under.
 */
export function usePlatform(): Platform {
  const notify = useToast();
  const [apps, setApps] = useState<App[]>([]);
  const [dnsSuffix, setDnsSuffix] = useState("…");
  const [healthy, setHealthy] = useState(false);
  const [ready, setReady] = useState(false);
  const known = useRef<Record<string, string>>({});

  /*
   * The dialog is long gone by the time a pull finishes, so the outcome has to
   * find the operator wherever they are.
   */
  const announceSettled = useCallback(
    (next: App[]) => {
      for (const app of next) {
        if (known.current[app.id] !== "pending" || app.status === "pending") continue;
        if (app.status === "running") {
          notify("success", "Application deployed", {
            error: `${app.name} is running at ${app.hostname}.`,
            caused_by: [],
          });
        } else {
          notify("danger", `Could not deploy ${app.name}`, app.last_error || "The deploy failed.");
        }
      }
      known.current = Object.fromEntries(next.map((app) => [app.id, app.status]));
    },
    [notify],
  );

  const reload = useCallback(async () => {
    const next = (await getJson<App[]>("/apps")) ?? [];
    announceSettled(next);
    setApps(next);
    return next;
  }, [announceSettled]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const status = await getJson<{ dns_suffix?: string }>("/bootstrap/status");
      if (cancelled) return;
      if (status?.dns_suffix) setDnsSuffix(status.dns_suffix);
      setHealthy((await getJson<unknown>("/health")) !== null);
      await reload();
      if (!cancelled) setReady(true);
    })();
    return () => {
      cancelled = true;
    };
  }, [reload]);

  /*
   * A deploy is accepted before Docker starts pulling, so the row arrives as
   * pending and settles minutes later. Keep asking while anything is in flight.
   */
  useEffect(() => {
    if (!apps.some((app) => app.status === "pending")) return;
    const timer = window.setTimeout(() => void reload(), 2000);
    return () => window.clearTimeout(timer);
  }, [apps, reload]);

  return { apps, dnsSuffix, healthy, ready, reload };
}
