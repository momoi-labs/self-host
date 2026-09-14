import { useCallback, useEffect, useState } from "react";

import { getJson } from "./api.js";
import type { Environment } from "./types.js";

/**
 * The Host's Virtual machines, for the screens that only list them. The detail
 * screen keeps its own copy: it drives actions and needs the log as it grows.
 */
export function useEnvironments(): {
  environments: Environment[];
  reload: () => Promise<void>;
} {
  const [environments, setEnvironments] = useState<Environment[]>([]);

  const reload = useCallback(async () => {
    setEnvironments((await getJson<Environment[]>("/environments")) ?? []);
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  /*
   * Create and update take minutes and survive a reload, so a row that arrives
   * mid-bootstrap has to keep moving on its own. The slow tick matters just as
   * much: an operation started from a machine's own screen, or from another
   * tab, is not in this copy yet, and a list that stops asking shows a machine
   * as running long after it was deleted.
   */
  useEffect(() => {
    const running = environments.some(
      (one) => one.operation?.status === "running",
    );
    const timer = window.setTimeout(() => void reload(), running ? 2000 : 5000);
    return () => window.clearTimeout(timer);
  }, [environments, reload]);

  return { environments, reload };
}
