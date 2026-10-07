import { useQuery } from "@tanstack/react-query";

import { readJson } from "./api.js";
import type { Environment } from "./types.js";

const noEnvironments: Environment[] = [];

/**
 * The Host's Virtual machines, for the screens that only list them. The detail
 * screen keeps its own copy: it drives actions and needs the log as it grows.
 */
export function useEnvironments(): {
  environments: Environment[];
  reload: () => Promise<void>;
} {
  /*
   * Create and update take minutes and survive a reload, so a row that arrives
   * mid-bootstrap has to keep moving on its own. The slow tick matters just as
   * much: an operation started from a machine's own screen, or from another
   * tab, is not in this copy yet, and a list that stops asking shows a machine
   * as running long after it was deleted.
   */
  const { data = noEnvironments, refetch } = useQuery({
    queryKey: ["environments"],
    queryFn: ({ signal }) => readJson<Environment[]>("/environments", signal),
    refetchInterval: (query) =>
      query.state.data?.some((one) => one.operation?.status === "running") ? 2000 : 5000,
  });

  return { environments: data, reload: async () => { await refetch(); } };
}
