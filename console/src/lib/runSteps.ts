/**
 * A run is one lifecycle action walked step by step: the stages the Host does
 * on its side (create, boot, start) and the steps the guest's recipe reports
 * as `SF_STEP <step>` lines. The Host's event log keeps every line the run
 * printed, each with its time and action, between `--- <action> started ---`
 * and `--- <action> succeeded|failed ---`. This module reads the last run
 * back out of that log so a screen can draw it.
 */

/**
 * The steps each action walks, in order. A create runs the whole provisioning
 * script, so it is the long one; the rest are a stage or two.
 *
 * This list lives here because the console is what needs to draw a fraction,
 * and the Platform reports a step name rather than a position. If a step
 * arrives that is not here, the bar goes quiet rather than lying: the label
 * still says where the machine is.
 */
export const SEQUENCES: Record<string, string[]> = {
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
export const TITLES: Record<string, string> = {
  create: "Creating new machine",
  bootstrap: "Bootstrapping the machine",
  update: "Updating the machine",
  start: "Starting the machine",
  stop: "Stopping the machine",
  restart: "Restarting the machine",
  delete: "Deleting the machine",
};

/** The same actions as a noun, for a run that is over: "Update failed". */
export const NOUNS: Record<string, string> = {
  create: "Create",
  bootstrap: "Bootstrap",
  update: "Update",
  start: "Start",
  stop: "Stop",
  restart: "Restart",
  delete: "Delete",
};

/** What each step does, in the Operator's words rather than the script's. */
export const STEP_LABELS: Record<string, string> = {
  creating: "Create the instance",
  booting: "Boot the machine",
  starting: "Start the machine",
  stopping: "Stop the machine",
  restarting: "Restart the machine",
  deleting: "Delete the machine",
  provisioning: "Run the recipe",
  "system-packages": "System packages",
  "user-and-ssh": "User and SSH",
  mise: "Install mise",
  tools: "Mise packages",
  setup: "Custom commands",
  checks: "Build checks",
  service: "Start the service",
  health: "Health check",
  ready: "Ready",
};

export function stepLabel(step: string): string {
  return STEP_LABELS[step] ?? step;
}

export function position(action: string, step: string | null | undefined) {
  const sequence = SEQUENCES[action];
  if (!sequence || !step) return null;
  const index = sequence.indexOf(step);
  return index < 0 ? null : { at: index + 1, of: sequence.length };
}

export type RunStep = {
  name: string;
  startedAt: string;
  endedAt: string | null;
  /** What the run printed while on this step. */
  output: string[];
};

export type Run = {
  action: string;
  status: "running" | "succeeded" | "failed";
  startedAt: string;
  endedAt: string | null;
  steps: RunStep[];
  /** The step the recipe named when it failed, when it named one. */
  failedStep: string | null;
  /** The closing line's reason, when the run failed. */
  message: string | null;
};

const LINE = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z) (\S+) ?(.*)$/;
const STARTED = /^--- (\S+) started ---$/;
const SUCCEEDED = /^--- \S+ succeeded ---$/;
const FAILED = /^--- \S+ failed: (.*) ---$/;

/**
 * The last run in a machine's event log, or null when none has started.
 * Everything before the last `started` line belongs to an earlier run and is
 * skipped; a run with no closing line is still going.
 */
export function parseRun(events: string): Run | null {
  let run: Run | null = null;
  for (const raw of events.split("\n")) {
    const match = LINE.exec(raw);
    if (!match) continue;
    const [, at, , text] = match;
    const started = STARTED.exec(text);
    if (started) {
      run = {
        action: started[1],
        status: "running",
        startedAt: at,
        endedAt: null,
        steps: [],
        failedStep: null,
        message: null,
      };
      continue;
    }
    if (!run) continue;
    if (SUCCEEDED.test(text)) {
      close(run, at);
      run.status = "succeeded";
      run.endedAt = at;
      continue;
    }
    const failed = FAILED.exec(text);
    if (failed) {
      close(run, at);
      run.status = "failed";
      run.endedAt = at;
      run.message = failed[1];
      continue;
    }
    const marker = text.startsWith("SF_STEP ") ? text.slice(8) : null;
    if (marker?.startsWith("failed:")) {
      close(run, at);
      run.failedStep = marker.slice(7);
      continue;
    }
    // The Host's own stages arrive as bare words; only the ones the action is
    // known to walk count, so a guest line that happens to be one word does
    // not open a step.
    const step =
      marker ?? (SEQUENCES[run.action]?.includes(text) ? text : null);
    if (step) {
      close(run, at);
      run.steps.push({ name: step, startedAt: at, endedAt: null, output: [] });
      continue;
    }
    run.steps.at(-1)?.output.push(text);
  }
  return run;
}

function close(run: Run, at: string) {
  const current = run.steps.at(-1);
  if (current && !current.endedAt) current.endedAt = at;
}

export type StepState = "done" | "skipped" | "running" | "failed" | "pending";

export type StepView = {
  name: string;
  state: StepState;
  /** Seconds the step took, once it has ended. */
  seconds: number | null;
  output: string[];
};

/**
 * The run laid over the steps its action is expected to walk, so the ones
 * still ahead are drawn too. A step the sequence does not know is kept where
 * the run put it rather than dropped.
 */
export function stepViews(run: Run): StepView[] {
  const seen = new Map(run.steps.map((step) => [step.name, step]));
  const names = [...(SEQUENCES[run.action] ?? [])];
  for (const step of run.steps)
    if (!names.includes(step.name)) names.push(step.name);
  const last = run.steps.at(-1)?.name ?? null;
  const failedAt = run.failedStep ?? (run.status === "failed" ? last : null);
  const reached = last ? names.indexOf(last) : -1;
  return names.map((name, index) => {
    const step = seen.get(name);
    // A stage the run walked past without printing was not needed: an update
    // of a machine that is already up never starts it. A run that succeeded
    // reached its end, so the same goes for what comes after its last line;
    // the Host's start reports no "ready" of its own.
    const state: StepState = !step
      ? index < reached
        ? "skipped"
        : run.status === "succeeded"
          ? "done"
          : "pending"
      : name === failedAt
        ? "failed"
        : step.endedAt
          ? "done"
          : run.status === "running"
            ? "running"
            : "done";
    return {
      name,
      state,
      seconds: step?.endedAt ? seconds(step.startedAt, step.endedAt) : null,
      output: step?.output ?? [],
    };
  });
}

export function seconds(from: string, to: string): number {
  return Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000));
}

/** "3 min 23 s", "48 s". */
export function formatSeconds(total: number): string {
  const minutes = Math.floor(total / 60);
  const rest = total % 60;
  return minutes ? `${minutes} min ${rest} s` : `${rest} s`;
}
