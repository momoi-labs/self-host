import { StatusBadge } from "@momoi-labs/kiso-react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../lib/api.js";
import { readinessLabel, readinessTone } from "../lib/status.js";
import type { HttpReadiness } from "../lib/types.js";

/** Shows Web Target HTTP readiness separately from container lifecycle state. */
export function HttpStatus({ id, status }: { id: string; status: string }) {
  const { data: readiness = "unknown" } = useQuery({
    queryKey: ["apps", id, "http-status"],
    queryFn: async ({ signal }): Promise<HttpReadiness> => {
      try {
        const response = await api(`/apps/id/${encodeURIComponent(id)}/http-status`, { signal });
        return response.ok ? ((await response.json()).readiness as HttpReadiness) : "unknown";
      } catch {
        return "unknown";
      }
    },
    enabled: status === "running",
    refetchInterval: 5000,
  });
  return <StatusBadge tone={readinessTone(readiness)}>{readinessLabel(readiness)}</StatusBadge>;
}
