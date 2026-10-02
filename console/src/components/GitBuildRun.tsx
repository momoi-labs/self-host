import { Button, LogView, LogViewLine, StatusBadge, StepList } from "@momoi-labs/kiso-react";
import type { PlatformEvent } from "../lib/platformEvents.js";

export function GitBuildRun({ event, error, busy, onRetry }: {
  event: PlatformEvent | null;
  error: string | null;
  busy: boolean;
  onRetry: () => void;
}) {
  if (!event) return <p className="muted run-empty">{error ?? "The build task appears here when the Host records it."}</p>;
  const active = event.status === "running" || event.status === "pending";
  return <div className="run">
    <div className="run-head">
      <div><p className="run-title">Build and deploy <StatusBadge tone={event.status === "failed" ? "danger" : event.status === "completed" || active ? "success" : "neutral"} pulse={active}>{event.status === "pending" ? "Queued" : event.status === "running" ? "Building" : event.status === "failed" ? "Failed" : "Completed"}</StatusBadge></p>
        <p className="muted t-label">Started {new Date(event.startedAt ?? event.occurredAt).toLocaleString()}</p>
      </div>
      {event.status === "failed" ? <Button size="sm" disabled={busy} onClick={onRetry}>Review and retry</Button> : null}
    </div>
    {error ? <p className="muted">{error}</p> : null}
    {event.changes?.filter((change) => change.setting === "Git revision").map((change) => <dl className="summary-facts" key={change.setting}>
      <dt>Previous commit</dt><dd><code>{change.from}</code></dd>
      <dt>Deployed commit</dt><dd><code>{change.to}</code></dd>
    </dl>)}
    <div className="run-list">
      <StepList label="Git deployment task" selected="deploy" onSelect={() => {}} steps={[{
        key: "deploy", label: "Build and deploy",
        state: event.status === "pending" ? "pending" : event.status === "running" ? "running" : event.status === "failed" ? "failed" : "done",
        detail: <LogView aria-label="Git deployment result" follow={active}>
          <LogViewLine>{event.description}</LogViewLine>
          {event.error ? <><LogViewLine className="log-error">{event.error.error}</LogViewLine>{event.error.caused_by.filter((cause) => cause !== event.error?.error).map((cause, index) => <LogViewLine key={index} className="log-error">{cause}</LogViewLine>)}</> : null}
          <LogViewLine className="log-time">Detailed build output is not retained. Runtime output is in Logs.</LogViewLine>
        </LogView>,
      }]} />
    </div>
  </div>;
}
