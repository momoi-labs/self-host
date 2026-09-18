import { useEffect, useRef } from "react";
import {
  Progress,
  Toast,
  ToastContent,
  ToastDescription,
  ToastTitle,
} from "@momoi-labs/kiso-react";

import { TITLES, position } from "../lib/runSteps.js";
import type { Environment } from "../lib/types.js";
import { useToast } from "./Toasts.js";

/**
 * What the Host is doing to a machine right now, wherever the Operator is
 * looking. A create takes minutes, and the screen it started from is not
 * where the Operator has to wait it out.
 *
 * It holds while the operation runs and closes when it ends, because an
 * outcome is a notification rather than a progress bar: the toast that
 * replaces it says whether it worked.
 */
export function OperationProgress({
  environments,
  watching = null,
}: {
  environments: Environment[];
  /** The machine whose own screen is open, which draws its own progress. */
  watching?: string | null;
}) {
  const notify = useToast();
  const running = environments.find(
    (machine) =>
      machine.operation?.status === "running" && machine.id !== watching,
  );
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
      if (operation.status === "succeeded") {
        notify("success", `${machine.config.name}: ${operation.action} finished`);
      } else if (operation.status === "failed") {
        notify(
          "danger",
          `${machine.config.name}: ${operation.action} failed`,
          operation.error ?? undefined,
        );
      }
    }
    before.current = now;
  }, [environments, notify]);

  if (!running?.operation) return null;
  const { action, step } = running.operation;
  const place = position(action, step);

  return (
    <Toast
      open
      variant="neutral"
      duration={Number.POSITIVE_INFINITY}
      role="status"
    >
      <ToastContent>
        <ToastTitle>
          {running.config.name}: {TITLES[action] ?? action}
        </ToastTitle>
        <ToastDescription>{step ?? "starting"}</ToastDescription>
        <Progress
          className="operation-track"
          label={TITLES[action] ?? action}
          value={place ? place.at : null}
          max={place ? place.of : 100}
          valueText={place ? `${place.at} of ${place.of}` : "Working"}
        />
      </ToastContent>
    </Toast>
  );
}
