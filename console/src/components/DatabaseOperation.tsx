import { useEffect, useState } from "react";
import { LogView, LogViewLine, Pane, Split, Splitter, StatusBadge, StepList } from "@momoi-labs/kiso-react";

import { eventActionLabel, type OperationStage, type PlatformEvent } from "../lib/platformEvents.js";
import { formatSeconds, seconds } from "../lib/runSteps.js";
import { useMediaQuery } from "../lib/useMediaQuery.js";

/** A database Task's stages, each with the commands it ran and their output. */
export function DatabaseOperation({ event, child, error, title }: { event: PlatformEvent | null; child: PlatformEvent | null; error: string | null; /** Names the run when the Task's own action is too general. */ title?: string }) {
  const wide = useMediaQuery("(min-width: 1024px)");
  const [chosen, setChosen] = useState<string | null>(null);
  useEffect(() => setChosen(null), [event?.id]);
  if (!event) return <p className="muted run-empty">{error ?? "Waiting for operation details."}</p>;
  const stages: (Omit<OperationStage, "status"> & { status: PlatformEvent["status"] })[] = event.progress?.stages.length ? [...event.progress.stages] : [{
    id: event.id, label: eventActionLabel(event), status: event.status,
    startedAt: event.startedAt ?? event.occurredAt, finishedAt: event.finishedAt,
    output: [event.description], error: event.error,
  }];
  if (child) stages.push({ id: child.id, label: "Apply application variable", status: child.status, startedAt: child.startedAt ?? child.occurredAt, finishedAt: child.finishedAt, output: [child.description], error: child.error });
  const active = stages.find((stage) => stage.status === "failed") ?? stages.find((stage) => stage.status === "running");
  const shown = stages.find((stage) => stage.id === chosen) ?? active ?? stages[stages.length - 1];
  const output = <LogView aria-label={`${shown.label} result`} follow={shown.status === "running"}>
    <LogViewLine className="log-time">Started {new Date(shown.startedAt).toLocaleString()}</LogViewLine>
    {shown.output.map((line, index) => <LogViewLine key={index}>{line}</LogViewLine>)}
    {shown.error ? <><LogViewLine className="log-error">{shown.error.error}</LogViewLine>{shown.error.caused_by.map((cause, index) => <LogViewLine key={index} className="log-error">{cause}</LogViewLine>)}</> : null}
  </LogView>;
  const list = <StepList label="Database operation" selected={shown.id} onSelect={setChosen} steps={stages.map((stage) => ({
    key: stage.id, label: stage.label,
    state: stage.status === "completed" ? "done" : stage.status === "failed" ? "failed" : stage.status === "running" ? "running" : "pending",
    meta: stage.finishedAt ? formatSeconds(seconds(stage.startedAt, stage.finishedAt)) : undefined,
    detail: !wide && stage.id === shown.id ? output : undefined,
  }))} />;
  return <div className="run">
    <div className="run-head"><p className="run-title">{title ?? eventActionLabel(event)} <StatusBadge tone={(child ?? event).status === "failed" ? "danger" : ["completed", "running"].includes((child ?? event).status) ? "success" : "neutral"} pulse={(child ?? event).status === "running"}>{(child ?? event).status === "running" ? active?.label ?? "Running" : (child ?? event).status}</StatusBadge></p></div>
    {error ? <p className="muted">{error}</p> : null}
    {wide ? <Split className="run-split"><Pane className="run-steps">{list}</Pane><Splitter defaultSize={36} aria-label="Resize the steps and output panes" /><Pane className="grow run-output">{output}</Pane></Split> : <div className="run-list">{list}</div>}
  </div>;
}
