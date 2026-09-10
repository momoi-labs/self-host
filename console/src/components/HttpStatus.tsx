import { useEffect, useState } from "react";
import { api } from "../lib/api.js";
import { readinessLabel, readinessTone } from "../lib/status.js";
import type { HttpReadiness } from "../lib/types.js";
import { StatusBadge } from "./StatusBadge.js";

/** Shows Web Target HTTP readiness separately from container lifecycle state. */
export function HttpStatus({ id, status }: { id: string; status: string }) {
  const [readiness, setReadiness] = useState<HttpReadiness>("unknown");
  useEffect(() => {
    const controller = new AbortController();
    let running = false;
    const poll = async () => {
      if (running || status !== "running") return;
      running = true;
      try {
        const response = await api(`/apps/id/${encodeURIComponent(id)}/http-status`, {
          signal: controller.signal,
        });
        if (response.ok) setReadiness((await response.json()).readiness as HttpReadiness);
        else setReadiness("unknown");
      } catch {
        if (!controller.signal.aborted) setReadiness("unknown");
      } finally {
        running = false;
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 5000);
    return () => {
      controller.abort();
      window.clearInterval(timer);
    };
  }, [id, status]);
  return <StatusBadge tone={readinessTone(readiness)}>{readinessLabel(readiness)}</StatusBadge>;
}
