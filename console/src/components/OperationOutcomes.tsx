import { useEffect, useRef } from "react";

import { NOUNS } from "../lib/runSteps.js";
import type { Environment } from "../lib/types.js";
import { useToast } from "./Toasts.js";

/**
 * How a machine's operation ended, said once, wherever the Operator is
 * looking. A create takes minutes and the screen it started from is not where
 * they have to wait it out: the Overview's table and the machine's own screen
 * show the run going, and this says when it is over.
 */
export function OperationOutcomes({ environments }: { environments: Environment[] }) {
  const notify = useToast();
  // What each machine's operation was on the previous render, so an outcome is
  // announced once, by the change rather than by the state.
  const before = useRef<Record<string, string>>({});

  useEffect(() => {
    const now: Record<string, string> = {};
    for (const machine of environments) {
      const operation = machine.operation;
      if (!operation) continue;
      now[machine.id] = `${operation.action}:${operation.status}`;
      const was = before.current[machine.id];
      if (!was || was === now[machine.id]) continue;
      if (!was.endsWith(":running")) continue;
      const noun = NOUNS[operation.action] ?? operation.action;
      if (operation.status === "succeeded") {
        notify("success", `${machine.config.name}: ${noun} finished`);
      } else if (operation.status === "failed") {
        notify(
          "danger",
          `${machine.config.name}: ${noun} failed`,
          operation.error ?? undefined,
        );
      }
    }
    before.current = now;
  }, [environments, notify]);

  return null;
}
