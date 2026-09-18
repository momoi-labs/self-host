import { useEffect, useRef } from "react";
import { Button, Progress } from "@momoi-labs/kiso-react";

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
import { Causes } from "./Causes.js";
import { Icon } from "./Icon.js";
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

/**
 * The machine's last lifecycle action, step by step: which ones are done,
 * which one it is on, and where it stopped when it failed. While the action
 * runs this is the progress; afterwards it is the account of what happened,
 * with the failed step's own output beside it so the reason is read here and
 * not hunted for in a two-thousand-line log.
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
  if (!run)
    return (
      <p className="muted">
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
  return (
    <div className="run">
      <div className="run-head">
        <div>
          <p className="run-title">
            {run.status === "running" ? (TITLES[run.action] ?? run.action) : noun}
            {/* In progress is neutral here as everywhere else in the
                console: not good news yet, and not bad news either. */}
            {run.status === "running" ? (
              <StatusBadge tone="neutral">Running</StatusBadge>
            ) : run.status === "failed" ? (
              <StatusBadge tone="danger">Failed</StatusBadge>
            ) : (
              <StatusBadge tone="success">Succeeded</StatusBadge>
            )}
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
      {run.status === "running" ? (
        <Progress
          className="run-track"
          label={TITLES[run.action] ?? run.action}
          value={place ? place.at : null}
          max={place ? place.of : 100}
          valueText={place ? `${place.at} of ${place.of}` : "Working"}
        />
      ) : null}
      <ol className="run-steps">
        {steps.map((step) => {
          const lines = step.output.length;
          const timing =
            step.state === "pending"
              ? run.status === "running"
                ? ""
                : "not run"
              : step.state === "skipped"
                ? "not needed"
                : step.state === "running"
                  ? "running"
                  : step.seconds
                    ? formatSeconds(step.seconds)
                    : "";
          // A step that printed something opens on it. The one that is
          // running and the one that failed open by themselves; a finished
          // one waits to be asked, and says how much there is to read.
          const opens = lines > 0 || step.state === "failed";
          const open = step.state === "running" || step.state === "failed";
          const row = (
            <>
              <span className="run-step-mark">
                {step.state === "done" || step.state === "skipped" ? (
                  <Icon name="check" />
                ) : step.state === "failed" ? (
                  <Icon name="x" />
                ) : null}
              </span>
              <span className="run-step-name">{stepLabel(step.name)}</span>
              <span className="run-step-meta muted mono">
                {timing}
                {opens && step.state !== "running" && lines
                  ? `${timing ? " · " : ""}${lines} ${lines === 1 ? "line" : "lines"}`
                  : ""}
              </span>
              {step.state === "running" && lines ? (
                <span className="run-step-last muted mono">{step.output[lines - 1]}</span>
              ) : null}
            </>
          );
          return (
            <li key={step.name} className={`is-${step.state}`}>
              {opens ? (
                <details open={open}>
                  <summary className="run-step">{row}</summary>
                  <div className="run-step-detail">
                    {step.state === "failed" ? (
                      <FailedStep step={step} operation={operation} message={run.message} />
                    ) : (
                      <Output lines={step.output} follow={step.state === "running"} />
                    )}
                  </div>
                </details>
              ) : (
                <div className="run-step">{row}</div>
              )}
            </li>
          );
        })}
      </ol>
    </div>
  );
}

/**
 * What a step printed, the last few hundred lines of it. A step still going
 * keeps its newest line in view; a finished one reads from the top.
 */
function Output({ lines, follow }: { lines: string[]; follow: boolean }) {
  const shown = lines.slice(-OUTPUT_LINES);
  const pre = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (follow && pre.current) pre.current.scrollTop = pre.current.scrollHeight;
  }, [follow, lines.length]);
  if (!lines.length)
    return <p className="muted t-label">Nothing printed yet.</p>;
  return (
    <>
      {lines.length > shown.length ? (
        <p className="muted t-label">
          Earlier lines are in the log. Showing the last {shown.length}.
        </p>
      ) : null}
      <pre ref={pre} className="run-excerpt">
        {shown.join("\n")}
      </pre>
    </>
  );
}

/**
 * Why the step failed, in the order the reader needs it: the Platform's
 * framing, then what the guest printed just before it gave up.
 */
function FailedStep({
  step,
  operation,
  message,
}: {
  step: StepView;
  operation: Operation;
  message: string | null;
}) {
  const report = operation?.error;
  return (
    <>
      {report ? (
        <>
          <p>
            <b>{report.error}</b>
          </p>
          <Causes report={report} />
        </>
      ) : message ? (
        <p>
          <b>{message}</b>
        </p>
      ) : null}
      {step.output.length ? (
        <Output lines={step.output} follow />
      ) : (
        <p className="muted t-label">The step printed nothing before it failed.</p>
      )}
      <p className="muted t-label">
        Fix the cause in Configuration, save, and retry. A retry runs every step again; what is already installed is kept.
      </p>
    </>
  );
}
