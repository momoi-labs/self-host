import { useEffect, useRef } from "react";
import {
  Progress,
  Toast,
  ToastContent,
  ToastDescription,
  ToastTitle,
} from "@momoi-labs/kiso-react";

import type { Environment, Operation } from "../lib/types.js";
import { useToast } from "./Toasts.js";

/**
 * The steps each action walks, in order. A create runs the whole provisioning
 * script, so it is the long one; the rest are a stage or two.
 *
 * This list lives here because the console is what needs to draw a fraction,
 * and the Platform reports a step name rather than a position. If a step
 * arrives that is not here, the bar goes quiet rather than lying: the label
 * still says where the machine is.
 */
const SEQUENCES: Record<string, string[]> = {
  create: [
    "creating",
    "booting",
    "system-packages",
    "user-and-ssh",
    "mise",
    "tools",
    "setup",
    "checks",
    "service",
    "health",
    "ready",
  ],
  bootstrap: [
    "starting",
    "provisioning",
    "system-packages",
    "user-and-ssh",
    "mise",
    "tools",
    "setup",
    "checks",
    "service",
    "health",
    "ready",
  ],
  start: ["starting", "ready"],
  stop: ["stopping", "ready"],
  restart: ["restarting", "ready"],
  delete: ["deleting"],
};
SEQUENCES.update = SEQUENCES.bootstrap;

/**
 * What each action is doing, said as the work rather than as the verb the API
 * takes. "create" is the name of a request; "Creating new machine" is what the
 * Operator is waiting for.
 */
const TITLES: Record<string, string> = {
  create: "Creating new machine",
  bootstrap: "Bootstrapping the machine",
  update: "Updating the machine",
  start: "Starting the machine",
  stop: "Stopping the machine",
  restart: "Restarting the machine",
  delete: "Deleting the machine",
};

function position(action: string, step: string | null | undefined) {
  const sequence = SEQUENCES[action];
  if (!sequence || !step) return null;
  const index = sequence.indexOf(step);
  return index < 0 ? null : { at: index + 1, of: sequence.length };
}

/**
 * Where the machine is and how much of the wait is left. It names one step, the
 * one it is on, and lets that label change: the steps ahead are names the
 * Operator cannot act on, and listing all ten made the header taller than the
 * machine it describes.
 *
 * It takes the status badges' place while it runs, so it names the action the
 * badge named.
 */
export function OperationSteps({
  operation,
}: {
  operation: NonNullable<Operation>;
}) {
  const place = position(operation.action, operation.step);

  return (
    <div className="operation-steps">
      <Progress
        label={TITLES[operation.action] ?? operation.action}
        value={place ? place.at : null}
        max={place ? place.of : 100}
        valueText={place ? `${place.at} of ${place.of}` : "Working"}
      />
      <p className="operation-step">{operation.step ?? "starting"}</p>
    </div>
  );
}

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
