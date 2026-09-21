import type { PlatformEvent } from "./platformEvents.js";

/** A task that is done, one way or the other. */
export type TaskOutcome = PlatformEvent & { status: "completed" | "failed" };

/** The task's event once it has finished, or null while it is queued or running. */
export function taskOutcome(events: readonly PlatformEvent[], taskId: string): TaskOutcome | null {
  const event = events.find((candidate) => candidate.id === taskId);
  if (!event || (event.status !== "completed" && event.status !== "failed")) return null;
  return event as TaskOutcome;
}

const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * Follows a task the API accepted with a 202 until it finishes. A task keeps
 * its id from `pending` to the end, so this is the one place a screen waits
 * on work the scheduler carries out. `events` is what reads the history, so
 * a test can hand in its own.
 */
export async function waitForTask(
  taskId: string,
  {
    events,
    delay = wait,
    interval = 1000,
    attempts = 600,
  }: { events: () => Promise<PlatformEvent[]>; delay?: (ms: number) => Promise<void>; interval?: number; attempts?: number },
): Promise<TaskOutcome> {
  for (let attempt = 0; attempt < attempts; attempt++) {
    const outcome = taskOutcome(await events(), taskId);
    if (outcome) return outcome;
    await delay(interval);
  }
  throw new Error("The task is taking longer than expected. Check the Events page.");
}
