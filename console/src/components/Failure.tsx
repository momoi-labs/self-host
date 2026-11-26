import { useId, useState } from "react";
import { Alert, AlertContent, AlertLabel, AlertMessage, AlertTitle, Button } from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { Causes } from "./Causes.js";
import { Icon } from "./Icon.js";

const FRAMING = /^(failed|could not|cannot|unable)\b/i;

export function splitCauses(causes: string[]): { steps: string[]; root: string | null } {
  if (!causes.length) return { steps: [], root: null };
  let i = causes.length - 1;
  while (i > 0 && !FRAMING.test(causes[i - 1]) && !FRAMING.test(causes[i])) i--;
  return { steps: causes.slice(0, i), root: causes.slice(i).join(": ") };
}

function capitalize(text: string): string {
  return text ? text.charAt(0).toUpperCase() + text.slice(1) : text;
}

function GraphLane({
  kind,
  isLastStep,
}: {
  kind: "root" | "peek" | "fork" | "step" | "result";
  isLastStep?: boolean;
}) {
  return (
    <span className="failure-story-lane" aria-hidden="true">
      <svg className="failure-story-svg" width="30" height="100%">
        {kind === "root" ? (
          <line className="failure-story-trunk" x1="8" y1="8" x2="8" y2="100%" />
        ) : null}
        {kind === "peek" ? (
          <line className="failure-story-trunk" x1="8" y1="0" x2="8" y2="8" />
        ) : null}
        {kind === "fork" ? (
          <>
            <line className="failure-story-trunk" x1="8" y1="0" x2="8" y2="100%" />
            <path className="failure-story-branch" d="M 8 8 C 8 18, 22 15, 22 26" />
            <line className="failure-story-branch" x1="22" y1="26" x2="22" y2="100%" />
            <circle className="failure-story-dot-main" cx="8" cy="8" r="4" />
          </>
        ) : null}
        {kind === "step" ? (
          <>
            <line className="failure-story-trunk" x1="8" y1="0" x2="8" y2="100%" />
            {!isLastStep ? (
              <line className="failure-story-branch" x1="22" y1="0" x2="22" y2="100%" />
            ) : null}
            <circle className="failure-story-dot-branch" cx="22" cy="10" r="3" />
          </>
        ) : null}
        {kind === "result" ? (
          <>
            <line className="failure-story-trunk" x1="8" y1="0" x2="8" y2="8" />
            <circle className="failure-story-dot-main" cx="8" cy="8" r="4.5" />
          </>
        ) : null}
        {kind === "root" ? (
          <circle className="failure-story-dot-main" cx="8" cy="8" r="4.5" />
        ) : null}
        {kind === "peek" ? (
          <circle className="failure-story-dot-peek" cx="8" cy="8" r="3" />
        ) : null}
      </svg>
      {kind === "step" && isLastStep ? (
        <>
          <svg className="failure-story-svg-stem" width="30">
            <line className="failure-story-branch" x1="22" y1="0" x2="22" y2="100%" />
          </svg>
          <svg className="failure-story-svg-merge" width="30" height="18" viewBox="0 0 30 18">
            <path className="failure-story-branch" d="M 22 0 C 22 10, 8 8, 8 18" />
          </svg>
        </>
      ) : null}
    </span>
  );
}

export function Failure({
  failure,
  actionLabel,
  onAction,
  incident,
}: {
  failure: unknown;
  actionLabel?: string;
  onAction?: () => void;
  incident?: string;
}) {
  const [stepsOpen, setStepsOpen] = useState(false);
  const stepsId = useId();
  const report = asReport(failure);
  const action = actionLabel ? (
    <Button size="sm" onClick={onAction}>
      {actionLabel}
    </Button>
  ) : null;
  if (incident) {
    const { steps, root } = splitCauses(report.caused_by);
    const middle = steps.slice().reverse();
    const hasDetails = Boolean(root);
    return (
      <Alert variant="error" appearance="loud" tape="top" animated pulse={false} placement="inline">
        <AlertContent>
          <div className="failure-story-head">
            <AlertLabel>Error · {incident}</AlertLabel>
            {hasDetails || actionLabel ? (
              <div className="failure-story-actions">
                {hasDetails ? (
                  <Button
                    size="xs"
                    variant="ghost"
                    className="failure-story-toggle"
                    aria-expanded={stepsOpen}
                    aria-controls={stepsId}
                    onClick={() => setStepsOpen((open) => !open)}
                  >
                    <Icon name={stepsOpen ? "chevron-up" : "chevron-down"} size="sm" />
                    Details
                  </Button>
                ) : null}
                {actionLabel ? (
                  <Button size="xs" className="failure-story-primary" onClick={onAction}>
                    {actionLabel}
                  </Button>
                ) : null}
              </div>
            ) : null}
          </div>
          {root ? (
            <ol className="failure-story-rail" id={stepsId}>
              <li className="failure-story-root">
                <GraphLane kind="root" />
                <div className="failure-story-cell">
                  <span className="failure-story-kicker">Root cause</span>
                  <AlertMessage>{root}</AlertMessage>
                </div>
              </li>
              {stepsOpen ? (
                <>
                  {middle.length > 0 ? (
                    <li className="failure-story-fork">
                      <GraphLane kind="fork" />
                      <div className="failure-story-cell">
                        <span className="failure-story-kicker">Triggered while</span>
                      </div>
                    </li>
                  ) : null}
                  {middle.map((step, index) => (
                    <li
                      key={index}
                      className={index === middle.length - 1 ? "failure-story-step is-last" : "failure-story-step"}
                    >
                      <GraphLane kind="step" isLastStep={index === middle.length - 1} />
                      <div className="failure-story-cell">
                        <p>{step}</p>
                      </div>
                    </li>
                  ))}
                  <li className="failure-story-result">
                    <GraphLane kind="result" />
                    <div className="failure-story-cell">
                      <span className="failure-story-kicker">Result</span>
                      <p>{capitalize(report.error)}</p>
                    </div>
                  </li>
                </>
              ) : (
                <li
                  className="failure-story-peek"
                  aria-hidden="true"
                  onClick={() => setStepsOpen(true)}
                >
                  <GraphLane kind="peek" />
                  <div className="failure-story-cell failure-story-peek-body">
                    <span className="failure-story-kicker">
                      {middle.length > 0 ? "Triggered while" : "Result"}
                    </span>
                    <p>{middle.length > 0 ? middle[0] : capitalize(report.error)}</p>
                  </div>
                </li>
              )}
            </ol>
          ) : (
            <div className="failure-story-single">
              <span className="failure-story-kicker">Cause</span>
              <AlertMessage>{capitalize(report.error)}</AlertMessage>
            </div>
          )}
        </AlertContent>
      </Alert>
    );
  }
  return (
    <Alert variant="error">
      <Icon name="alert" size="md" />
      <AlertContent>
        <AlertTitle>{report.error}</AlertTitle>
        <Causes report={report} />
      </AlertContent>
      {action}
    </Alert>
  );
}
