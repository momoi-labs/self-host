import { useEffect, useState } from "react";
import {
  Button,
  LogView,
  LogViewLine,
  Pane,
  Split,
  Splitter,
  StepList,
} from "@momoi-labs/kiso-react";

import type { Operation } from "../lib/types.js";
import {
  NOUNS,
  TITLES,
  formatSeconds,
  position,
  seconds,
  stepLabel,
  stepViews,
  type Run,
  type StepView,
} from "../lib/runSteps.js";
import { useMediaQuery } from "../lib/useMediaQuery.js";
import { StatusBadge } from "./StatusBadge.js";

const startedFormat = new Intl.DateTimeFormat("en-GB", {
  day: "2-digit",
  month: "short",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

/** How much of a step's output is kept on screen; the log has the rest. */
const OUTPUT_LINES = 400;

/** Below this the sidebar stacks and the panel has no height to split. */
const WIDE = "(min-width: 1024px)";

/**
 * The machine's last lifecycle action, step by step: which ones are done,
 * which one it is on, and where it stopped when it failed. While the action
 * runs this is the progress; afterwards it is the account of what happened.
 * The steps sit beside the output of the one selected, so the reason a step
 * failed is read here and not hunted for in a two-thousand-line log.
 */
export function LastRun({
  run,
  operation,
  busy,
  onRetry,
  onOpenLog,
}: {
  run: Run | null;
  /** The record's own view of the operation, for the reason it reports. */
  operation: Operation;
  busy: boolean;
  onRetry: () => void;
  onOpenLog: () => void;
}) {
  // The step the reader asked for. Left alone, the output follows the run:
  // the step that is going, or the one that failed. A new run starts over.
  const [chosen, setChosen] = useState<string | null>(null);
  useEffect(() => setChosen(null), [run?.startedAt]);
  const wide = useMediaQuery(WIDE);
  if (!run)
    return (
      <p className="muted run-empty">
        {operation?.status === "running"
          ? "The run has started. Its steps appear here as the Host records them."
          : "Nothing has run on this machine yet, or its event log is gone."}
      </p>
    );
  const steps = stepViews(run);
  const noun = NOUNS[run.action] ?? run.action;
  const current = steps.find((step) => step.state === "running");
  const failed = steps.find((step) => step.state === "failed");
  const at = failed ?? current;
  const place = at ? position(run.action, at.name) : null;
  const total = run.endedAt ? seconds(run.startedAt, run.endedAt) : null;
  const started = startedFormat.format(new Date(run.startedAt));
  // What the output shows: the step the reader picked, else the one the run
  // is on or stopped at, else the end of a run that is over.
  const shown =
    steps.find((step) => step.name === chosen) ??
    at ??
    steps[run.status === "succeeded" ? steps.length - 1 : 0];
  const back =
    at && shown !== at ? (
      <Button type="button" size="sm" variant="ghost" onClick={() => setChosen(null)}>
        Back to {stepLabel(at.name)}
      </Button>
    ) : null;
  const output = shown ? (
    <Output step={shown} operation={operation} message={run.message} />
  ) : null;
  const list = (
    <StepList
      label={`${TITLES[run.action] ?? run.action}, step by step`}
      selected={shown?.name ?? null}
      onSelect={setChosen}
      steps={steps.map((step) => ({
        key: step.name,
        label: stepLabel(step.name),
        state: step.state,
        meta: meta(step, run),
        detail:
          !wide && step === shown ? (
            <div className="run-detail">
              {output}
              {back}
            </div>
          ) : undefined,
      }))}
    />
  );
  return (
    <div className="run">
      <div className="run-head">
        <div>
          <p className="run-title">
            {run.status === "running" ? (TITLES[run.action] ?? run.action) : noun}
            {/* A run that is going needs no badge: the title is already in
                the present tense and the header names the phase. A run that
                is over says how it ended. */}
            {run.status === "failed" ? (
              <StatusBadge tone="danger">Failed</StatusBadge>
            ) : run.status === "succeeded" ? (
              <StatusBadge tone="success">Succeeded</StatusBadge>
            ) : null}
          </p>
          <p className="muted t-label">
            Started {started}
            {run.status === "running" && place
              ? ` · step ${place.at} of ${place.of}`
              : run.status === "failed" && at
                ? ` · stopped at ${place ? `step ${place.at} of ${place.of}, ` : ""}${stepLabel(at.name)}`
                : total !== null
                  ? ` · ${formatSeconds(total)}`
                  : ""}
          </p>
        </div>
        <span className="run-actions">
          <Button type="button" size="sm" onClick={onOpenLog}>
            Open log
          </Button>
          {run.status === "failed" ? (
            <Button type="button" size="sm" variant="primary" onClick={onRetry} disabled={busy}>
              Retry {noun.toLowerCase()}
            </Button>
          ) : null}
        </span>
      </div>
      {/* The list is the progress: the rail fills as the run advances. Wide,
          the selected step's output fills the pane beside it; narrow, it
          opens under the step's own row. */}
      {wide ? (
        <Split className="run-split">
          <Pane className="run-steps">
            {list}
            {back}
          </Pane>
          <Splitter defaultSize={36} aria-label="Resize the steps and output panes" />
          <Pane className="grow run-output">{output}</Pane>
        </Split>
      ) : (
        <div className="run-list">{list}</div>
      )}
    </div>
  );
}

/**
 * What the row says beside the label: how long the step took and how much it
 * printed. The list has its own words for a step that is going or that was
 * not needed; a step still ahead says nothing until the run is over.
 */
function meta(step: StepView, run: Run): string | undefined {
  if (step.state === "running" || step.state === "skipped") return undefined;
  if (step.state === "pending") return run.status === "running" ? "" : "Not run";
  const lines = step.output.length;
  return [
    step.seconds ? formatSeconds(step.seconds) : "",
    lines ? `${lines} ${lines === 1 ? "line" : "lines"}` : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

/**
 * What a step printed, the last few hundred lines of it, as the log it is.
 * A step still going keeps its newest line in view; a finished one reads
 * from the top. The failed one ends with why, in the order the reader needs
 * it: what the guest printed, then the Platform's framing and its causes.
 */
function Output({
  step,
  operation,
  message,
}: {
  step: StepView;
  operation: Operation;
  message: string | null;
}) {
  if (step.state === "pending") return <p className="muted t-label">Not run.</p>;
  if (step.state === "skipped")
    return <p className="muted t-label">Not needed on this machine.</p>;
  const lines = step.output.slice(-OUTPUT_LINES);
  const report = operation?.error;
  const reason = report?.error ?? message ?? "The step failed.";
  return (
    <>
      <LogView
        follow={step.state !== "done"}
        aria-label={`${stepLabel(step.name)} output`}
      >
        {step.output.length > lines.length ? (
          <LogViewLine className="log-time">
            Earlier lines are in the log. Showing the last {lines.length}.
          </LogViewLine>
        ) : null}
        {lines.map((line, index) => (
          <LogViewLine key={index}>{line || " "}</LogViewLine>
        ))}
        {!lines.length && step.state !== "failed" ? (
          <LogViewLine className="log-time">
            {step.state === "running" ? "Nothing printed yet." : "Nothing printed."}
          </LogViewLine>
        ) : null}
        {step.state === "failed" ? (
          <>
            {lines.length ? <LogViewLine className="log-time">──</LogViewLine> : null}
            <LogViewLine className="log-error">✕ {reason}</LogViewLine>
            {(report?.caused_by ?? []).map((cause, index) => (
              <LogViewLine key={index} className="log-error">
                {"  ↳ "}
                {cause}
              </LogViewLine>
            ))}
          </>
        ) : null}
      </LogView>
      {step.state === "failed" ? (
        <p className="run-note muted t-label">
          Fix the cause in Configuration, save, and retry. A retry runs every step again; what is already installed is kept.
        </p>
      ) : null}
    </>
  );
}
